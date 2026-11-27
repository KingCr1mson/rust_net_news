//! 命令行入口：解析参数、并发获取、过滤排序并渲染输出。
//!
//! 两条命令共用同一条“获取 → 当天过滤 → 排序”流水线：
//!
//! - 默认（无子命令）：按文本或 `--json` 输出列表，**不需要**任何 OpenCode 配置；
//! - `brief`：把当天新闻交给 AI 生成一份中文简报，失败时回退为原始列表。
//!
//! 本文件只做“编排”：参数解析、环境读取、调用库函数、决定退出码。
//! 网络传输在 `fetch` / `opencode`，领域校验在 `brief`，渲染在 `render`。

use std::io::Write;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};

use rust_net_news::brief::{Brief, BriefInput, MAX_INPUT_ITEMS, SYSTEM_PROMPT};
use rust_net_news::fetch::{build_client, fetch_all};
use rust_net_news::model::NewsItem;
use rust_net_news::opencode::{
    build_ai_client, request_brief_with_timeout, OpenCodeConfig, OPENCODE_TIMEOUT,
};
use rust_net_news::render::{render_json, render_text};
use rust_net_news::source::{builtin_sources, NewsSource};
use rust_net_news::today::{day_range, filter_today, sort_items};

/// 正常结束（包括“AI 失败但已回退原始列表”）。
const EXIT_OK: u8 = 0;
/// 失败：新闻源全部失败、写出失败、回退列表写出失败。
const EXIT_FAILURE: u8 = 1;
/// 用法或配置错误：在任何网络请求之前结束。
const EXIT_USAGE: u8 = 2;

/// 抓取 IT之家与 Hacker News 当天的新闻。
#[derive(Debug, Parser)]
#[command(
    name = "rust_net_news",
    version,
    about = "输出 IT之家与 Hacker News 当天的新闻条目",
    // 顶层参数只属于默认列表流程：`--json brief` 这种组合必须被拒绝，
    // 而不是把 `--json` 当成“对简报也生效”。
    args_conflicts_with_subcommands = true
)]
struct Args {
    /// 以 JSON 数组输出
    #[arg(long)]
    json: bool,

    /// 子命令；不写时执行默认的列表输出
    #[command(subcommand)]
    command: Option<Command>,
}

/// 可选的子命令。
///
/// 用 `Option<Command>` 而不是“默认再写一个 list 子命令”，是为了让
/// `rust_net_news` 与 `rust_net_news --json` 这两种既有调用保持原样。
#[derive(Debug, Subcommand)]
enum Command {
    /// 用 OpenCode 生成当天中文简报（需要 OPENCODE_API_KEY 与 OPENCODE_MODEL）
    Brief,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    let sources = builtin_sources();

    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    let code = match args.command {
        Some(Command::Brief) => {
            run_brief_command(&sources, Utc::now(), &mut stdout, &mut stderr).await
        }
        None => run(&sources, args.json, Utc::now(), &mut stdout, &mut stderr).await,
    };
    let _ = stdout.flush();

    ExitCode::from(code)
}

/// `brief` 子命令的入口：**先**读并校验配置，再决定要不要联网。
///
/// 顺序很重要：配置错误要在创建新闻请求之前就结束，否则用户会先等一轮新闻抓取，
/// 才看到“你忘了设置 OPENCODE_API_KEY”。
async fn run_brief_command(
    sources: &[Arc<dyn NewsSource>],
    now: DateTime<Utc>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let config = match OpenCodeConfig::from_env() {
        Ok(config) => config,
        Err(error) => {
            // `Display` 文本只含变量名与原因，不含 API Key，可以直接写终端。
            let _ = writeln!(err, "{error}");
            return EXIT_USAGE;
        }
    };

    run_brief(sources, &config, now, out, err).await
}

/// 执行默认的列表流程，返回进程退出码。
///
/// - 全部源失败：标准错误列出每个源的失败原因，返回 1。
/// - 部分源失败：标准输出保留成功源的条目，标准错误列出失败源，返回 0。
async fn run(
    sources: &[Arc<dyn NewsSource>],
    json: bool,
    now: DateTime<Utc>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let items = match collect_today_items(sources, now, err).await {
        Ok(items) => items,
        Err(code) => return code,
    };

    let rendered = if json {
        render_json(&items, out)
    } else {
        render_text(&items, out)
    };

    match rendered {
        Ok(()) => EXIT_OK,
        Err(error) => {
            let _ = writeln!(err, "写出结果失败：{error}");
            EXIT_FAILURE
        }
    }
}

/// 获取、过滤并排序当天条目；同时把每个失败源的警告写入 `err`。
///
/// 默认列表与 `brief` 共用这一段，所以抽成 helper：既避免两份容易走偏的重复流程，
/// 也保证“部分源失败仍继续”“全部源失败退出 1”这两种既有语义只实现一次。
///
/// 返回 `Err(退出码)` 表示错误信息已经写完，调用方直接返回该退出码即可；
/// 用 `u8` 而不是自定义错误类型，是因为这里只关心进程退出码。
async fn collect_today_items(
    sources: &[Arc<dyn NewsSource>],
    now: DateTime<Utc>,
    err: &mut impl Write,
) -> Result<Vec<NewsItem>, u8> {
    let client = match build_client() {
        Ok(client) => client,
        Err(error) => {
            let _ = writeln!(err, "创建 HTTP 客户端失败：{error}");
            return Err(EXIT_FAILURE);
        }
    };

    let outcome = match fetch_all(&client, sources).await {
        Ok(outcome) => outcome,
        Err(all_failed) => {
            let _ = writeln!(err, "{all_failed}");
            return Err(EXIT_FAILURE);
        }
    };

    for failure in &outcome.failures {
        let _ = writeln!(err, "源获取失败：{failure}");
    }

    // `outcome.items` 已被移动进过滤函数：`filter_today` 接收 `IntoIterator<Item = NewsItem>`，
    // 拿走所有权以免逐条克隆。
    let mut items = filter_today(outcome.items, day_range(now));
    sort_items(&mut items);
    Ok(items)
}

/// 用生产配置（60 秒 AI 总超时）生成简报。
async fn run_brief(
    sources: &[Arc<dyn NewsSource>],
    config: &OpenCodeConfig,
    now: DateTime<Utc>,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    run_brief_with_timeout(sources, config, now, OPENCODE_TIMEOUT, out, err).await
}

/// 生成简报，AI 超时由调用方注入（测试用 100ms，生产固定 60 秒）。
///
/// 决策顺序与规约一一对应：
///
/// 1. 新闻源全部失败 → 退出 1，不调用 AI；
/// 2. 当天没有条目 → 输出“当天没有新闻”，退出 0，不调用 AI；
/// 3. 有条目 → 至多一次 AI 请求；成功则渲染简报，失败则回退原始列表。
async fn run_brief_with_timeout(
    sources: &[Arc<dyn NewsSource>],
    config: &OpenCodeConfig,
    now: DateTime<Utc>,
    ai_timeout: Duration,
    out: &mut impl Write,
    err: &mut impl Write,
) -> u8 {
    let items = match collect_today_items(sources, now, err).await {
        Ok(items) => items,
        Err(code) => return code,
    };

    if items.is_empty() {
        return match render_text(&items, out) {
            Ok(()) => EXIT_OK,
            Err(error) => {
                let _ = writeln!(err, "写出结果失败：{error}");
                EXIT_FAILURE
            }
        };
    }

    // `BriefInput` 内部只克隆前 `MAX_INPUT_ITEMS` 条；`items` 本身保留完整列表，
    // 供 AI 失败时的回退使用（回退不受 50 条上限影响）。
    let input = BriefInput::new(&items);
    if input.is_truncated() {
        let _ = writeln!(
            err,
            "当天新闻超过 {MAX_INPUT_ITEMS} 条，仅使用最新 {MAX_INPUT_ITEMS} 条生成简报"
        );
    }

    match generate_brief(config, &input, ai_timeout).await {
        Ok(brief) => match input.render(&brief, out) {
            Ok(()) => EXIT_OK,
            Err(error) => {
                let _ = writeln!(err, "写出简报失败：{error}");
                EXIT_FAILURE
            }
        },
        Err(reason) => {
            // 6.2 的临时行为：先用退出码 1 暴露“AI 失败”。回退完整列表在 6.3 实现。
            let _ = writeln!(err, "AI 简报生成失败：{reason}");
            EXIT_FAILURE
        }
    }
}

/// 一次 AI 简报尝试：构建提示词 → 请求 → 校验结构。
///
/// 三种失败（提示词、传输、校验）对调用方是同一件事，所以统一收敛成 `String` 原因，
/// 而不是让 `main` 去区分 `BriefError` / `OpenCodeError`。
async fn generate_brief(
    config: &OpenCodeConfig,
    input: &BriefInput,
    ai_timeout: Duration,
) -> Result<Brief, String> {
    let prompt = input
        .build_user_prompt()
        .map_err(|error| error.to_string())?;

    // 简报用独立客户端：不跟随重定向、不自动重试，避免一次 brief 产生多次请求与费用。
    // 新闻抓取仍使用 `fetch::build_client`，那套行为不受影响。
    let client = build_ai_client().map_err(|_| "创建 AI HTTP 客户端失败".to_string())?;

    let content = request_brief_with_timeout(&client, config, SYSTEM_PROMPT, &prompt, ai_timeout)
        .await
        .map_err(|error| error.to_string())?;

    input
        .parse_response(&content)
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rust_net_news::opencode::OpenCodeConfig;
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

    // ---- AI mock 辅助 ----------------------------------------------------

    const TEST_KEY: &str = "oc_sk_secret_for_test";
    const TEST_MODEL: &str = "kimi-k3";

    /// 一份引用第 1 条本地条目的合法简报：概览与重点文字由模型提供，
    /// 来源名称和链接必须由本地条目补回。
    const VALID_BRIEF: &str = r#"{
        "overview": "当天新闻集中在开源模型与芯片",
        "highlights": [
            {
                "title": "开源模型进展",
                "summary": "多个团队发布了新模型",
                "source_ids": [1]
            }
        ]
    }"#;

    /// 用 mock 服务地址构造配置，测试不会碰环境变量。
    fn ai_config(server: &MockServer) -> OpenCodeConfig {
        OpenCodeConfig::new(TEST_KEY, TEST_MODEL, Some(&server.uri())).expect("配置有效")
    }

    /// 把一段内容包成 OpenAI Chat Completions 兼容的成功响应。
    fn chat_completion_response(content: &str) -> ResponseTemplate {
        let body = serde_json::to_vec(&serde_json::json!({
            "id": "chatcmpl_test",
            "object": "chat.completion",
            "model": TEST_MODEL,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop"
            }]
        }))
        .expect("响应可序列化");
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/json")
            .set_body_bytes(body)
    }

    /// 让 AI mock 返回给定状态码（用于 401/429/500 之类的失败用例）。
    async fn mount_ai_status(server: &MockServer, status: u16) {
        Mock::given(method("POST"))
            .and(path("/openai/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(status).set_body_string("upstream boom"))
            .mount(server)
            .await;
    }

    /// AI mock 收到的请求数：用来断言“零请求”或“至多一次请求”。
    async fn ai_request_count(server: &MockServer) -> usize {
        server
            .received_requests()
            .await
            .expect("mock 记录了请求")
            .len()
    }

    #[tokio::test]
    async fn brief_success_outputs_validated_brief() {
        let news = MockServer::start().await;
        mount_feed(&news, "/ithome", ITHOME_XML).await;
        let sources = vec![MockSource::boxed(
            "IT之家",
            format!("{}/ithome", news.uri()),
        )];

        let ai = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/openai/v1/chat/completions"))
            .respond_with(chat_completion_response(VALID_BRIEF))
            .mount(&ai)
            .await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let text = String::from_utf8(out).expect("UTF-8");
        assert!(
            text.contains("当天新闻集中在开源模型与芯片"),
            "缺少概览：{text}"
        );
        assert!(text.contains("开源模型进展"), "缺少重点：{text}");
        // 来源名称与链接必须来自本地条目，不是模型给的。
        assert!(text.contains("IT之家"), "缺少本地来源：{text}");
        assert!(
            text.contains("https://www.ithome.com/1/031/466.htm"),
            "缺少本地链接：{text}"
        );
        assert_eq!(ai_request_count(&ai).await, 1, "一次 brief 只请求一次");
    }

    #[tokio::test]
    async fn brief_no_items_does_not_call_ai() {
        let news = MockServer::start().await;
        mount_feed(&news, "/ithome", ITHOME_XML).await;
        let sources = vec![MockSource::boxed(
            "IT之家",
            format!("{}/ithome", news.uri()),
        )];
        let ai = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(chat_completion_response(VALID_BRIEF))
            .mount(&ai)
            .await;
        // 下一周：fixtures 里的条目都不属于“当天”。
        let next_week = chrono::DateTime::parse_from_rfc3339("2026-12-05T12:00:00+08:00")
            .expect("时间有效")
            .with_timezone(&chrono::Utc);

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), next_week, &mut out, &mut err).await;

        assert_eq!(code, 0);
        assert_eq!(String::from_utf8(out).expect("UTF-8"), "当天没有新闻\n");
        assert_eq!(ai_request_count(&ai).await, 0, "没有新闻时不得请求 AI");
    }

    #[tokio::test]
    async fn brief_partial_source_failure_keeps_warning_and_generates() {
        let news = MockServer::start().await;
        mount_feed(&news, "/ithome", ITHOME_XML).await;
        Mock::given(method("GET"))
            .and(path("/hn"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&news)
            .await;
        let sources = vec![
            MockSource::boxed("IT之家", format!("{}/ithome", news.uri())),
            MockSource::boxed("Hacker News", format!("{}/hn", news.uri())),
        ];
        let ai = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(chat_completion_response(VALID_BRIEF))
            .mount(&ai)
            .await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 0, "部分源失败仍应生成简报");
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(stderr.contains("Hacker News"), "应保留失败源警告：{stderr}");
        assert!(stderr.contains("500"), "应保留失败原因：{stderr}");
        assert!(
            String::from_utf8(out)
                .expect("UTF-8")
                .contains("开源模型进展"),
            "成功源应能生成简报"
        );
    }

    #[tokio::test]
    async fn brief_all_sources_fail_does_not_call_ai() {
        let news = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ithome"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&news)
            .await;
        Mock::given(method("GET"))
            .and(path("/hn"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&news)
            .await;
        let sources = vec![
            MockSource::boxed("IT之家", format!("{}/ithome", news.uri())),
            MockSource::boxed("Hacker News", format!("{}/hn", news.uri())),
        ];
        let ai = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(chat_completion_response(VALID_BRIEF))
            .mount(&ai)
            .await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 1, "全部源失败应以 1 退出");
        assert!(out.is_empty(), "全部源失败时标准输出应为空");
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(
            stderr.contains("IT之家") && stderr.contains("500"),
            "{stderr}"
        );
        assert!(
            stderr.contains("Hacker News") && stderr.contains("404"),
            "{stderr}"
        );
        assert_eq!(ai_request_count(&ai).await, 0, "全部源失败时不得请求 AI");
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
