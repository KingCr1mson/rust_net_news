//! 新闻源抽象与内置源。

use crate::error::SourceError;
use crate::model::NewsItem;
use crate::parse::parse_feed;

/// IT之家的名称。
pub const ITHOME_NAME: &str = "IT之家";
/// IT之家 RSS 地址。
pub const ITHOME_URL: &str = "https://www.ithome.com/rss/";
/// Hacker News（hnrss）的名称。
pub const HACKER_NEWS_NAME: &str = "Hacker News";
/// Hacker News（hnrss）地址。
pub const HACKER_NEWS_URL: &str = "https://hnrss.org/frontpage";

/// 一个新闻源：只需要提供名称、地址与解析方式。
///
/// 默认的 [`NewsSource::parse`] 走 feed-rs 的通用 RSS / Atom 解析，
/// 因此新增 RSS 源只需实现 `name()` 与 `url()`。
pub trait NewsSource: Send + Sync {
    /// 源名称，出现在输出结果与错误信息里。
    fn name(&self) -> &'static str;

    /// 源的 RSS / Atom 地址。
    fn url(&self) -> &'static str;

    /// 把响应体解析为统一条目。
    fn parse(&self, body: &[u8]) -> Result<Vec<NewsItem>, SourceError> {
        parse_feed(body, self.name())
    }
}

/// IT之家 RSS 源。
#[derive(Debug, Default, Clone, Copy)]
pub struct IthomeSource;

impl NewsSource for IthomeSource {
    fn name(&self) -> &'static str {
        ITHOME_NAME
    }

    fn url(&self) -> &'static str {
        ITHOME_URL
    }
}

/// Hacker News（hnrss）源。
#[derive(Debug, Default, Clone, Copy)]
pub struct HackerNewsSource;

impl NewsSource for HackerNewsSource {
    fn name(&self) -> &'static str {
        HACKER_NEWS_NAME
    }

    fn url(&self) -> &'static str {
        HACKER_NEWS_URL
    }
}

/// 内置的全部新闻源。
pub fn builtin_sources() -> Vec<Box<dyn NewsSource>> {
    vec![Box::new(IthomeSource), Box::new(HackerNewsSource)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ithome_source_metadata() {
        let source = IthomeSource;
        assert_eq!(source.name(), "IT之家");
        assert_eq!(source.url(), "https://www.ithome.com/rss/");
    }

    #[test]
    fn hacker_news_source_metadata() {
        let source = HackerNewsSource;
        assert_eq!(source.name(), "Hacker News");
        assert_eq!(source.url(), "https://hnrss.org/frontpage");
    }

    #[test]
    fn builtin_sources_are_usable_as_trait_objects() {
        let sources: Vec<Box<dyn NewsSource>> =
            vec![Box::new(IthomeSource), Box::new(HackerNewsSource)];
        let names: Vec<&str> = sources.iter().map(|s| s.name()).collect();
        assert_eq!(names, vec!["IT之家", "Hacker News"]);
    }

    #[test]
    fn ithome_parse_uses_default_implementation() {
        let body = include_bytes!("../tests/fixtures/ithome.xml");
        let items = IthomeSource.parse(body).expect("解析成功");
        assert_eq!(items.len(), 3, "缺链接与缺发布时间的构造条目应被丢弃");
        assert!(items.iter().all(|item| item.source == "IT之家"));
    }
}
