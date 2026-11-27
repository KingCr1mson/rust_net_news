//! OpenCode 传输层：运行时配置 + 一次 Chat Completions 请求。
//!
//! 本模块在 `brief` 数据流中的位置：
//!
//! ```text
//! main.rs（分派 brief 子命令）
//!   → 读取环境变量得到配置        ← 本模块的 OpenCodeConfig
//!   → 新闻获取 / 当天过滤（复用第一阶段能力）
//!   → brief.rs 构建提示词        ← 只把字符串交给本模块
//!   → 本模块发出唯一一次 AI 请求  ← 这里
//!   → brief.rs 校验并渲染结果
//! ```
//!
//! 主要职责（只做“配置与 HTTP”，不懂新闻领域）：
//!
//! - 校验运行时配置：`OPENCODE_API_KEY`、`OPENCODE_MODEL` 必填，`OPENCODE_BASE_URL`
//!   缺省时回落到 OpenCode 的兼容端点；配置错误必须在任何网络请求之前暴露。
//! - 定义 OpenAI Chat Completions 兼容的请求/响应数据结构（DTO），并完成一次
//!   非流式调用的编解码、认证头、超时与输出 token 上限。
//!
//! 输入与输出：输入是纯粹的提示词字符串（`system` 与 `user` 两段）与一个共享的
//! `reqwest::Client`；输出是模型 `message.content` 的原始字符串，本模块不解释其中
//! 的 JSON 语义，也不决定简报长什么样 —— 那是 `brief` 模块的领域问题。
//!
//! 错误边界：所有失败都收敛为本模块的传输错误类型（配置、超时、HTTP 状态、
//! 传输、响应协议），由 `main.rs` 决定“回退为原始列表并退出 0”还是“配置错误退出 2”。
//! 错误信息只允许描述原因和状态码：API Key 只出现在认证头里，绝不允许进入
//! 错误文本、提示词或日志。
//!
//! 依赖方向约束：本模块不得依赖 `brief` 的领域类型，`brief` 模块也不得依赖
//! `reqwest`。两者之间只通过字符串和错误类型协作，这样提示词/校验逻辑可以纯离线
//! 单测，HTTP 逻辑可以用本地 mock 服务单测。

use std::fmt;
use std::time::Duration;

use serde::Serialize;

/// 未配置 `OPENCODE_BASE_URL` 时使用的 OpenCode 兼容服务地址。
///
/// 该端点按量计费、面向 OpenAI Chat Completions 兼容调用；不内置默认模型，
/// 避免隐藏模型选择和相关费用。
pub const DEFAULT_BASE_URL: &str = "https://opencode.ai/inference";

/// 一次简报请求的生产总超时时长（秒）。
pub const OPENCODE_TIMEOUT_SECS: u64 = 60;

/// 一次简报请求的生产总超时。
pub const OPENCODE_TIMEOUT: Duration = Duration::from_secs(OPENCODE_TIMEOUT_SECS);

/// 单次简报请求的输出 token 上限，限制响应长度与费用。
pub const OPENCODE_MAX_TOKENS: u32 = 1200;

/// Chat Completions 兼容路径，实际请求地址为 `<base-url>` + 本路径。
pub const CHAT_COMPLETIONS_PATH: &str = "/openai/v1/chat/completions";

/// `Debug` 输出中替换 API Key 的占位符。
///
/// 用常量而不是空字符串，是为了让读者和日志一眼看出“这里本应有凭据、但被故意隐藏”。
pub const REDACTED_API_KEY: &str = "REDACTED";

/// OpenCode 运行时配置。
///
/// 三个字段全部来自环境变量或显式注入值：
///
/// - `api_key`：必填，只用于 `Authorization` 请求头，私有字段且不实现 `Debug` 透传；
/// - `model`：必填，请求体内使用的模型 ID（不内置默认值）；
/// - `endpoint`：由 Base URL 派生的完整请求地址，已通过 `reqwest::Url` 校验。
#[derive(Clone)]
pub struct OpenCodeConfig {
    // API Key 只在发送请求时读取，因此不提供公开访问器：少一个出口就少一条泄露路径。
    api_key: String,
    model: String,
    endpoint: reqwest::Url,
}

impl OpenCodeConfig {
    /// 用显式值构造配置。
    ///
    /// `base_url` 为 `None` 时使用 [`DEFAULT_BASE_URL`]。单元测试走这条路径，
    /// 因此不需要修改进程级环境变量（环境变量是全局状态，并行测试下互相污染）。
    ///
    /// 返回 `Result`：配置错误是“启动即失败”的可预期失败，交给调用方在发起网络
    /// 请求之前转成退出码 2。
    pub fn new(api_key: &str, model: &str, base_url: Option<&str>) -> Result<Self, OpenCodeError> {
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(OpenCodeError::config("OPENCODE_API_KEY 未配置或仅包含空白"));
        }

        let model = model.trim();
        if model.is_empty() {
            return Err(OpenCodeError::config("OPENCODE_MODEL 未配置或仅包含空白"));
        }

        // 空白 Base URL 视为“未配置”，与用户只设置空变量时的直觉一致。
        let raw_base_url = base_url.map(str::trim).filter(|raw| !raw.is_empty());
        let base_url = raw_base_url.unwrap_or(DEFAULT_BASE_URL);
        let parsed = reqwest::Url::parse(base_url).map_err(|err| {
            OpenCodeError::config(format!("OPENCODE_BASE_URL 不是有效地址（{err}）"))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(OpenCodeError::config(
                "OPENCODE_BASE_URL 必须是带主机名的 HTTP(S) 地址",
            ));
        }

        // 先去掉尾部 `/` 再拼路径：`Url::join` 会把最后一段当成可替换的文件名，
        // 例如 `https://host/inference` join `openai/...` 会丢掉 `inference`。
        let endpoint = format!(
            "{}{}",
            parsed.as_str().trim_end_matches('/'),
            CHAT_COMPLETIONS_PATH
        );
        let endpoint = reqwest::Url::parse(&endpoint).map_err(|err| {
            OpenCodeError::config(format!("OPENCODE_BASE_URL 无法构成请求地址（{err}）"))
        })?;

        Ok(Self {
            api_key: api_key.to_string(),
            model: model.to_string(),
            endpoint,
        })
    }

    /// 从 `OPENCODE_API_KEY`、`OPENCODE_MODEL`、`OPENCODE_BASE_URL` 读取配置。
    ///
    /// 只读取环境变量，不做任何网络请求，也不读写 `.env` 文件：配置来源保持显式。
    pub fn from_env() -> Result<Self, OpenCodeError> {
        let api_key = std::env::var("OPENCODE_API_KEY").unwrap_or_default();
        let model = std::env::var("OPENCODE_MODEL").unwrap_or_default();
        let base_url = std::env::var("OPENCODE_BASE_URL").ok();
        Self::new(&api_key, &model, base_url.as_deref())
    }

    /// 模型 ID，写入请求体。
    pub fn model(&self) -> &str {
        &self.model
    }

    /// 完整的 Chat Completions 请求地址（Base URL + 兼容路径）。
    pub fn endpoint(&self) -> &reqwest::Url {
        &self.endpoint
    }
}

impl fmt::Debug for OpenCodeConfig {
    /// 手写 `Debug` 而不是 `#[derive(Debug)]`：派生实现会把 `api_key` 原样打印，
    /// 一旦记录到日志或 panic 输出就构成凭据泄露。
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenCodeConfig")
            .field("api_key", &REDACTED_API_KEY)
            .field("model", &self.model)
            .field("endpoint", &self.endpoint.as_str())
            .finish()
    }
}

/// OpenCode 传输层的错误。
///
/// 所有变体都只描述“发生了什么”，不携带认证头、API Key 或完整提示词：
/// 这些错误会直接写入标准错误，任何人都可能看到。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OpenCodeError {
    /// 配置缺失或无效：必须在任何网络请求之前处理。
    #[error("OpenCode 配置错误：{message}")]
    Config {
        /// 可操作的原因，包含出问题的环境变量名。
        message: String,
    },
    /// 请求超时（发送与读取响应体共用同一总超时）。
    #[error("OpenCode 请求超时（{timeout_millis} 毫秒）")]
    Timeout {
        /// 实际生效的超时时长，单位毫秒；测试注入 100ms，生产为 60000ms。
        timeout_millis: u64,
    },
    /// 返回了非 2xx 状态码。
    #[error("OpenCode 返回非 2xx 状态码：{status}")]
    Status {
        /// HTTP 状态码；上游响应体被直接舍弃，避免把不可信内容写进终端。
        status: u16,
    },
    /// 网络层错误（连接失败、TLS 失败等）。
    #[error("OpenCode 请求失败：{reason}")]
    Http {
        /// 底层错误描述，不含认证信息。
        reason: String,
    },
    /// 响应协议无效：JSON 无法解析、没有 choice、content 不是字符串等。
    #[error("OpenCode 响应协议错误：{reason}")]
    Protocol {
        /// 具体原因。
        reason: String,
    },
}

impl OpenCodeError {
    /// 构造配置错误的小助手，统一 `message` 字段的写法。
    fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
        }
    }
}

/// 构造 AI 请求专用客户端。
///
/// 与新闻获取客户端（`fetch::build_client`）相比只改两件事，且都服务于
/// “每次 `brief` 至多一次请求、不留隐藏费用”：
///
/// - `redirect::Policy::none()`：不跟随 3xx，把 302/307 当成非 2xx 失败；
/// - `retry::never()`：关掉 reqwest 0.13 默认会对协议 NACK 做的重试。
///
/// 新闻获取客户端保持原有行为（跟随重定向、默认重试策略），不受本函数影响。
pub fn build_ai_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(crate::fetch::USER_AGENT)
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
}

/// 用生产超时（[`OPENCODE_TIMEOUT`]）发起一次简报请求。
pub async fn request_brief(
    client: &reqwest::Client,
    config: &OpenCodeConfig,
    system_prompt: &str,
    user_prompt: &str,
) -> Result<String, OpenCodeError> {
    request_brief_with_timeout(client, config, system_prompt, user_prompt, OPENCODE_TIMEOUT).await
}

/// 以指定总超时发起一次简报请求，返回模型 `message.content` 原始字符串。
///
/// 超时参数可注入：测试用 100ms 加 mock 延迟来稳定复现超时，生产入口固定传
/// [`OPENCODE_TIMEOUT`]（60 秒）。整个函数只发一次 HTTP 请求，任何失败都直接返回，
/// 不做重试 —— 重试意味着可能重复计费。
///
/// 这里用 `body(...)` + `bytes()` 而不是 `.json()`：不需要为 reqwest 开启额外
/// feature，也不会因为类型推断在请求体上多做一次克隆。
pub async fn request_brief_with_timeout(
    client: &reqwest::Client,
    config: &OpenCodeConfig,
    system_prompt: &str,
    user_prompt: &str,
    timeout: Duration,
) -> Result<String, OpenCodeError> {
    let payload = ChatRequest {
        model: &config.model,
        messages: [
            ChatRequestMessage {
                role: "system",
                content: system_prompt,
            },
            ChatRequestMessage {
                role: "user",
                content: user_prompt,
            },
        ],
        max_tokens: OPENCODE_MAX_TOKENS,
        stream: false,
    };
    let body = serde_json::to_vec(&payload).map_err(|err| OpenCodeError::Protocol {
        reason: format!("请求体序列化失败（{err}）"),
    })?;

    let request = client
        .post(config.endpoint.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::ACCEPT, "application/json")
        // Bearer 认证头只在此处出现：不打印、不写日志、不进错误文本。
        .bearer_auth(&config.api_key)
        // 请求级 timeout 覆盖“发送请求 + 读完响应体”两个阶段，
        // 因此下面 `bytes().await` 超时不会被误判成普通 HTTP 错误。
        .timeout(timeout)
        .body(body);

    // `.await` 让出执行权等待网络 I/O，不阻塞运行时线程。
    let response = request
        .send()
        .await
        .map_err(|err| classify_transport_error(&err, timeout))?;

    let status = response.status();
    if !status.is_success() {
        // 直接舍弃上游响应体：内容不可信，且可能回显请求头或提示词。
        return Err(OpenCodeError::Status {
            status: status.as_u16(),
        });
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|err| classify_transport_error(&err, timeout))?;
    let parsed: ChatResponse =
        serde_json::from_slice(&bytes).map_err(|err| OpenCodeError::Protocol {
            reason: format!("响应不是合法的 Chat Completions JSON（{err}）"),
        })?;

    // 规范只取首个 choice；`into_iter().next()` 表示“可能没有 choice”，
    // 空数组会走到 `ok_or_else` 而不会 panic。
    let choice = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| OpenCodeError::Protocol {
            reason: "响应没有 choices".to_string(),
        })?;
    choice
        .message
        .content
        .ok_or_else(|| OpenCodeError::Protocol {
            reason: "首个 choice 的 content 不是字符串".to_string(),
        })
}

/// 把 reqwest 错误分成超时与其它网络错误。
///
/// 只取逻辑分类，不拼接请求细节：`reqwest::Error` 的 Display 不含认证头与请求体。
fn classify_transport_error(err: &reqwest::Error, timeout: Duration) -> OpenCodeError {
    if err.is_timeout() {
        OpenCodeError::Timeout {
            timeout_millis: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        }
    } else {
        OpenCodeError::Http {
            reason: err.to_string(),
        }
    }
}

/// Chat Completions 请求体。
///
/// `Serialize` 把字段名直接映射成 JSON 键；`stream: false` 是类型上的保证，
/// 不是运行期开关，因此“非流式”这件事在 DTO 里就写死了。
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: [ChatRequestMessage<'a>; 2],
    max_tokens: u32,
    stream: bool,
}

/// 请求中的一条对话消息。
#[derive(Serialize)]
struct ChatRequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// Chat Completions 响应体：只声明用得到的字段，其余字段由 serde 忽略。
#[derive(serde::Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

/// 响应中的一个 choice。
#[derive(serde::Deserialize)]
struct ChatChoice {
    message: ChatResponseMessage,
}

/// 响应中的 assistant 消息。
///
/// `content` 是 `Option<String>`：协议允许它为 `null`（例如被安全策略截断），
/// 这种情形必须显式判成协议错误，而不是当成空字符串继续往下走。
#[derive(serde::Deserialize)]
struct ChatResponseMessage {
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{OpenCodeConfig, OpenCodeError, DEFAULT_BASE_URL};

    const TEST_KEY: &str = "oc_sk_secret_for_test";

    #[test]
    fn uses_default_base_url() {
        let config = OpenCodeConfig::new(TEST_KEY, "kimi-k3", None).expect("配置有效");
        assert!(
            config.endpoint().as_str().starts_with(DEFAULT_BASE_URL),
            "默认服务地址应为 {DEFAULT_BASE_URL}：{}",
            config.endpoint()
        );
        assert_eq!(
            config.endpoint().as_str(),
            "https://opencode.ai/inference/openai/v1/chat/completions"
        );
    }

    #[test]
    fn accepts_custom_http_base_url_for_mock() {
        let config = OpenCodeConfig::new(TEST_KEY, "kimi-k3", Some("http://127.0.0.1:8080"))
            .expect("配置有效");
        assert_eq!(
            config.endpoint().as_str(),
            "http://127.0.0.1:8080/openai/v1/chat/completions"
        );
    }

    #[test]
    fn rejects_blank_api_key() {
        for blank in ["", "   ", "\t\n"] {
            let err = OpenCodeConfig::new(blank, "kimi-k3", None).expect_err("空 Key 应被拒绝");
            match err {
                OpenCodeError::Config { message } => {
                    assert!(
                        message.contains("OPENCODE_API_KEY"),
                        "错误应指明字段：{message}"
                    );
                }
                other => panic!("错误类型应为 Config：{other}"),
            }
        }
    }

    #[test]
    fn rejects_blank_model() {
        for blank in ["", "  "] {
            let err = OpenCodeConfig::new(TEST_KEY, blank, None).expect_err("空模型应被拒绝");
            match err {
                OpenCodeError::Config { message } => {
                    assert!(
                        message.contains("OPENCODE_MODEL"),
                        "错误应指明字段：{message}"
                    );
                }
                other => panic!("错误类型应为 Config：{other}"),
            }
        }
    }

    #[test]
    fn rejects_invalid_base_url() {
        let err = OpenCodeConfig::new(TEST_KEY, "kimi-k3", Some("not a url"))
            .expect_err("无效地址应被拒绝");
        match err {
            OpenCodeError::Config { message } => {
                assert!(
                    message.contains("OPENCODE_BASE_URL"),
                    "错误应指明字段：{message}"
                );
            }
            other => panic!("错误类型应为 Config：{other}"),
        }
    }

    #[test]
    fn rejects_non_http_base_url() {
        for raw in ["ftp://example.com/inference", "file:///tmp/inference"] {
            let err = OpenCodeConfig::new(TEST_KEY, "kimi-k3", Some(raw))
                .expect_err("非 HTTP(S) 地址应被拒绝");
            match err {
                OpenCodeError::Config { message } => {
                    assert!(
                        message.contains("OPENCODE_BASE_URL"),
                        "错误应指明字段：{message}"
                    );
                }
                other => panic!("错误类型应为 Config：{other}"),
            }
        }
    }

    /// 凭据保护不只检查 Debug 不含某个固定字面量：必须确认脱敏占位符存在，
    /// 且模型与地址仍然可见，方便排查配置问题。
    #[test]
    fn config_debug_redacts_api_key() {
        let config = OpenCodeConfig::new(TEST_KEY, "kimi-k3", None).expect("配置有效");
        let debug = format!("{config:?}");
        assert!(!debug.contains(TEST_KEY), "Debug 不得包含 API Key：{debug}");
        assert!(
            debug.contains("REDACTED"),
            "Debug 应显示脱敏占位符：{debug}"
        );
        assert!(debug.contains("kimi-k3"), "Debug 应保留模型信息：{debug}");
        assert!(
            debug.contains(DEFAULT_BASE_URL),
            "Debug 应保留服务地址：{debug}"
        );
    }
}
