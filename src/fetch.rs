//! HTTP 获取与多源并发。

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinSet;

use crate::error::SourceError;
use crate::model::NewsItem;
use crate::source::NewsSource;

/// 单个源的请求超时时间（秒）。
pub const FETCH_TIMEOUT_SECS: u64 = 10;

/// 单个源的请求超时时间。
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(FETCH_TIMEOUT_SECS);

/// 请求时使用的 User-Agent，避免部分站点拒绝默认 UA。
pub const USER_AGENT: &str = concat!("rust_net_news/", env!("CARGO_PKG_VERSION"));

/// 构建带 User-Agent 的 HTTP 客户端。
pub fn build_client() -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder().user_agent(USER_AGENT).build()
}

/// 获取单个源的条目。
pub async fn fetch_source(
    client: &reqwest::Client,
    source: &dyn NewsSource,
) -> Result<Vec<NewsItem>, SourceError> {
    fetch_source_with_timeout(client, source, FETCH_TIMEOUT).await
}

/// 以指定超时获取单个源的条目。
///
/// 超时参数可注入，测试里用较短时长，生产路径使用 [`FETCH_TIMEOUT`]。
pub async fn fetch_source_with_timeout(
    client: &reqwest::Client,
    source: &dyn NewsSource,
    timeout: Duration,
) -> Result<Vec<NewsItem>, SourceError> {
    let name = source.name();
    let response = client
        .get(source.url())
        .timeout(timeout)
        .send()
        .await
        .map_err(|err| classify_request_error(name, &err, timeout))?;

    let status = response.status();
    if !status.is_success() {
        return Err(SourceError::Status {
            source_name: name.to_string(),
            status: status.as_u16(),
        });
    }

    let body = response.bytes().await.map_err(|err| SourceError::Http {
        source_name: name.to_string(),
        reason: err.to_string(),
    })?;

    source.parse(&body)
}

fn classify_request_error(name: &str, err: &reqwest::Error, timeout: Duration) -> SourceError {
    if err.is_timeout() {
        SourceError::Timeout {
            source_name: name.to_string(),
            timeout_secs: timeout.as_secs(),
        }
    } else {
        SourceError::Http {
            source_name: name.to_string(),
            reason: err.to_string(),
        }
    }
}

/// 并发获取的结果：成功源的条目与失败源的错误。
#[derive(Debug, Default)]
pub struct FetchOutcome {
    /// 所有成功源的条目（未过滤、未排序）。
    pub items: Vec<NewsItem>,
    /// 每个失败源对应的错误。
    pub failures: Vec<SourceError>,
}

/// 全部源都失败。
#[derive(Debug)]
pub struct AllSourcesFailed {
    /// 每个源的失败原因。
    pub failures: Vec<SourceError>,
}

impl fmt::Display for AllSourcesFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "所有新闻源都失败")?;
        for failure in &self.failures {
            write!(f, "；{failure}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AllSourcesFailed {}

/// 并发获取所有源。
///
/// 单个源失败不影响其他源；只有全部源都失败时返回 [`AllSourcesFailed`]。
pub async fn fetch_all(
    client: &reqwest::Client,
    sources: &[Arc<dyn NewsSource>],
) -> Result<FetchOutcome, AllSourcesFailed> {
    let mut join_set: JoinSet<(usize, Result<Vec<NewsItem>, SourceError>)> = JoinSet::new();

    for (index, source) in sources.iter().enumerate() {
        let client = client.clone();
        let source = Arc::clone(source);
        join_set.spawn(async move { (index, fetch_source(&client, source.as_ref()).await) });
    }

    // 按源的下标收集结果，保证结果顺序与传入顺序一致。
    let mut results: Vec<Option<Result<Vec<NewsItem>, SourceError>>> =
        sources.iter().map(|_| None).collect();
    while let Some(joined) = join_set.join_next().await {
        match joined {
            Ok((index, result)) => results[index] = Some(result),
            Err(_) => {
                // 获取任务 panic：留空，下面按未完成处理，不影响其他源。
            }
        }
    }

    let mut outcome = FetchOutcome::default();
    for (index, result) in results.into_iter().enumerate() {
        match result {
            Some(Ok(items)) => outcome.items.extend(items),
            Some(Err(err)) => outcome.failures.push(err),
            None => outcome.failures.push(SourceError::Http {
                source_name: sources[index].name().to_string(),
                reason: "获取任务异常终止".to_string(),
            }),
        }
    }

    if outcome.failures.len() == sources.len() {
        return Err(AllSourcesFailed {
            failures: outcome.failures,
        });
    }

    Ok(outcome)
}
