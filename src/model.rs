//! 统一的新闻条目结构。

use chrono::{DateTime, FixedOffset};
use serde::{Deserialize, Serialize};

/// 统一的新闻条目。
///
/// 所有内置源都解析成这个结构，`published_at` 带时区偏移，
/// 序列化时按 RFC 3339 输出（例如 `2025-01-02T09:30:00+08:00`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsItem {
    /// 标题。
    pub title: String,
    /// 来源名称，例如 `IT之家`。
    pub source: String,
    /// 原文链接。
    pub url: String,
    /// 发布时间，带时区偏移。
    pub published_at: DateTime<FixedOffset>,
    /// 摘要（已去除 HTML 标签），没有摘要时为 `None`，JSON 中输出 `null`。
    pub summary: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::NewsItem;
    use chrono::{FixedOffset, TimeZone};

    fn sample() -> NewsItem {
        let offset = FixedOffset::east_opt(8 * 3600).expect("UTC+8 偏移有效");
        NewsItem {
            title: "标题".to_string(),
            source: "IT之家".to_string(),
            url: "https://www.ithome.com/0/1.htm".to_string(),
            published_at: offset
                .with_ymd_and_hms(2025, 1, 2, 9, 30, 0)
                .single()
                .expect("时间有效"),
            summary: Some("摘要".to_string()),
        }
    }

    #[test]
    fn json_uses_required_field_names() {
        let json = serde_json::to_value(sample()).expect("序列化成功");
        let object = json.as_object().expect("是 JSON 对象");
        let mut keys: Vec<&String> = object.keys().collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["published_at", "source", "summary", "title", "url"]
        );
        assert_eq!(json["title"], "标题");
        assert_eq!(json["source"], "IT之家");
        assert_eq!(json["url"], "https://www.ithome.com/0/1.htm");
    }

    #[test]
    fn published_at_is_rfc3339_with_offset() {
        let json = serde_json::to_value(sample()).expect("序列化成功");
        assert_eq!(json["published_at"], "2025-01-02T09:30:00+08:00");
    }

    #[test]
    fn summary_can_be_null_and_round_trips() {
        let mut item = sample();
        item.summary = None;
        let json = serde_json::to_value(&item).expect("序列化成功");
        assert!(json["summary"].is_null());

        let restored: NewsItem = serde_json::from_value(json).expect("反序列化成功");
        assert_eq!(restored, item);
    }
}
