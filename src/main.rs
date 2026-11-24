//! 命令行入口：解析参数、并发获取、过滤排序并渲染输出。

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use clap::Parser;

use rust_net_news::fetch::{build_client, fetch_all};
use rust_net_news::render::{render_json, render_text};
use rust_net_news::source::{builtin_sources, NewsSource};
use rust_net_news::today::{day_range, filter_today, sort_items};

/// 抓取 IT之家与 Hacker News 当天的新闻。
#[derive(Debug, Parser)]
#[command(
    name = "rust_net_news",
    version,
    about = "输出 IT之家与 Hacker News 当天的新闻条目"
)]
struct Args {
    /// 以 JSON 数组输出
    #[arg(long)]
    json: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let sources = builtin_sources();

    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let code = run(&sources, args.json, Utc::now(), &mut stdout, &mut stderr).await;
    let _ = stdout.flush();

    ExitCode::from(code)
}

/// 执行完整流程，返回进程退出码。
///
/// - 全部源失败：标准错误列出每个源的失败原因，返回非零退出码。
/// - 部分源失败：标准输出保留成功源的条目，标准错误列出失败源，返回 0。
async fn run(
    sources: &[Arc<dyn NewsSource>],
    json: bool,
    now: DateTime<Utc>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let client = match build_client() {
        Ok(client) => client,
        Err(error) => {
            let _ = writeln!(err, "创建 HTTP 客户端失败：{error}");
            return 1;
        }
    };

    let outcome = match fetch_all(&client, sources).await {
        Ok(outcome) => outcome,
        Err(all_failed) => {
            let _ = writeln!(err, "{all_failed}");
            return 1;
        }
    };

    for failure in &outcome.failures {
        let _ = writeln!(err, "源获取失败：{failure}");
    }

    let mut items = filter_today(outcome.items, day_range(now));
    sort_items(&mut items);

    let rendered = if json {
        render_json(&items, out)
    } else {
        render_text(&items, out)
    };

    match rendered {
        Ok(()) => 0,
        Err(error) => {
            let _ = writeln!(err, "写出结果失败：{error}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rust_net_news::source::NewsSource;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const ITHOME_XML: &[u8] = include_bytes!("../tests/fixtures/ithome.xml");
    const HNRSS_XML: &[u8] = include_bytes!("../tests/fixtures/hnrss.xml");
    /// fixtures 中最早的发布时间所在的那一天（UTC+8）。
    const FIXTURE_DAY: &str = "2026-11-24T12:00:00+08:00";

    struct MockSource {
        name: &'static str,
        url: String,
    }

    impl MockSource {
        fn boxed(name: &'static str, url: String) -> Arc<dyn NewsSource> {
            Arc::new(Self { name, url })
        }
    }

    impl NewsSource for MockSource {
        fn name(&self) -> &'static str {
            self.name
        }

        fn url(&self) -> &str {
            &self.url
        }
    }

    fn rss_response(body: &[u8]) -> ResponseTemplate {
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/rss+xml; charset=utf-8")
            .set_body_bytes(body.to_vec())
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(FIXTURE_DAY)
            .expect("时间有效")
            .with_timezone(&chrono::Utc)
    }

    async fn mount_feed(server: &MockServer, route: &str, body: &[u8]) {
        Mock::given(method("GET"))
            .and(path(route))
            .respond_with(rss_response(body))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn all_sources_succeed() {
        let server = MockServer::start().await;
        mount_feed(&server, "/ithome", ITHOME_XML).await;
        mount_feed(&server, "/hn", HNRSS_XML).await;
        let sources = vec![
            MockSource::boxed("IT之家", format!("{}/ithome", server.uri())),
            MockSource::boxed("Hacker News", format!("{}/hn", server.uri())),
        ];

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run(&sources, false, now(), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let text = String::from_utf8(out).expect("UTF-8");
        assert!(text.contains("IT之家"), "{text}");
        assert!(text.contains("Hacker News"), "{text}");
        assert!(
            err.is_empty(),
            "不应有错误输出：{}",
            String::from_utf8_lossy(&err)
        );
    }

    #[tokio::test]
    async fn partial_failure_keeps_successful_items() {
        let server = MockServer::start().await;
        mount_feed(&server, "/ithome", ITHOME_XML).await;
        Mock::given(method("GET"))
            .and(path("/hn"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let sources = vec![
            MockSource::boxed("IT之家", format!("{}/ithome", server.uri())),
            MockSource::boxed("Hacker News", format!("{}/hn", server.uri())),
        ];

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run(&sources, false, now(), &mut out, &mut err).await;

        assert_eq!(code, 0, "部分失败应返回 0");
        let text = String::from_utf8(out).expect("UTF-8");
        assert!(text.contains("IT之家"));
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(
            stderr.contains("Hacker News"),
            "标准错误应列出失败源：{stderr}"
        );
        assert!(stderr.contains("500"), "标准错误应包含原因：{stderr}");
    }

    #[tokio::test]
    async fn all_sources_failing_exits_nonzero() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ithome"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/hn"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let sources = vec![
            MockSource::boxed("IT之家", format!("{}/ithome", server.uri())),
            MockSource::boxed("Hacker News", format!("{}/hn", server.uri())),
        ];

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run(&sources, false, now(), &mut out, &mut err).await;

        assert_ne!(code, 0, "全部失败应返回非零退出码");
        assert!(out.is_empty(), "全部失败时标准输出应为空");
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(
            stderr.contains("IT之家") && stderr.contains("500"),
            "{stderr}"
        );
        assert!(
            stderr.contains("Hacker News") && stderr.contains("404"),
            "{stderr}"
        );
    }

    #[tokio::test]
    async fn json_flag_outputs_array() {
        let server = MockServer::start().await;
        mount_feed(&server, "/ithome", ITHOME_XML).await;
        let sources = vec![MockSource::boxed(
            "IT之家",
            format!("{}/ithome", server.uri()),
        )];

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run(&sources, true, now(), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let parsed: serde_json::Value = serde_json::from_slice(&out).expect("标准输出是合法 JSON");
        let array = parsed.as_array().expect("是数组");
        assert_eq!(array.len(), 3);
        assert!(array.iter().all(|item| item["source"] == "IT之家"));
    }

    #[tokio::test]
    async fn no_items_today_prints_notice_and_exits_zero() {
        let server = MockServer::start().await;
        mount_feed(&server, "/ithome", ITHOME_XML).await;
        let sources = vec![MockSource::boxed(
            "IT之家",
            format!("{}/ithome", server.uri()),
        )];
        let next_week = chrono::DateTime::parse_from_rfc3339("2026-12-05T12:00:00+08:00")
            .expect("时间有效")
            .with_timezone(&chrono::Utc);

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run(&sources, false, next_week, &mut out, &mut err).await;

        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).expect("UTF-8"), "当天没有新闻\n");
        assert!(err.is_empty());
    }

    #[tokio::test]
    async fn no_items_today_with_json_prints_empty_array() {
        let server = MockServer::start().await;
        mount_feed(&server, "/ithome", ITHOME_XML).await;
        let sources = vec![MockSource::boxed(
            "IT之家",
            format!("{}/ithome", server.uri()),
        )];
        let next_week = chrono::DateTime::parse_from_rfc3339("2026-12-05T12:00:00+08:00")
            .expect("时间有效")
            .with_timezone(&chrono::Utc);

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run(&sources, true, next_week, &mut out, &mut err).await;

        assert_eq!(code, 0);
        let parsed: serde_json::Value = serde_json::from_slice(&out).expect("合法 JSON");
        assert_eq!(parsed, serde_json::json!([]));
    }
}
