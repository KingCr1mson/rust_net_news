//! RSS / Atom 解析与摘要清理。

use chrono::{DateTime, FixedOffset, Utc};
use feed_rs::model::Entry;

use crate::error::SourceError;
use crate::model::NewsItem;

/// 摘要保留的最大字符数（按字符计，避免截断中文）。
pub const SUMMARY_MAX_CHARS: usize = 200;

/// 本地时区：UTC+8（Asia/Shanghai 全年无夏令时）。
pub fn local_offset() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+8 是合法偏移")
}

/// 把 RSS / Atom 内容解析为统一条目。
///
/// 丢弃规则：缺少标题、缺少链接、缺少发布时间的条目都会被丢弃，
/// 且不影响同一源中其他条目。
pub fn parse_feed(body: &[u8], source_name: &str) -> Result<Vec<NewsItem>, SourceError> {
    let feed = feed_rs::parser::parse(body).map_err(|err| SourceError::Parse {
        source_name: source_name.to_string(),
        reason: err.to_string(),
    })?;

    Ok(feed
        .entries
        .into_iter()
        .filter_map(|entry| convert_entry(entry, source_name))
        .collect())
}

fn convert_entry(entry: Entry, source_name: &str) -> Option<NewsItem> {
    let title = entry.title?.content.trim().to_string();
    if title.is_empty() {
        return None;
    }

    let url = entry
        .links
        .first()
        .map(|link| link.href.trim().to_string())
        .filter(|href| !href.is_empty())?;

    // Atom 条目只有 updated，RSS 条目的 updated 由 pubDate 复制而来，
    // 因此取两者中先出现的那个；都没有时该条目无法判断是否属于当天。
    let published = entry.published.or(entry.updated)?;

    let summary = entry
        .summary
        .map(|text| clean_summary(&text.content))
        .filter(|text| !text.is_empty());

    Some(NewsItem {
        title,
        source: source_name.to_string(),
        url,
        published_at: to_local(published),
        summary,
    })
}

fn to_local(time: DateTime<Utc>) -> DateTime<FixedOffset> {
    time.with_timezone(&local_offset())
}

/// 去除 HTML 标签、压缩空白，并按字符截断到 [`SUMMARY_MAX_CHARS`]。
pub fn clean_summary(raw: &str) -> String {
    let text = strip_tags(raw);
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&collapsed, SUMMARY_MAX_CHARS)
}

fn strip_tags(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_tag = false;
    for ch in raw.chars() {
        match ch {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                // 标签被移除后补一个空格，避免两段文字粘连。
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((byte_index, _)) => text[..byte_index].to_string(),
        None => text.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, FixedOffset};

    const ITHOME_XML: &[u8] = include_bytes!("../tests/fixtures/ithome.xml");
    const HNRSS_XML: &[u8] = include_bytes!("../tests/fixtures/hnrss.xml");

    fn offset() -> FixedOffset {
        FixedOffset::east_opt(8 * 3600).expect("UTC+8 偏移有效")
    }

    #[test]
    fn parses_rss_items_with_required_fields() {
        let items = parse_feed(ITHOME_XML, "IT之家").expect("解析成功");
        assert_eq!(items.len(), 3);
        let first = &items[0];
        assert_eq!(first.title, "索尼微软开放《GTA 6》云游戏：PlayStation Portal 无 PS5 也可串流，需 PS Plus 三档高级订阅");
        assert_eq!(first.source, "IT之家");
        assert_eq!(first.url, "https://www.ithome.com/1/031/466.htm");
        assert_eq!(
            first.published_at,
            DateTime::parse_from_rfc3339("2026-11-24T12:58:53+08:00").expect("时间有效")
        );
        assert_eq!(first.published_at.offset(), &offset());
    }

    #[test]
    fn drops_items_missing_link() {
        let items = parse_feed(ITHOME_XML, "IT之家").expect("解析成功");
        assert!(
            !items.iter().any(|item| item.title.contains("缺少链接")),
            "缺链接的条目应被丢弃"
        );
    }

    #[test]
    fn drops_items_missing_published_time() {
        let items = parse_feed(ITHOME_XML, "IT之家").expect("解析成功");
        assert!(
            !items.iter().any(|item| item.url.contains("no-time")),
            "缺发布时间的条目应被丢弃"
        );
    }

    #[test]
    fn parses_hnrss_fixture() {
        let items = parse_feed(HNRSS_XML, "Hacker News").expect("解析成功");
        assert_eq!(items.len(), 3);
        let first = &items[0];
        assert_eq!(first.title, "Drone Maps of Salvage Yards");
        assert_eq!(first.source, "Hacker News");
        assert_eq!(first.url, "https://salvageyarddrone.wordpress.com/");
        assert_eq!(
            first.published_at,
            DateTime::parse_from_rfc3339("2026-11-24T12:00:28+08:00").expect("时间有效")
        );
    }

    #[test]
    fn non_feed_content_is_parse_error() {
        let err = parse_feed(b"<html><body>not a feed</body></html>", "IT之家")
            .expect_err("应返回解析错误");
        assert!(
            matches!(err, SourceError::Parse { .. }),
            "错误类型应为 Parse：{err}"
        );
        assert!(err.to_string().contains("IT之家"));
    }

    #[test]
    fn empty_body_is_parse_error() {
        let err = parse_feed(b"", "Hacker News").expect_err("应返回解析错误");
        assert!(
            matches!(err, SourceError::Parse { .. }),
            "错误类型应为 Parse：{err}"
        );
    }

    #[test]
    fn clean_summary_removes_html_tags() {
        let cleaned = clean_summary("<p>你好 <b>世界</b></p>");
        assert_eq!(cleaned, "你好 世界");
    }

    #[test]
    fn clean_summary_collapses_whitespace() {
        let cleaned = clean_summary("<p>第一段</p>\n\n<p>第二段   </p>\t");
        assert_eq!(cleaned, "第一段 第二段");
    }

    #[test]
    fn clean_summary_truncates_by_characters() {
        let cleaned = clean_summary(&"新".repeat(300));
        assert_eq!(cleaned.chars().count(), 200);
        assert!(cleaned.chars().all(|c| c == '新'));
    }

    #[test]
    fn clean_summary_keeps_chinese_intact() {
        let cleaned = clean_summary(&format!("<p>{}</p>", "中文内容测试".repeat(40)));
        assert_eq!(cleaned.chars().count(), 200);
        assert!(!cleaned.contains('\u{FFFD}'), "不应出现替换字符");
        assert_eq!(cleaned.chars().count(), cleaned.len() / 3);
    }

    #[test]
    fn item_summary_is_cleaned() {
        let items = parse_feed(HNRSS_XML, "Hacker News").expect("解析成功");
        let summary = items[0].summary.as_deref().expect("有摘要");
        assert!(!summary.contains('<'), "摘要不应包含 HTML 标签：{summary}");
        assert!(summary.contains("Article URL: https://salvageyarddrone.wordpress.com/"));
    }
}
