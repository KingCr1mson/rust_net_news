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
