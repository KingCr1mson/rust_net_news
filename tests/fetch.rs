//! 用 wiremock 模拟 HTTP 的集成测试：单源获取、并发获取与失败汇总。

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rust_net_news::error::SourceError;
use rust_net_news::fetch::{build_client, fetch_all, fetch_source_with_timeout, FETCH_TIMEOUT};
use rust_net_news::source::{HackerNewsSource, IthomeSource, NewsSource};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ITHOME_XML: &[u8] = include_bytes!("fixtures/ithome.xml");
const HNRSS_XML: &[u8] = include_bytes!("fixtures/hnrss.xml");

struct MockSource {
    name: &'static str,
    url: String,
}

impl MockSource {
    fn new(name: &'static str, url: String) -> Self {
        Self { name, url }
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

#[tokio::test]
async fn fetch_single_source_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/feed"))
        .respond_with(rss_response(ITHOME_XML))
        .mount(&server)
        .await;

    let source = MockSource::new("IT之家", format!("{}/feed", server.uri()));
    let client = build_client().expect("客户端创建成功");
    let items = fetch_source_with_timeout(&client, &source, FETCH_TIMEOUT)
        .await
        .expect("获取成功");

    assert_eq!(items.len(), 3);
    assert!(items.iter().all(|item| item.source == "IT之家"));
}

#[tokio::test]
async fn fetch_reports_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(rss_response(ITHOME_XML).set_delay(Duration::from_millis(800)))
        .mount(&server)
        .await;

    let source = MockSource::new("IT之家", format!("{}/slow", server.uri()));
    let client = build_client().expect("客户端创建成功");
    let err = fetch_source_with_timeout(&client, &source, Duration::from_millis(100))
        .await
        .expect_err("应超时");

    assert!(
        matches!(err, SourceError::Timeout { .. }),
        "错误类型应为 Timeout：{err}"
    );
    assert!(err.to_string().contains("IT之家"));
    assert!(err.to_string().contains("超时"));
}

/// 起一个只写响应头与部分响应体、随后挂起的本地服务，用于造“读响应体超时”。
async fn spawn_stalled_body_server(hold: Duration) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("绑定成功");
    let addr = listener.local_addr().expect("本地地址有效");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("接受连接");
        // 读掉请求（本测试不关心内容）。
        let mut request = [0u8; 2048];
        let _ = socket.read(&mut request).await;
        // 声明 4 KiB 响应体，但只发一小段，然后挂着不断开。
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/rss+xml; charset=utf-8\r\nContent-Length: 4096\r\n\r\n<rss version=\"2.0\"><channel>",
            )
            .await
            .expect("写出响应头");
        socket.flush().await.expect("刷新 socket");
        tokio::time::sleep(hold).await;
    });
    addr
}

#[tokio::test]
async fn fetch_reports_timeout_while_reading_body() {
    let addr = spawn_stalled_body_server(Duration::from_secs(5)).await;
    let source = MockSource::new("IT之家", format!("http://{addr}/feed"));
    let client = build_client().expect("客户端创建成功");

    let err = fetch_source_with_timeout(&client, &source, Duration::from_millis(150))
        .await
        .expect_err("响应体停发应超时");

    assert!(
        matches!(err, SourceError::Timeout { .. }),
        "读响应体超时应归类为 Timeout：{err}"
    );
    assert!(err.to_string().contains("IT之家"), "缺少源名称：{err}");
}

#[tokio::test]
async fn fetch_reports_non_2xx_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/broken"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let source = MockSource::new("Hacker News", format!("{}/broken", server.uri()));
    let client = build_client().expect("客户端创建成功");
    let err = fetch_source_with_timeout(&client, &source, FETCH_TIMEOUT)
        .await
        .expect_err("应返回状态码错误");

    match err {
        SourceError::Status { status, .. } => assert_eq!(status, 503),
        other => panic!("错误类型应为 Status：{other}"),
    }
}

#[tokio::test]
async fn fetch_reports_parse_error_for_non_feed_body() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/html"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string("<html><body>oops</body></html>"),
        )
        .mount(&server)
        .await;

    let source = MockSource::new("IT之家", format!("{}/html", server.uri()));
    let client = build_client().expect("客户端创建成功");
    let err = fetch_source_with_timeout(&client, &source, FETCH_TIMEOUT)
        .await
        .expect_err("应返回解析错误");

    assert!(
        matches!(err, SourceError::Parse { .. }),
        "错误类型应为 Parse：{err}"
    );
}

#[tokio::test]
async fn fetch_all_keeps_successful_source_when_another_fails() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(rss_response(HNRSS_XML))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/fail"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let ok = MockSource::new("Hacker News", format!("{}/ok", server.uri()));
    let fail = MockSource::new("IT之家", format!("{}/fail", server.uri()));
    let sources: Vec<Arc<dyn NewsSource>> = vec![Arc::new(ok), Arc::new(fail)];
    let client = build_client().expect("客户端创建成功");

    let outcome = fetch_all(&client, &sources)
        .await
        .expect("部分失败仍是成功结果");

    assert_eq!(outcome.items.len(), 3);
    assert_eq!(outcome.failures.len(), 1);
    let failure = &outcome.failures[0];
    assert_eq!(failure.source_name(), "IT之家");
    assert!(
        failure.to_string().contains("500"),
        "应记录状态码：{failure}"
    );
}

#[tokio::test]
async fn fetch_all_reports_every_source_when_all_fail() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/fail-a"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/fail-b"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let a = MockSource::new("IT之家", format!("{}/fail-a", server.uri()));
    let b = MockSource::new("Hacker News", format!("{}/fail-b", server.uri()));
    let sources: Vec<Arc<dyn NewsSource>> = vec![Arc::new(a), Arc::new(b)];
    let client = build_client().expect("客户端创建成功");

    let err = fetch_all(&client, &sources)
        .await
        .expect_err("全部失败应返回错误");
    let text = err.to_string();
    assert!(text.contains("IT之家"), "缺少第一个源：{text}");
    assert!(text.contains("500"), "缺少第一个源的原因：{text}");
    assert!(text.contains("Hacker News"), "缺少第二个源：{text}");
    assert!(text.contains("404"), "缺少第二个源的原因：{text}");
}

#[tokio::test]
async fn builtin_sources_expose_expected_urls() {
    let ithome = IthomeSource;
    let hn = HackerNewsSource;
    assert!(ithome.url().starts_with("https://www.ithome.com/"));
    assert!(hn.url().starts_with("https://hnrss.org/"));
}
