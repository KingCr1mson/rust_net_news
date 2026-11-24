//! 文本与 JSON 输出。

use std::io::{self, Write};

use crate::model::NewsItem;
use crate::parse::local_offset;

/// 没有当天条目时的文本提示。
pub const EMPTY_TEXT: &str = "当天没有新闻";

/// 文本输出中条目之间的分隔线。
pub const SEPARATOR: &str = "----";

/// 以人类可读的文本写入条目。
///
/// 每条包含标题、来源、发布时间（UTC+8，`YYYY-MM-DD HH:MM`）与链接，
/// 条目之间用 [`SEPARATOR`] 分隔。
pub fn render_text(items: &[NewsItem], out: &mut impl Write) -> io::Result<()> {
    if items.is_empty() {
        return writeln!(out, "{EMPTY_TEXT}");
    }

    let blocks: Vec<String> = items
        .iter()
        .map(|item| {
            let published_at = item.published_at.with_timezone(&local_offset());
            format!(
                "{}\n来源：{}\n发布：{}\n链接：{}",
                item.title,
                item.source,
                published_at.format("%Y-%m-%d %H:%M"),
                item.url
            )
        })
        .collect();

    writeln!(out, "{}", blocks.join(&format!("\n{SEPARATOR}\n")))
}

/// 以 JSON 数组写入条目。
pub fn render_json(items: &[NewsItem], out: &mut impl Write) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *out, items)?;
    writeln!(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewsItem;
    use chrono::{FixedOffset, TimeZone};

    fn at(y: i32, m: u32, d: u32, h: u32, min: u32) -> NewsItem {
        NewsItem {
            title: format!("标题 {h}:{min}"),
            source: "IT之家".to_string(),
            url: format!("https://example.com/{h}-{min}"),
            published_at: FixedOffset::east_opt(8 * 3600)
                .expect("UTC+8")
                .with_ymd_and_hms(y, m, d, h, min, 0)
                .single()
                .expect("时间有效"),
            summary: None,
        }
    }

    fn render_to_string(items: &[NewsItem]) -> String {
        let mut buffer: Vec<u8> = Vec::new();
        render_text(items, &mut buffer).expect("写入成功");
        String::from_utf8(buffer).expect("输出是 UTF-8")
    }

    #[test]
    fn text_output_is_empty_notice_without_items() {
        assert_eq!(render_to_string(&[]), "当天没有新闻\n");
    }

    #[test]
    fn text_output_contains_required_fields() {
        let items = vec![at(2026, 11, 24, 9, 30)];
        let text = render_to_string(&items);
        assert!(text.contains("标题 9:30"), "缺少标题：{text}");
        assert!(text.contains("IT之家"), "缺少来源：{text}");
        assert!(text.contains("2026-11-24 09:30"), "缺少发布时间：{text}");
        assert!(
            text.contains("https://example.com/9-30"),
            "缺少链接：{text}"
        );
    }

    #[test]
    fn text_output_separates_items() {
        let items = vec![at(2026, 11, 24, 9, 30), at(2026, 11, 24, 10, 0)];
        let text = render_to_string(&items);
        let blocks: Vec<&str> = text.split(SEPARATOR).collect();
        assert_eq!(blocks.len(), 2, "两条条目应被分隔：{text}");
        assert!(blocks[0].contains("标题 9:30"));
        assert!(blocks[1].contains("标题 10:0"));
    }

    #[test]
    fn text_output_formats_time_in_utc_plus_8() {
        let mut item = at(2026, 11, 24, 9, 30);
        // 同一时刻的 UTC 表示，渲染时仍应显示 UTC+8 的时间。
        item.published_at = item
            .published_at
            .with_timezone(&FixedOffset::east_opt(0).expect("UTC"));
        let text = render_to_string(&[item]);
        assert!(text.contains("2026-11-24 09:30"), "应按 UTC+8 显示：{text}");
    }

    #[test]
    fn json_output_is_empty_array_without_items() {
        let mut buffer: Vec<u8> = Vec::new();
        render_json(&[], &mut buffer).expect("写入成功");
        let text = String::from_utf8(buffer).expect("UTF-8");
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("合法 JSON");
        assert_eq!(parsed, serde_json::json!([]));
    }

    #[test]
    fn json_output_has_required_fields_and_null_summary() {
        let mut item = at(2026, 11, 24, 9, 30);
        item.summary = None;
        let mut buffer: Vec<u8> = Vec::new();
        render_json(&[item], &mut buffer).expect("写入成功");
        let parsed: serde_json::Value =
            serde_json::from_str(&String::from_utf8(buffer).expect("UTF-8")).expect("合法 JSON");
        let array = parsed.as_array().expect("是数组");
        assert_eq!(array.len(), 1);
        let object = array[0].as_object().expect("是对象");
        assert_eq!(object["published_at"], "2026-11-24T09:30:00+08:00");
        assert!(object["summary"].is_null());
        assert_eq!(object.len(), 5);
    }
}
