//! RSS / Atom 解析与摘要清理。

use chrono::{DateTime, FixedOffset, Utc};
use feed_rs::model::{Entry, FeedType};

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
/// 且不影响同一源中其他条目。发布时间只取 `published`（RSS 的 `pubDate`、
/// Atom 的 `published`），不使用 `updated` 兜底。
pub fn parse_feed(body: &[u8], source_name: &str) -> Result<Vec<NewsItem>, SourceError> {
    let feed = feed_rs::parser::parse(body).map_err(|err| SourceError::Parse {
        source_name: source_name.to_string(),
        reason: err.to_string(),
    })?;

    // 只接受 RSS（0.9x / 1.0 / 2.0）与 Atom；其它已识别格式（如 JSON Feed）
    // 不符合本系统的输入契终，属于源失败。
    if !matches!(
        feed.feed_type,
        FeedType::Atom | FeedType::RSS0 | FeedType::RSS1 | FeedType::RSS2
    ) {
        return Err(SourceError::Parse {
            source_name: source_name.to_string(),
            reason: format!(
                "不支持的 feed 格式 {:?}，只接受 RSS 或 Atom",
                feed.feed_type
            ),
        });
    }

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

    // 只认发布时间：`updated` / `atom:updated` 是最后修改时间，不能当作发布时间。
    // 缺少发布时间的条目无法判断是否属于当天，必须丢弃。
    let published = entry.published?;

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

    /// 只有 `atom:updated`、没有 `pubDate` 的 RSS 条目：无法判断发布时间，必须丢弃。
    const RSS_ONLY_UPDATED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0" xmlns:atom="http://www.w3.org/2005/Atom">
  <channel>
    <title>只有更新时间的 RSS 源</title>
    <link>https://example.com/</link>
    <description>测试用 RSS 源</description>
    <item>
      <title>只有更新时间：RSS</title>
      <link>https://example.com/only-updated-rss</link>
      <atom:updated>2026-11-24T04:58:53Z</atom:updated>
    </item>
  </channel>
</rss>"#;

    /// 只有 `<updated>`、没有 `<published>` 的 Atom 条目：同样必须丢弃。
    const ATOM_ONLY_UPDATED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>只有更新时间的 Atom 源</title>
  <id>https://example.com/atom</id>
  <updated>2026-11-24T04:58:53Z</updated>
  <entry>
    <title>只有更新时间：Atom</title>
    <id>https://example.com/only-updated-atom</id>
    <link href="https://example.com/only-updated-atom"/>
    <updated>2026-11-24T04:58:53Z</updated>
  </entry>
</feed>"#;

    /// 合法的最小 JSON Feed 1.1：feed-rs 能解析，但不是 RSS / Atom，必须视为源解析失败。
    const JSON_FEED: &str = r#"
{
  "version": "https://jsonfeed.org/version/1.1",
  "title": "JSON Feed 测试源",
  "home_page_url": "https://example.com/",
  "feed_url": "https://example.com/feed.json",
  "items": [
    {
      "id": "https://example.com/json-item-1",
      "url": "https://example.com/json-item-1",
      "title": "JSON Feed 条目",
      "content_text": "该条目来自 JSON Feed。",
      "date_published": "2026-11-24T04:58:53Z"
    }
  ]
}"#;

    /// 缺标题（无 `<title>` 或只有空白）的 RSS 条目：无法展示，必须丢弃。
    const RSS_MISSING_TITLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>缺标题测试源</title>
    <link>https://example.com/</link>
    <description>测试用 RSS 源</description>
    <item>
      <link>https://example.com/no-title</link>
      <pubDate>Tue, 24 Nov 2026 04:58:53 GMT</pubDate>
    </item>
    <item>
      <title>   </title>
      <link>https://example.com/blank-title</link>
      <pubDate>Tue, 24 Nov 2026 04:57:00 GMT</pubDate>
    </item>
    <item>
      <title>正常条目</title>
      <link>https://example.com/kept</link>
      <pubDate>Tue, 24 Nov 2026 04:56:00 GMT</pubDate>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn drops_rss_item_with_only_updated_time() {
        let items =
            parse_feed(RSS_ONLY_UPDATED.as_bytes(), "只有更新时间的 RSS 源").expect("解析成功");
        assert!(
            items.is_empty(),
            "只有更新时间、没有发布时间的 RSS 条目必须被丢弃，实际保留：{:?}",
            items.iter().map(|item| &item.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn drops_atom_entry_with_only_updated_time() {
        let items =
            parse_feed(ATOM_ONLY_UPDATED.as_bytes(), "只有更新时间的 Atom 源").expect("解析成功");
        assert!(
            items.is_empty(),
            "只有更新时间、没有发布时间的 Atom 条目必须被丢弃，实际保留：{:?}",
            items.iter().map(|item| &item.title).collect::<Vec<_>>()
        );
    }

    #[test]
    fn json_feed_is_parse_error() {
        let err = parse_feed(JSON_FEED.as_bytes(), "IT之家")
            .expect_err("JSON Feed 不是 RSS / Atom，应视为源解析失败");
        assert!(
            matches!(err, SourceError::Parse { .. }),
            "错误类型应为 Parse：{err}"
        );
        assert!(err.to_string().contains("IT之家"), "缺少源名称：{err}");
        assert!(err.to_string().contains("解析失败"), "缺少原因：{err}");
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
    fn drops_items_missing_title() {
        let items = parse_feed(RSS_MISSING_TITLE.as_bytes(), "缺标题测试源").expect("解析成功");
        let urls: Vec<&str> = items.iter().map(|item| item.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://example.com/kept"],
            "缺标题（含只有空白的标题）的条目应被丢弃，且不影响其他条目"
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
