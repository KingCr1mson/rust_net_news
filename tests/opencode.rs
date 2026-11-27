//! 用 wiremock 模拟 OpenCode：验证一次 Chat Completions 请求的路径、认证、请求体、
//! 响应提取与失败分类。全部请求都发给本地 mock，不访问真实服务、不读取真实凭据。

use std::time::Duration;

use rust_net_news::fetch::USER_AGENT;
use rust_net_news::opencode::{
    build_ai_client, request_brief, request_brief_with_timeout, OpenCodeConfig, OpenCodeError,
    OPENCODE_MAX_TOKENS,
};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

const TEST_KEY: &str = "oc_sk_secret_for_test";
const TEST_MODEL: &str = "kimi-k3";
const SYSTEM_PROMPT: &str = "你是新闻编辑。只依据输入新闻生成中文 JSON 简报。";
const USER_PROMPT: &str = r#"{"news":[{"id":1,"title":"标题 0"}]}"#;

/// 以 mock 服务地址构造配置：`new` 接受显式 Base URL，测试不需要改环境变量。
fn config_for(server: &MockServer) -> OpenCodeConfig {
    let base = server.uri();
    OpenCodeConfig::new(TEST_KEY, TEST_MODEL, Some(base.as_str())).expect("配置有效")
}

/// 一段 OpenAI Chat Completions 兼容的成功响应。
fn chat_completion_body(content: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "id": "chatcmpl_test",
        "object": "chat.completion",
        "model": TEST_MODEL,
        "choices": [
            {
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop"
            }
        ]
    }))
    .expect("响应可序列化")
}

fn json_response(body: Vec<u8>) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/json")
        .set_body_bytes(body)
}

/// 读取唯一一次请求的 JSON 请求体。
async fn single_request(server: &MockServer) -> Request {
    let requests = server.received_requests().await.expect("mock 记录了请求");
    assert_eq!(requests.len(), 1, "一次 brief 只允许一个 AI 请求");
    requests.into_iter().next().expect("已确认有一条请求")
}

#[tokio::test]
async fn sends_expected_chat_completion_request() {
    // 准备：只接受 POST + 固定路径 + Bearer 认证的请求。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/v1/chat/completions"))
        .and(header("authorization", format!("Bearer {TEST_KEY}")))
        .respond_with(json_response(chat_completion_body(&serde_json::json!(
            r#"{"overview":"概览","highlights":[]}"#
        ))))
        .mount(&server)
        .await;

    // 执行：真实调用一次请求函数。
    let client = build_ai_client().expect("客户端创建成功");
    let config = config_for(&server);
    request_brief(&client, &config, SYSTEM_PROMPT, USER_PROMPT)
        .await
        .expect("请求成功");

    // 断言：请求体字段严格符合协议，且只发了一次。
    let request = single_request(&server).await;
    assert_eq!(
        request.headers.get("authorization").map(|v| v.as_bytes()),
        Some(format!("Bearer {TEST_KEY}").as_bytes()),
        "认证头必须是 Bearer + API Key"
    );
    assert_eq!(
        request
            .headers
            .get("user-agent")
            .map(|value| value.to_str().expect("UA 可读")),
        Some(USER_AGENT),
        "应复用项目统一的 User-Agent"
    );

    let body: serde_json::Value = serde_json::from_slice(&request.body).expect("请求体是 JSON");
    assert_eq!(body["model"], TEST_MODEL);
    assert_eq!(body["max_tokens"], OPENCODE_MAX_TOKENS);
    assert_eq!(body["max_tokens"], 1200);
    assert_eq!(body["stream"], false);
    assert_eq!(body["messages"][0]["role"], "system");
    assert_eq!(body["messages"][0]["content"], SYSTEM_PROMPT);
    assert_eq!(body["messages"][1]["role"], "user");
    assert_eq!(body["messages"][1]["content"], USER_PROMPT);
}

#[tokio::test]
async fn returns_first_choice_content() {
    let server = MockServer::start().await;
    let content = r#"{"overview":"概览","highlights":[]}"#;
    Mock::given(method("POST"))
        .respond_with(json_response(chat_completion_body(&serde_json::json!(
            content
        ))))
        .mount(&server)
        .await;

    let client = build_ai_client().expect("客户端创建成功");
    let text = request_brief(&client, &config_for(&server), SYSTEM_PROMPT, USER_PROMPT)
        .await
        .expect("请求成功");

    assert_eq!(text, content, "应原样返回首个 choice 的 content");
}

#[tokio::test]
async fn rejects_empty_choices() {
    let server = MockServer::start().await;
    let body = serde_json::to_vec(&serde_json::json!({
        "id": "chatcmpl_test",
        "choices": []
    }))
    .expect("响应可序列化");
    Mock::given(method("POST"))
        .respond_with(json_response(body))
        .mount(&server)
        .await;

    let client = build_ai_client().expect("客户端创建成功");
    let err = request_brief(&client, &config_for(&server), SYSTEM_PROMPT, USER_PROMPT)
        .await
        .expect_err("没有 choice 应失败");

    assert!(
        matches!(err, OpenCodeError::Protocol { .. }),
        "错误类型应为 Protocol：{err}"
    );
}

#[tokio::test]
async fn rejects_non_string_content() {
    // content 分别是数字与 null：都不是字符串，属于协议错误。
    for content in [serde_json::json!(123), serde_json::Value::Null] {
        let server = MockServer::start().await;
        let body = serde_json::to_vec(&serde_json::json!({
            "choices": [{ "index": 0, "message": { "role": "assistant", "content": content } }]
        }))
        .expect("响应可序列化");
        Mock::given(method("POST"))
            .respond_with(json_response(body))
            .mount(&server)
            .await;

        let client = build_ai_client().expect("客户端创建成功");
        let err = request_brief(&client, &config_for(&server), SYSTEM_PROMPT, USER_PROMPT)
            .await
            .expect_err("content 不是字符串应失败");
        assert!(
            matches!(err, OpenCodeError::Protocol { .. }),
            "错误类型应为 Protocol：{err}"
        );
    }
}

#[tokio::test]
async fn reports_timeout_with_injected_short_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            json_response(chat_completion_body(&serde_json::json!("{}")))
                .set_delay(Duration::from_millis(800)),
        )
        .mount(&server)
        .await;

    // 注入 100ms，避免测试真的等 60 秒；生产入口固定使用 60 秒。
    let client = build_ai_client().expect("客户端创建成功");
    let err = request_brief_with_timeout(
        &client,
        &config_for(&server),
        SYSTEM_PROMPT,
        USER_PROMPT,
        Duration::from_millis(100),
    )
    .await
    .expect_err("应超时");

    match &err {
        OpenCodeError::Timeout { timeout_millis } => assert_eq!(*timeout_millis, 100),
        other => panic!("错误类型应为 Timeout：{other}"),
    }
    assert!(
        err.to_string().contains("超时"),
        "错误文本应说明超时：{err}"
    );
}

/// 规约的 60 秒总超时必须同时覆盖**读取响应体**：若只限制“拿到响应头”，网关先回
/// 2xx 再把 body 挂住就会无限等待。这里用手工 TCP 服务先发响应头与半截 body，
/// 断言客户端在注入的超时处失败，且不会重连重试。
#[tokio::test]
async fn reports_timeout_while_reading_body_after_2xx_headers() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // 准备：本地 loopback 监听，绝不访问外网。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("本地监听成功");
    let addr = listener.local_addr().expect("本地地址有效");
    let config = OpenCodeConfig::new(TEST_KEY, TEST_MODEL, Some(&format!("http://{addr}")))
        .expect("配置有效");
    let client = build_ai_client().expect("客户端创建成功");

    // 执行：客户端在独立任务里跑，下面由“服务端”手工控制响应节奏。
    let request = tokio::spawn(async move {
        request_brief_with_timeout(
            &client,
            &config,
            SYSTEM_PROMPT,
            USER_PROMPT,
            Duration::from_millis(200),
        )
        .await
    });

    let (mut socket, _) = listener.accept().await.expect("客户端应建立连接");
    // 先把请求读完（客户端在等响应；请求头以空行结束）。
    let mut buffer = [0u8; 1024];
    let mut received: Vec<u8> = Vec::new();
    while !received.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = socket.read(&mut buffer).await.expect("可读到请求");
        assert!(read > 0, "客户端请求应完整发出");
        received.extend_from_slice(&buffer[..read]);
    }

    // 只发响应头和半截 body：`content-length` 声明的剩余字节永远不来。
    socket
        .write_all(
            b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 1000\r\n\r\n{\"choices\":",
        )
        .await
        .expect("响应头可写出");
    socket.flush().await.expect("刷新成功");

    // 断言：已收到 2xx 头，超时发生在读 body 阶段。
    let err = request
        .await
        .expect("任务未被取消")
        .expect_err("读完响应体前应超时");
    match &err {
        OpenCodeError::Timeout { timeout_millis } => assert_eq!(*timeout_millis, 200),
        other => panic!("错误类型应为 Timeout：{other}"),
    }

    // 超时不得重连：再等一小段时间，不应出现第二个连接。
    assert!(
        tokio::time::timeout(Duration::from_millis(300), listener.accept())
            .await
            .is_err(),
        "body 读超时不得触发重试"
    );
}

#[tokio::test]
async fn reports_non_2xx_without_retry() {
    for status in [401u16, 429, 500] {
        // 每个状态用独立 mock 服务，便于断言“只请求一次”。
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string("upstream says no"))
            .mount(&server)
            .await;

        let client = build_ai_client().expect("客户端创建成功");
        let err = request_brief(&client, &config_for(&server), SYSTEM_PROMPT, USER_PROMPT)
            .await
            .expect_err("非 2xx 应失败");

        match &err {
            OpenCodeError::Status { status: actual } => assert_eq!(*actual, status),
            other => panic!("错误类型应为 Status：{other}"),
        }
        assert!(
            err.to_string().contains(&status.to_string()),
            "错误文本应包含状态码：{err}"
        );
        let requests = server.received_requests().await.expect("mock 记录了请求");
        assert_eq!(requests.len(), 1, "状态 {status} 不得自动重试");
    }
}

#[tokio::test]
async fn invalid_response_json_is_protocol_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(json_response(b"this is not json".to_vec()))
        .mount(&server)
        .await;

    let client = build_ai_client().expect("客户端创建成功");
    let err = request_brief(&client, &config_for(&server), SYSTEM_PROMPT, USER_PROMPT)
        .await
        .expect_err("非法 JSON 应失败");

    assert!(
        matches!(err, OpenCodeError::Protocol { .. }),
        "错误类型应为 Protocol：{err}"
    );
}

#[tokio::test]
async fn http_error_does_not_leak_api_key() {
    let server = MockServer::start().await;
    // 上游“回显”诊断信息（含 Key）：错误输出必须直接舍弃响应体。
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401)
                .insert_header("www-authenticate", "Bearer")
                .set_body_string(format!(
                    "invalid token: Bearer {TEST_KEY}; prompt={USER_PROMPT}"
                )),
        )
        .mount(&server)
        .await;

    let client = build_ai_client().expect("客户端创建成功");
    let config = config_for(&server);
    let err = request_brief(&client, &config, SYSTEM_PROMPT, USER_PROMPT)
        .await
        .expect_err("401 应失败");

    let text = err.to_string();
    assert!(!text.contains(TEST_KEY), "错误文本不得包含 API Key：{text}");
    assert!(!text.contains("Bearer"), "错误文本不得包含认证头：{text}");
    // 提示词内容同样不得随错误外泄（上游把 user 提示词回显在响应体里）。
    assert!(
        !text.contains("标题 0"),
        "错误文本不得包含提示词内容：{text}"
    );
    assert!(text.contains("401"), "错误文本应保留状态码：{text}");
}

/// Base URL 自带路径前缀时，兼容路径必须追加在前缀之后，而不是替换掉它。
#[tokio::test]
async fn sends_request_under_path_prefix_base_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/gateway/openai/v1/chat/completions"))
        .respond_with(json_response(chat_completion_body(&serde_json::json!(
            "{}"
        ))))
        .mount(&server)
        .await;

    let base = format!("{}/gateway", server.uri());
    let config = OpenCodeConfig::new(TEST_KEY, TEST_MODEL, Some(base.as_str())).expect("配置有效");
    let client = build_ai_client().expect("客户端创建成功");
    request_brief(&client, &config, SYSTEM_PROMPT, USER_PROMPT)
        .await
        .expect("带路径前缀的请求应成功");

    let request = single_request(&server).await;
    assert_eq!(request.url.path(), "/gateway/openai/v1/chat/completions");
}

/// 规约只使用首个 choice，因此它合法时，后面那些畸形 choice 不能把整份响应拖成协议错误。
#[tokio::test]
async fn first_valid_choice_wins_over_malformed_later_choices() {
    // 第一类：第二个 choice 的 content 是数字；第二类：第二个 choice 根本不是对象。
    let second_choices = [
        serde_json::json!({ "index": 1, "message": { "role": "assistant", "content": 42 } }),
        serde_json::json!("not a choice object"),
    ];
    for second in second_choices {
        let server = MockServer::start().await;
        let body = serde_json::to_vec(&serde_json::json!({
            "choices": [
                { "index": 0, "message": { "role": "assistant", "content": "{\"overview\":\"概览\"}" } },
                second
            ]
        }))
        .expect("响应可序列化");
        Mock::given(method("POST"))
            .respond_with(json_response(body))
            .mount(&server)
            .await;

        let client = build_ai_client().expect("客户端创建成功");
        let text = request_brief(&client, &config_for(&server), SYSTEM_PROMPT, USER_PROMPT)
            .await
            .expect("首个 choice 合法时不应因为后续 choice 失败");
        assert_eq!(text, r#"{"overview":"概览"}"#);
    }
}

/// 畸形 JSON 的错误文本只允许包含行/列与受控分类，不能把上游响应体里的内容（包括它可能
/// 回显的 Key）带到标准错误。
#[tokio::test]
async fn malformed_json_error_does_not_echo_upstream_content() {
    let bodies = [
        // 截断的 JSON，里面嵌了虚构 Key。
        format!(r#"{{"choices":[{{"message":{{"content":"{TEST_KEY}"}}}}]"#),
        // 类型不符：serde 的 Display 会把 "oc_sk_..." 原样写进错误文本。
        format!(r#"{{"choices": "{TEST_KEY}"}}"#),
    ];
    for body in bodies {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(json_response(body.into_bytes()))
            .mount(&server)
            .await;

        let client = build_ai_client().expect("客户端创建成功");
        let err = request_brief(&client, &config_for(&server), SYSTEM_PROMPT, USER_PROMPT)
            .await
            .expect_err("畸形 JSON 应失败");

        assert!(
            matches!(err, OpenCodeError::Protocol { .. }),
            "错误类型应为 Protocol：{err}"
        );
        let text = err.to_string();
        assert!(!text.contains(TEST_KEY), "错误文本不得回显上游内容：{text}");
        assert!(text.contains("行"), "错误文本应保留行列信息：{text}");
    }
}

/// 传输错误的文本不得包含请求 URL：错误会直接写到终端，URL 里可能含有内网主机名或
/// 网关凭据信息；只能用受控的中文分类描述发生了什么。
#[tokio::test]
async fn transport_error_does_not_echo_request_url() {
    // 127.0.0.1:1 上几乎不会有服务监听，连接会被拒绝，不会真的发出请求。
    let config = OpenCodeConfig::new(TEST_KEY, TEST_MODEL, Some("http://127.0.0.1:1/mock-base"))
        .expect("配置有效");
    let client = build_ai_client().expect("客户端创建成功");
    let err = request_brief_with_timeout(
        &client,
        &config,
        SYSTEM_PROMPT,
        USER_PROMPT,
        Duration::from_secs(2),
    )
    .await
    .expect_err("连接被拒绝应失败");

    assert!(
        matches!(err, OpenCodeError::Http { .. }),
        "错误类型应为 Http：{err}"
    );
    let text = err.to_string();
    assert!(
        !text.contains("127.0.0.1"),
        "错误文本不得回显请求 URL：{text}"
    );
    assert!(
        !text.contains("chat/completions"),
        "错误文本不得回显请求路径：{text}"
    );
    assert!(!text.contains(TEST_KEY), "错误文本不得包含 API Key：{text}");
    assert!(text.contains("连接"), "应给出受控的中文原因：{text}");
}

/// 一次性请求的额外保障：客户端不跟随重定向，302 直接按非 2xx 处理，
/// 否则可能产生第二次请求（以及第二次费用）。
#[tokio::test]
async fn does_not_follow_redirects() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("{}/elsewhere", server.uri())),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/elsewhere"))
        .respond_with(json_response(chat_completion_body(&serde_json::json!(
            "{}"
        ))))
        .mount(&server)
        .await;

    let client = build_ai_client().expect("客户端创建成功");
    let err = request_brief(&client, &config_for(&server), SYSTEM_PROMPT, USER_PROMPT)
        .await
        .expect_err("302 应视为非 2xx");

    match err {
        OpenCodeError::Status { status } => assert_eq!(status, 302),
        other => panic!("错误类型应为 Status：{other}"),
    }
    let requests = server.received_requests().await.expect("mock 记录了请求");
    assert_eq!(requests.len(), 1, "不得跟随重定向产生第二次请求");
    assert_eq!(
        requests[0].url.path(),
        "/openai/v1/chat/completions",
        "只应请求兼容路径本身"
    );
}
