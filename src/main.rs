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
/// 3. 有条目 → 至多一次 AI 请求；成功则渲染简报，失败则回退完整原始列表并退出 0。
///
/// 注意第 3 条的退出码：只要回退列表写成功就是 0。判断“是否 AI 简报”只能看标准错误，
/// 这是为了提高交互可用性而接受的权衡（详见 design D6）。
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
        Ok(brief) => match input.render(&brief, out).and_then(|()| out.flush()) {
            Ok(()) => EXIT_OK,
            Err(error) => {
                let _ = writeln!(err, "写出简报失败：{error}");
                EXIT_FAILURE
            }
        },
        Err(reason) => {
            // 失败不能只报错：用户拿这份输出是为了看新闻，所以把当天新闻完整降级输出。
            // 两行 stderr 缺一不可：前一行说 AI 的哪一步失败，后一行说 stdout 究竟是什么，
            // 避免读者把它当成 AI 生成的简报。
            let _ = writeln!(err, "AI 简报生成失败：{reason}");
            let _ = writeln!(err, "回退为原始新闻列表");

            // 回退用的是未截断的 `items`（不是 `input` 里的 50 条），并且只调一次文本渲染。
            // `and_then(|()| out.flush())`：写入成功但 flush 失败的输出同样没真的落到终端，
            // 必须归为退出码 1，而不是默默返回 0。
            match render_text(&items, out).and_then(|()| out.flush()) {
                Ok(()) => EXIT_OK,
                Err(error) => {
                    let _ = writeln!(err, "输出原始新闻列表失败：{error}");
                    EXIT_FAILURE
                }
            }
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

    /// 生成 `count` 条都属于“当天”的 RSS 条目（每条相隔 1 分钟）。
    ///
    /// 用于验证“发送给 AI 的输入被截断到 50 条，但回退列表仍完整”这条规约。
    fn many_items_feed(count: usize) -> Vec<u8> {
        let base = chrono::DateTime::parse_from_rfc3339(FIXTURE_DAY)
            .expect("时间有效")
            .with_timezone(&chrono::Utc);
        let mut items = String::new();
        for index in 0..count {
            let published = base - chrono::Duration::minutes(index as i64);
            items.push_str(&format!(
                "<item><title>批量条目 {index}</title><link>https://example.com/{index}</link><pubDate>{}</pubDate></item>",
                published.format("%a, %d %b %Y %H:%M:%S GMT")
            ));
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?><rss version=\"2.0\"><channel><title>批量源</title><link>https://example.com/</link><description>测试源</description>{items}</channel></rss>"
        )
        .into_bytes()
    }

    /// 测试用的 writer：可以指定在 `write` 或 `flush` 阶段失败。
    ///
    /// 为什么两种都测：`writeln!` 当场失败与“数据还在缓冲、flush 时才发现失败”是两条
    /// 不同的出口，而退出码必须是同一个 1。
    struct FailingWriter {
        fail_on: FailOn,
    }

    enum FailOn {
        Write,
        Flush,
    }

    impl Write for FailingWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            if matches!(self.fail_on, FailOn::Write) {
                // `ErrorKind::Other`（`Error::other` 的简写）模拟最常见的写出失败。
                return Err(std::io::Error::other("写入失败（测试注入）"));
            }
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            if matches!(self.fail_on, FailOn::Flush) {
                return Err(std::io::Error::other("刷新失败（测试注入）"));
            }
            Ok(())
        }
    }

    /// 把一个 RSS 响应挂到 `/ithome`，返回只含该源的源列表。
    async fn single_news_source(news: &MockServer, body: &[u8]) -> Vec<Arc<dyn NewsSource>> {
        mount_feed(news, "/ithome", body).await;
        vec![MockSource::boxed(
            "IT之家",
            format!("{}/ithome", news.uri()),
        )]
    }

    #[tokio::test]
    async fn brief_ai_429_falls_back_to_original_list() {
        let news = MockServer::start().await;
        let sources = single_news_source(&news, ITHOME_XML).await;
        let ai = MockServer::start().await;
        mount_ai_status(&ai, 429).await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        // 回退是“新闻仍然可用”的表达，因此退出码为 0；但标准错误必须明确报错。
        assert_eq!(code, 0, "AI 失败后回退成功应以 0 退出");
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(stderr.contains("AI 简报生成失败"), "缺少失败提示：{stderr}");
        assert!(
            stderr.contains("回退为原始新闻列表"),
            "缺少回退提示：{stderr}"
        );
        assert!(stderr.contains("429"), "应保留状态码：{stderr}");

        let text = String::from_utf8(out).expect("UTF-8");
        assert!(
            text.contains("https://www.ithome.com/1/031/466.htm"),
            "回退列表应包含原始新闻链接：{text}"
        );
        assert!(!text.contains("AI 简报"), "回退列表不得伪装成简报：{text}");
        assert_eq!(ai_request_count(&ai).await, 1, "失败后不得重试");
    }

    #[tokio::test]
    async fn brief_ai_timeout_falls_back_without_retry() {
        let news = MockServer::start().await;
        let sources = single_news_source(&news, ITHOME_XML).await;
        let ai = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                chat_completion_response(VALID_BRIEF).set_delay(Duration::from_millis(800)),
            )
            .mount(&ai)
            .await;

        // 注入 100ms 超时，避免真的等 60 秒；生产入口 run_brief 仍固定传 60 秒。
        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief_with_timeout(
            &sources,
            &ai_config(&ai),
            now(),
            Duration::from_millis(100),
            &mut out,
            &mut err,
        )
        .await;

        assert_eq!(code, 0, "超时后回退成功应以 0 退出");
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(stderr.contains("AI 简报生成失败"), "{stderr}");
        assert!(stderr.contains("回退为原始新闻列表"), "{stderr}");
        assert!(stderr.contains("超时"), "应说明是超时：{stderr}");
        assert!(
            String::from_utf8(out)
                .expect("UTF-8")
                .contains("https://www.ithome.com/1/031/466.htm"),
            "回退列表应包含原始新闻"
        );
        assert_eq!(ai_request_count(&ai).await, 1, "超时不得自动重试");
    }

    #[tokio::test]
    async fn brief_invalid_json_falls_back() {
        let news = MockServer::start().await;
        let sources = single_news_source(&news, ITHOME_XML).await;
        let ai = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(chat_completion_response(
                "这不是 JSON，也没有 Markdown 围栏",
            ))
            .mount(&ai)
            .await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(stderr.contains("AI 简报生成失败"), "{stderr}");
        assert!(stderr.contains("回退为原始新闻列表"), "{stderr}");
        assert!(String::from_utf8(out)
            .expect("UTF-8")
            .contains("https://www.ithome.com/1/031/466.htm"));
    }

    #[tokio::test]
    async fn brief_unknown_source_id_falls_back() {
        let news = MockServer::start().await;
        let sources = single_news_source(&news, ITHOME_XML).await;
        let ai = MockServer::start().await;
        // 引用了不存在的 ID：整份简报无效（否则读者会看到无法溯源的“新闻”）。
        let content = r#"{
            "overview": "概览",
            "highlights": [
                {"title": "重点", "summary": "说明", "source_ids": [99]}
            ]
        }"#;
        Mock::given(method("POST"))
            .respond_with(chat_completion_response(content))
            .mount(&ai)
            .await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(stderr.contains("AI 简报生成失败"), "{stderr}");
        assert!(stderr.contains("回退为原始新闻列表"), "{stderr}");
        assert!(
            !String::from_utf8(out).expect("UTF-8").contains("AI 简报"),
            "无效简报不得被输出"
        );
    }

    #[tokio::test]
    async fn brief_fallback_above_50_contains_all_items() {
        let news = MockServer::start().await;
        let sources = single_news_source(&news, &many_items_feed(51)).await;
        let ai = MockServer::start().await;
        mount_ai_status(&ai, 500).await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 0);
        let text = String::from_utf8(out).expect("UTF-8");
        assert_eq!(
            text.matches("链接：").count(),
            51,
            "回退列表必须是完整当天列表，不受 AI 输入 50 条上限影响"
        );
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(
            stderr.contains("仅使用最新 50 条"),
            "超过 50 条时应在标准错误提示截断：{stderr}"
        );
        assert!(stderr.contains("回退为原始新闻列表"), "{stderr}");
        assert_eq!(ai_request_count(&ai).await, 1);
    }

    #[tokio::test]
    async fn brief_fallback_write_failure_exits_one() {
        let news = MockServer::start().await;
        let sources = single_news_source(&news, ITHOME_XML).await;
        let ai = MockServer::start().await;
        mount_ai_status(&ai, 500).await;

        let mut out = FailingWriter {
            fail_on: FailOn::Write,
        };
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 1, "回退列表写出失败应以 1 退出");
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(stderr.contains("回退为原始新闻列表"), "{stderr}");
        assert!(stderr.contains("失败"), "标准错误应说明写出失败：{stderr}");
    }

    #[tokio::test]
    async fn brief_fallback_flush_failure_exits_one() {
        let news = MockServer::start().await;
        let sources = single_news_source(&news, ITHOME_XML).await;
        let ai = MockServer::start().await;
        mount_ai_status(&ai, 500).await;

        // 数据写进了缓冲，但 flush 失败：对用户而言输出并没有真的落到终端，必须报 1。
        let mut out = FailingWriter {
            fail_on: FailOn::Flush,
        };
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 1, "回退列表 flush 失败应以 1 退出");
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(stderr.contains("回退为原始新闻列表"), "{stderr}");
    }

    #[tokio::test]
    async fn brief_flush_failure_exits_one() {
        let news = MockServer::start().await;
        let sources = single_news_source(&news, ITHOME_XML).await;
        let ai = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(chat_completion_response(VALID_BRIEF))
            .mount(&ai)
            .await;

        let mut out = FailingWriter {
            fail_on: FailOn::Flush,
        };
        let mut err: Vec<u8> = Vec::new();
        let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

        assert_eq!(code, 1, "简报 flush 失败应以 1 退出");
        let stderr = String::from_utf8(err).expect("UTF-8");
        assert!(
            stderr.contains("简报"),
            "标准错误应说明是简报写出失败：{stderr}"
        );
    }

    /// 无论是 HTTP 错误体、畸形的 2xx JSON 还是畸形的简报内容，标准错误与标准输出都
    /// 不得包含 API Key。虚构 key 由上游“故意”回显，用来模拟最坏情况。
    #[tokio::test]
    async fn brief_errors_never_expose_api_key() {
        // 情形一：401，上游把 Key 与提示词回显在响应体里。
        // 情形二：200，但 JSON 结构不符，且非法字段值就是 Key（serde 的 Display 会回显它）。
        // 情形三：200，content 是畸形简报 JSON，且字段值含 Key。
        enum Reply {
            Status,
            MalformedJson(Vec<u8>),
            MalformedBrief(String),
        }
        let replies = vec![
            Reply::Status,
            Reply::MalformedJson(format!(r#"{{"choices": "{TEST_KEY}"}}"#).into_bytes()),
            Reply::MalformedBrief(format!(
                r#"{{"overview": "{TEST_KEY}", "highlights": "{TEST_KEY}"}}"#
            )),
        ];

        for reply in replies {
            let news = MockServer::start().await;
            let sources = single_news_source(&news, ITHOME_XML).await;
            let ai = MockServer::start().await;
            match reply {
                Reply::Status => {
                    Mock::given(method("POST"))
                        .respond_with(ResponseTemplate::new(401).set_body_string(format!(
                            "invalid token: Bearer {TEST_KEY}; prompt=批量条目"
                        )))
                        .mount(&ai)
                        .await;
                }
                Reply::MalformedJson(body) => {
                    Mock::given(method("POST"))
                        .respond_with(
                            ResponseTemplate::new(200)
                                .insert_header("content-type", "application/json")
                                .set_body_bytes(body),
                        )
                        .mount(&ai)
                        .await;
                }
                Reply::MalformedBrief(content) => {
                    Mock::given(method("POST"))
                        .respond_with(chat_completion_response(&content))
                        .mount(&ai)
                        .await;
                }
            }

            let mut out: Vec<u8> = Vec::new();
            let mut err: Vec<u8> = Vec::new();
            let code = run_brief(&sources, &ai_config(&ai), now(), &mut out, &mut err).await;

            assert_eq!(code, 0, "回退成功应以 0 退出");
            let stderr = String::from_utf8(err).expect("UTF-8");
            let stdout = String::from_utf8(out).expect("UTF-8");
            assert!(
                stderr.contains("回退为原始新闻列表"),
                "应走回退路径：{stderr}"
            );
            assert!(
                !stderr.contains(TEST_KEY),
                "标准错误不得包含 API Key：{stderr}"
            );
            assert!(
                !stdout.contains(TEST_KEY),
                "标准输出不得包含 API Key：{stdout}"
            );
            assert!(
                !stderr.contains("Bearer"),
                "标准错误不得包含认证头：{stderr}"
            );
        }
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
