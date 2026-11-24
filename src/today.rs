//! "当天" 区间、过滤与排序。

use chrono::{DateTime, FixedOffset, Utc};

use crate::model::NewsItem;
use crate::parse::local_offset;

/// 半开区间 `[start, end)`，两端都是 UTC+8 的零点。
pub type DayRange = (DateTime<FixedOffset>, DateTime<FixedOffset>);

/// 以 `now` 所在的 UTC+8 自然日，计算当天 00:00:00 与次日 00:00:00。
pub fn day_range(now: DateTime<Utc>) -> DayRange {
    let offset = local_offset();
    let local_now = now.with_timezone(&offset);
    let start = local_now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("00:00:00 合法")
        .and_local_timezone(offset)
        .single()
        .expect("零点在当前偏移下唯一");
    let end = start + chrono::Days::new(1);
    (start, end)
}

/// 只保留发布时间落在 `range`（左闭右开）内的条目。
pub fn filter_today(items: impl IntoIterator<Item = NewsItem>, range: DayRange) -> Vec<NewsItem> {
    let (start, end) = range;
    items
        .into_iter()
        .filter(|item| item.published_at >= start && item.published_at < end)
        .collect()
}

/// 按发布时间从新到旧排序；时间相同则按来源、再按标题的字典序。
/// `sort_by` 是稳定排序：三个排序键都相同的条目保持输入顺序，不额外引入链接兜底键
/// （兜底键会改变相对顺序，违背规约要求的“稳定”排序）。
pub fn sort_items(items: &mut [NewsItem]) {
    items.sort_by(|a, b| {
        b.published_at
            .cmp(&a.published_at)
            .then_with(|| a.source.cmp(&b.source))
            .then_with(|| a.title.cmp(&b.title))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NewsItem;
    use chrono::{DateTime, FixedOffset, TimeZone, Utc};

    fn local(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> DateTime<FixedOffset> {
        FixedOffset::east_opt(8 * 3600)
            .expect("UTC+8")
            .with_ymd_and_hms(y, m, d, h, min, s)
            .single()
            .expect("时间有效")
    }

    fn item(title: &str, source: &str, published_at: DateTime<FixedOffset>) -> NewsItem {
        item_with_url(
            title,
            source,
            published_at,
            &format!("https://example.com/{title}"),
        )
    }

    fn item_with_url(
        title: &str,
        source: &str,
        published_at: DateTime<FixedOffset>,
        url: &str,
    ) -> NewsItem {
        NewsItem {
            title: title.to_string(),
            source: source.to_string(),
            url: url.to_string(),
            published_at,
            summary: None,
        }
    }

    #[test]
    fn day_range_starts_at_midnight_local() {
        let now = local(2026, 11, 24, 0, 0, 0).with_timezone(&Utc);
        let (start, end) = day_range(now);
        assert_eq!(start, local(2026, 11, 24, 0, 0, 0));
        assert_eq!(end, local(2026, 11, 25, 0, 0, 0));
    }

    #[test]
    fn day_range_at_last_second_of_day() {
        let now = local(2026, 11, 24, 23, 59, 59).with_timezone(&Utc);
        let (start, end) = day_range(now);
        assert_eq!(start, local(2026, 11, 24, 0, 0, 0));
        assert_eq!(end, local(2026, 11, 25, 0, 0, 0));
    }

    #[test]
    fn day_range_at_next_midnight() {
        let now = local(2026, 11, 25, 0, 0, 0).with_timezone(&Utc);
        let (start, end) = day_range(now);
        assert_eq!(start, local(2026, 11, 25, 0, 0, 0));
        assert_eq!(end, local(2026, 11, 26, 0, 0, 0));
    }

    #[test]
    fn day_range_uses_local_day_for_utc_evening() {
        // 2026-11-24T16:00:00Z 就是 UTC+8 的 2026-11-25 00:00。
        let now = Utc
            .with_ymd_and_hms(2026, 11, 24, 16, 0, 0)
            .single()
            .expect("时间有效");
        let (start, end) = day_range(now);
        assert_eq!(start, local(2026, 11, 25, 0, 0, 0));
        assert_eq!(end, local(2026, 11, 26, 0, 0, 0));
    }

    #[test]
    fn filter_keeps_today_items_only() {
        let range = day_range(local(2026, 11, 24, 12, 0, 0).with_timezone(&Utc));
        let items = vec![
            item("昨天末尾", "IT之家", local(2026, 11, 23, 23, 59, 0)),
            item("今天零点", "IT之家", local(2026, 11, 24, 0, 0, 0)),
            item("今天上午", "IT之家", local(2026, 11, 24, 9, 30, 0)),
            item("今天末尾", "IT之家", local(2026, 11, 24, 23, 59, 59)),
            item("次日零点", "IT之家", local(2026, 11, 25, 0, 0, 0)),
        ];
        let kept = filter_today(items, range);
        let titles: Vec<&str> = kept.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles, vec!["今天零点", "今天上午", "今天末尾"]);
    }

    #[test]
    fn filter_compares_across_time_zones() {
        let range = day_range(local(2026, 11, 24, 12, 0, 0).with_timezone(&Utc));
        // 2026-11-24T01:30:00Z = UTC+8 的 09:30，属于当天。
        let utc_item = item(
            "UTC 条目",
            "Hacker News",
            Utc.with_ymd_and_hms(2026, 11, 24, 1, 30, 0)
                .single()
                .expect("时间有效")
                .fixed_offset(),
        );
        let kept = filter_today(vec![utc_item], range);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn sorts_by_published_descending() {
        let mut items = vec![
            item("较早", "IT之家", local(2026, 11, 24, 8, 0, 0)),
            item("最新", "IT之家", local(2026, 11, 24, 12, 0, 0)),
            item("中间", "IT之家", local(2026, 11, 24, 10, 0, 0)),
        ];
        sort_items(&mut items);
        let titles: Vec<&str> = items.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(titles, vec!["最新", "中间", "较早"]);
    }

    #[test]
    fn identical_keys_keep_input_order() {
        // 三个排序键（发布时间、来源、标题）完全一致，只有链接不同。
        // 规约要求“稳定”排序，因此必须保持输入相对顺序，不额外引入链接兜底键。
        let time = local(2026, 11, 24, 12, 0, 0);
        let mut items = vec![
            item_with_url("同名", "IT之家", time, "https://example.com/z"),
            item_with_url("同名", "IT之家", time, "https://example.com/a"),
            item_with_url("同名", "IT之家", time, "https://example.com/b"),
        ];

        sort_items(&mut items);

        let urls: Vec<&str> = items.iter().map(|item| item.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://example.com/z",
                "https://example.com/a",
                "https://example.com/b"
            ],
            "三个排序键相同的条目必须保持输入相对顺序（稳定排序）"
        );
    }

    #[test]
    fn ties_sorted_by_source_then_title() {
        let time = local(2026, 11, 24, 12, 0, 0);
        let mut items = vec![
            item("b", "IT之家", time),
            item("a", "IT之家", time),
            item("z", "Hacker News", time),
            item("c", "Hacker News", time),
        ];
        sort_items(&mut items);
        let pairs: Vec<(&str, &str)> = items
            .iter()
            .map(|item| (item.source.as_str(), item.title.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("Hacker News", "c"),
                ("Hacker News", "z"),
                ("IT之家", "a"),
                ("IT之家", "b")
            ]
        );
    }
}
