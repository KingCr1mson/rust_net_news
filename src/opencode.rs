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
        // 在配置阶段就验证 Key 能否构成 Bearer 头，而不是等发请求时才失败：
        // 发送阶段失败会走“AI 失败 → 回退列表 → 退出 0”，用户拿不到“配置错了”的信号。
        // 这里也不把 Key 本身写进错误文本。
        if reqwest::header::HeaderValue::from_str(api_key).is_err() {
            return Err(OpenCodeError::config(
                "OPENCODE_API_KEY 含无法用于认证头的字符（如控制字符或换行）",
            ));
        }

        let model = model.trim();
        if model.is_empty() {
            return Err(OpenCodeError::config("OPENCODE_MODEL 未配置或仅包含空白"));
        }

        // 区分“没设置”与“设置了但为空白”：后者是配置错误，不能静默回落到默认端点，
        // 否则用户以为请求发往自己的网关，实际却发到了公网默认地址。
        let raw_base_url = match base_url {
            None => DEFAULT_BASE_URL,
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(OpenCodeError::config("OPENCODE_BASE_URL 已设置但为空白"));
                }
                trimmed
            }
        };
        let parsed = reqwest::Url::parse(raw_base_url).map_err(|err| {
            OpenCodeError::config(format!("OPENCODE_BASE_URL 不是有效地址（{err}）"))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return Err(OpenCodeError::config(
                "OPENCODE_BASE_URL 必须是带主机名的 HTTP(S) 地址",
            ));
        }
        // 端点根后面要直接拼兼容路径，因此不能带查询串或片段：
        // `http://host/base?x=1` + `/openai/...` 会得到 `?x=1/openai/...`，那个路径并不存在。
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(OpenCodeError::config(
                "OPENCODE_BASE_URL 不能包含查询串（?）或片段（#）",
            ));
        }
        // userinfo 会隐式带上另一套凭据，不属于预期的“端点根”。
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(OpenCodeError::config(
                "OPENCODE_BASE_URL 不能包含用户名或密码",
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
        let base_url = base_url_from_env(std::env::var_os("OPENCODE_BASE_URL"))?;
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

/// 把 `OPENCODE_BASE_URL` 的原始环境值转成 `Option<String>`。
///
/// 为什么不用 `std::env::var(..).ok()`：那样「变量未设置」与「变量值不是合法 UTF-8」
/// 会合并成同一个 `None`，后者会被静默当成默认端点 —— 用户设置了地址，请求却发到
/// 别处，属于必须在配置阶段暴露的错误。
fn base_url_from_env(raw: Option<std::ffi::OsString>) -> Result<Option<String>, OpenCodeError> {
    match raw {
        None => Ok(None),
        Some(value) => value
            .into_string()
            .map(Some)
            .map_err(|_| OpenCodeError::config("OPENCODE_BASE_URL 不是有效的 Unicode 文本")),
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
            // 只用行列号和受控分类：serde 的 Display 会把上游的非法字段值原样带出来。
            reason: format!(
                "响应不是合法的 Chat Completions JSON（{}）",
                describe_json_error(&err)
            ),
        })?;

    // 规范只取首个 choice。下面只对首个元素做严格解析，因此后面那些畸形的 choice
    // 不会把一份本来可用的响应拖成协议错误。
    let first = parsed
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| OpenCodeError::Protocol {
            reason: "响应没有 choices".to_string(),
        })?;
    let choice: ChatChoice =
        serde_json::from_value(first).map_err(|_| OpenCodeError::Protocol {
            reason: "首个 choice 的结构不符合 Chat Completions 协议".to_string(),
        })?;
    choice
        .message
        .content
        .ok_or_else(|| OpenCodeError::Protocol {
            reason: "首个 choice 的 content 不是字符串".to_string(),
        })
}

/// 把 `serde_json` 的错误压缩成“受控分类 + 行列号”。
///
/// 为什么不直接用 `err.to_string()`：serde 的类型错误会写成
/// `invalid type: string "<上游内容>", expected a sequence`，那属于不可信内容，
/// 不应被写到终端；对排查而言，分类 + 行列号才是我们真正需要的信息。
fn describe_json_error(err: &serde_json::Error) -> String {
    let category = match err.classify() {
        serde_json::error::Category::Io => "读取失败",
        serde_json::error::Category::Syntax => "JSON 语法错误",
        serde_json::error::Category::Data => "字段类型或结构不符",
        serde_json::error::Category::Eof => "内容提前结束",
    };
    format!("{category}，第 {} 行第 {} 列", err.line(), err.column())
}

/// 把 reqwest 错误分成超时与其它网络错误。
///
/// 只保留受控的事故分类，不拼接 `reqwest::Error` 的 Display：它会把完整请求 URL（甚至
/// 部分上游内容）写进错误文本，而我们会把错误文本直接写到终端。
fn classify_transport_error(err: &reqwest::Error, timeout: Duration) -> OpenCodeError {
    if err.is_timeout() {
        OpenCodeError::Timeout {
            timeout_millis: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        }
    } else {
        let reason = if err.is_connect() {
            "无法建立连接（网络不可达、DNS 失败或连接被拒绘）"
        } else if err.is_body() || err.is_decode() {
            "读取响应体失败"
        } else {
            "请求发送失败"
        };
        OpenCodeError::Http {
            reason: reason.to_string(),
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
///
/// `choices` 先按 JSON 值收下，而不是直接反序列化成 `Vec<ChatChoice>`：
/// 否则第二个 choice 畸形就会让整份响应解析失败，违背“只使用首个 choice”的规约。
#[derive(serde::Deserialize)]
struct ChatResponse {
    choices: Vec<serde_json::Value>,
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
    use super::{base_url_from_env, OpenCodeConfig, OpenCodeError, DEFAULT_BASE_URL};

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

    /// 合法的路径前缀必须保留：`<base-url>/openai/v1/chat/completions` 里的 Base URL
    /// 允许自带路径（例如内网网关 `https://gateway.example.com/opencode`）。
    #[test]
    fn accepts_base_url_with_path_prefix() {
        let config =
            OpenCodeConfig::new(TEST_KEY, "kimi-k3", Some("http://127.0.0.1:8080/gateway/"))
                .expect("带路径前缀的地址有效");
        assert_eq!(
            config.endpoint().as_str(),
            "http://127.0.0.1:8080/gateway/openai/v1/chat/completions",
            "路径前缀必须保留，只追加兼容路径"
        );
    }

    /// 「显式设成空白」与「根本没有设置」是两件事：前者是配置错误，不能静默回落到默认端点，
    /// 否则用户以为请求发往自己的网关，实际却发到了公网默认地址。
    #[test]
    fn rejects_explicitly_blank_base_url() {
        for blank in ["", "   ", "\t"] {
            let err = OpenCodeConfig::new(TEST_KEY, "kimi-k3", Some(blank))
                .expect_err("显式空白 Base URL 应被拒绝");
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

    /// 带查询串或片段的地址无法作为端点根：字符串拼接会得到
    /// `http://host/base?x=1/openai/v1/chat/completions`，路径其实不存在。
    #[test]
    fn rejects_base_url_with_query_or_fragment() {
        for raw in [
            "http://127.0.0.1:8080/gateway?token=abc",
            "http://127.0.0.1:8080/gateway#section",
        ] {
            let err = OpenCodeConfig::new(TEST_KEY, "kimi-k3", Some(raw))
                .expect_err("带查询串或片段的地址应被拒绝");
            match err {
                OpenCodeError::Config { message } => {
                    assert!(
                        message.contains("OPENCODE_BASE_URL"),
                        "错误应指明字段：{message}"
                    );
                    assert!(!message.contains(raw), "错误不得回显原值：{message}");
                }
                other => panic!("错误类型应为 Config：{other}"),
            }
        }
    }

    /// userinfo（`user:pass@`）会隐式带上另一套凭据，也不属于预期的端点根。
    #[test]
    fn rejects_base_url_with_userinfo() {
        for raw in [
            "http://user:pass@127.0.0.1:8080",
            "http://user@127.0.0.1:8080",
        ] {
            let err = OpenCodeConfig::new(TEST_KEY, "kimi-k3", Some(raw))
                .expect_err("带 userinfo 的地址应被拒绝");
            match err {
                OpenCodeError::Config { message } => {
                    assert!(
                        message.contains("OPENCODE_BASE_URL"),
                        "错误应指明字段：{message}"
                    );
                    assert!(
                        !message.contains("user:pass"),
                        "错误不得回显凭据：{message}"
                    );
                }
                other => panic!("错误类型应为 Config：{other}"),
            }
        }
    }

    /// 环境变量的值可能不是合法 UTF-8。这种情况必须报成配置错误，不能被当成
    /// 「没有设置」而静默使用默认端点。
    #[cfg(unix)]
    #[test]
    fn rejects_non_unicode_base_url() {
        use std::os::unix::ffi::OsStringExt;

        let raw = std::ffi::OsString::from_vec(b"http://127.0.0.1:8080/\xFF".to_vec());
        let err = base_url_from_env(Some(raw)).expect_err("非 Unicode 的 Base URL 应被拒绝");
        assert!(
            matches!(err, OpenCodeError::Config { .. }),
            "错误类型应为 Config：{err}"
        );
        assert!(
            err.to_string().contains("OPENCODE_BASE_URL"),
            "错误应指明字段：{err}"
        );
    }

    /// 「变量未设置」仍然合法（使用默认端点），且未设置与空值必须区分开。
    #[test]
    fn missing_base_url_env_falls_back_to_default() {
        assert!(base_url_from_env(None).expect("未设置是合法的").is_none());
        assert_eq!(
            base_url_from_env(Some("http://127.0.0.1:8080".into())).expect("合法 UTF-8"),
            Some("http://127.0.0.1:8080".to_string())
        );
    }

    /// Bearer 头的值必须在配置阶段就能构造出来：否则无效 Key（控制字符、换行）
    /// 会一直走到发送请求时才失败，降级成“AI 失败 + 回退列表”，退出码变成 0。
    /// 另外错误文本不得包含这个非法值本身。
    #[test]
    fn rejects_api_key_that_cannot_be_used_as_bearer_header() {
        for key in [
            "oc_sk_secret_for_test\r\nX-Injected: 1",
            "oc_sk_secret_for_test\nX-Injected: 1",
            "oc_sk_secret_for_test\u{0}",
            "oc_sk_secret_for_test\u{7f}",
        ] {
            let err = OpenCodeConfig::new(key, "kimi-k3", None)
                .expect_err("无法构成认证头的 Key 应被拒绝");
            match err {
                OpenCodeError::Config { message } => {
                    assert!(
                        message.contains("OPENCODE_API_KEY"),
                        "错误应指明字段：{message}"
                    );
                    assert!(
                        !message.contains("oc_sk_secret_for_test"),
                        "错误不得回显 Key 值：{message}"
                    );
                    assert!(
                        !message.contains("X-Injected"),
                        "错误不得回显注入内容：{message}"
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
