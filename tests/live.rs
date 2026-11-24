//! 真实联网测试：默认忽略，用 `cargo test -- --ignored` 执行。
//!
//! 这些测试依赖 ithome.com 与 hnrss.org 的可用性，因此不进入常规测试集。
//! 断言只要求"请求成功且解析出条目"；当天是否有条目由过滤器决定，不在此断言。

use std::sync::Arc;

use rust_net_news::fetch::{build_client, fetch_all, fetch_source, FETCH_TIMEOUT};
use rust_net_news::source::{builtin_sources, HackerNewsSource, IthomeSource, NewsSource};
use rust_net_news::today::{day_range, filter_today};

#[tokio::test]
#[ignore = "需要访问真实新闻源 www.ithome.com"]
async fn live_ithome_returns_at_least_one_item() {
    let client = build_client().expect("客户端创建成功");
    let source = IthomeSource;
    let items = fetch_source(&client, &source)
        .await
        .expect("IT之家获取成功");

    assert!(!items.is_empty(), "IT之家应至少返回一个条目");
    assert!(items.iter().all(|item| item.source == "IT之家"));
    assert!(items
        .iter()
        .all(|item| !item.title.is_empty() && item.url.starts_with("http")));

    let today = filter_today(items.clone(), day_range(chrono::Utc::now()));
    println!(
        "IT之家：解析 {} 条，其中当天 {} 条（超时上限 {} 秒）",
        items.len(),
        today.len(),
        FETCH_TIMEOUT.as_secs()
    );
}

#[tokio::test]
#[ignore = "需要访问真实新闻源 hnrss.org"]
async fn live_hacker_news_returns_at_least_one_item() {
    let client = build_client().expect("客户端创建成功");
    let source = HackerNewsSource;
    let items = fetch_source(&client, &source)
        .await
        .expect("Hacker News 获取成功");

    assert!(!items.is_empty(), "Hacker News 应至少返回一个条目");
    assert!(items.iter().all(|item| item.source == "Hacker News"));

    let today = filter_today(items.clone(), day_range(chrono::Utc::now()));
    println!(
        "Hacker News：解析 {} 条，其中当天 {} 条",
        items.len(),
        today.len()
    );
}

#[tokio::test]
#[ignore = "需要访问真实新闻源 ithome.com 与 hnrss.org"]
async fn live_all_builtin_sources_succeed() {
    let client = build_client().expect("客户端创建成功");
    let sources: Vec<Arc<dyn NewsSource>> = builtin_sources();
    let outcome = fetch_all(&client, &sources).await.expect("所有源都应成功");

    assert!(
        outcome.failures.is_empty(),
        "不应有失败源，实际失败：{:?}",
        outcome.failures
    );
    assert!(!outcome.items.is_empty(), "合并结果应至少有一个条目");
    println!("两个源合计解析 {} 条", outcome.items.len());
}
