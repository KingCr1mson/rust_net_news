//! 简报领域层：输入裁剪、提示词构建、AI 返回结构校验与本地渲染。
//!
//! 本模块在 `brief` 数据流中的位置：
//!
//! ```text
//! 当天条目（已按时间倒序）
//!   → 本模块裁到最新 N 条并为每条分配稳定本地 ID
//!   → 本模块构建 user 提示词 → opencode.rs 发请求
//!   → 本模块把模型返回的 JSON 校验成 Brief
//!   → 本模块用本地条目补回来源名称与链接并渲染文本
//! ```
//!
//! 主要职责（只做“领域结构与校验”，不发任何网络请求）：
//!
//! - 输入裁剪：只把 RSS 已有字段交给模型，不抓正文；超出上限时记录截断事实，
//!   供 `main.rs` 在 stderr 提示“仅使用最新 N 条”。
//! - 提示词：要求模型只依据输入生成中文 JSON，不输出 Markdown 围栏。
//! - 结构校验：概览、重点标题与摘要必须非空，重点数量受限，引用 ID 必须全部
//!   存在于输入条目；未知引用使整份 AI 结果无效（宁可回退，也不输出无法溯源的简报）。
//! - 渲染：模型只提供概览、重点文字和引用关系；每条重点的来源名称、原始标题与
//!   URL 一律由本地条目补齐，因此模型无法伪造链接。
//!
//! 输入与输出：输入是 `model::NewsItem` 的切片借用（不复制条目），输出是可以直接
//! 写进 `impl std::io::Write` 的文本；AI 返回的原始字符串只用于解析，不做二次转发。
//!
//! 错误边界：提示词构建与响应解析的失败属于本模块的 `BriefError`，交给 `main.rs`
//! 走“AI 简报生成失败 → 回退完整原始列表”的路径；渲染阶段的 I/O 失败按写出失败处理。
//! 本模块不区分“模型质量”问题，只区分“结构上是否可用”。
//!
//! 依赖方向约束：不得依赖 `reqwest`；与 `opencode` 之间只交换字符串。

use serde::Serialize;

use crate::model::NewsItem;

/// 一次简报最多发送给模型的条目数。
///
/// 上限同时约束提示词长度、费用与模型上下文占用；超出的条目仍保留在本地列表里，
/// 用于 AI 失败时的完整回退。
pub const MAX_INPUT_ITEMS: usize = 50;

/// 交给模型的 system 提示词。
///
/// 内容固定且不含任何运行时数据：它只声明“角色 + 输出格式约束”，
/// 具体新闻放在 user 提示词里，方便评审逐条核对发送内容。
pub const SYSTEM_PROMPT: &str =
    "你是新闻编辑。只依据输入新闻生成中文 JSON 简报，不输出 Markdown 围栏。";

/// 简报输入：已按发布时间倒序排列的当天条目，最多保留 [`MAX_INPUT_ITEMS`] 条。
///
/// 条目顺序就是稳定 ID 的来源：下标 `i` 的条目 ID 为 `i + 1`。
/// 只要输入顺序不变，同一份新闻的 ID 在“构造提示词 → 校验引用”之间保持一致，
/// 模型返回的 `source_ids` 才能可靠地映射回本地条目。
pub struct BriefInput {
    items: Vec<NewsItem>,
    total_items: usize,
}

impl BriefInput {
    /// 从排序后的当天条目构造输入。
    ///
    /// 参数是切片借用（`&[NewsItem]`）：本模块需要持有条目以便后续渲染，
    /// 但不应要求调用方交出所有权，因此一次性克隆前 `MAX_INPUT_ITEMS` 条。
    pub fn new(sorted_items: &[NewsItem]) -> Self {
        let items: Vec<NewsItem> = sorted_items.iter().take(MAX_INPUT_ITEMS).cloned().collect();
        Self {
            items,
            total_items: sorted_items.len(),
        }
    }

    /// 原始条目数是否超过发送上限，即本次简报是否被截断。
    pub fn is_truncated(&self) -> bool {
        self.total_items > self.items.len()
    }

    /// 构造发送给模型的 user 提示词（一个 JSON 字符串）。
    ///
    /// 返回值是 `Result` 而不是 `String`：序列化理论上可能失败，把失败显式交给上层，
    /// 由 `main.rs` 归入“AI 简报生成失败”的回退路径，而不是在库层 panic。
    pub fn build_user_prompt(&self) -> Result<String, BriefError> {
        let news: Vec<PromptNewsItem<'_>> = self
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| PromptNewsItem {
                // ID 由下标推导，不依赖模型或网络，因此可重复构造出同一份提示词。
                id: index + 1,
                title: &item.title,
                source: &item.source,
                url: &item.url,
                published_at: &item.published_at,
                summary: item.summary.as_deref(),
            })
            .collect();

        let payload = PromptPayload {
            task: "依据 news 生成一份中文简报",
            rules: PROMPT_RULES,
            output_format: prompt_output_format(),
            truncated: self.is_truncated(),
            total_items: self.total_items,
            included_items: news.len(),
            news,
        };

        serde_json::to_string(&payload).map_err(|err| BriefError::Prompt {
            // 不拼接 serde 的 Display：序列化错误也可能带上字段值与类型细节。
            reason: format!(
                "提示词序列化失败（{}）",
                match err.classify() {
                    serde_json::error::Category::Io => "写入失败",
                    serde_json::error::Category::Syntax => "JSON 语法错误",
                    serde_json::error::Category::Data => "字段类型或结构不符",
                    serde_json::error::Category::Eof => "内容提前结束",
                }
            ),
        })
    }
}

/// 提示词中固定的规则文本。
const PROMPT_RULES: [&str; 6] = [
    "只使用 news 数组中的信息，不要引入外部知识或猜测",
    "news 只提供 title、source、url、published_at、summary，不包含正文，不要索要正文",
    "只输出一个 JSON 对象，不要输出 Markdown 围栏，也不要输出额外解释文字",
    "overview 必须是当天整体概览，非空字符串",
    "highlights 必须有 1 到 5 项，每项包含非空 title、非空 summary 和非空 source_ids",
    "source_ids 只能使用 news 中已存在的 id，并按重点实际来源填写",
];

/// 提示词里 `output_format` 字段的结构，给模型一个可照抄的骨架。
fn prompt_output_format() -> serde_json::Value {
    serde_json::json!({
        "overview": "当天整体概览",
        "highlights": [
            {
                "title": "重点主题",
                "summary": "中文说明",
                "source_ids": [1],
            }
        ],
    })
}

/// 发给模型的一条新闻。
///
/// 字段集合就是规约允许外发的全部 RSS 信息（`id` 由本地生成）；这里不添加正文或
/// 任何额外字段，序列化后的键必须与规约逐字一致。借用 `&str` 避免为每条新闻复制字符串。
#[derive(Serialize)]
struct PromptNewsItem<'a> {
    id: usize,
    title: &'a str,
    source: &'a str,
    url: &'a str,
    published_at: &'a chrono::DateTime<chrono::FixedOffset>,
    summary: Option<&'a str>,
}

/// user 提示词的顶层结构。serde 的 `Serialize` 会把字段名直接变成 JSON 键名，
/// 因此字段名即协议，重命名字段等于改变提示词格式。
#[derive(Serialize)]
struct PromptPayload<'a> {
    task: &'a str,
    rules: [&'a str; 6],
    output_format: serde_json::Value,
    truncated: bool,
    total_items: usize,
    included_items: usize,
    news: Vec<PromptNewsItem<'a>>,
}

/// 简报层的错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BriefError {
    /// 提示词无法序列化。实践上几乎不会发生，但仍按 `Result` 传播。
    #[error("简报输入序列化失败：{reason}")]
    Prompt {
        /// 底层序列化错误描述。
        reason: String,
    },
    /// AI 返回内容不符合简报结构。整份结果作废，由上层回退原始列表。
    #[error("AI 返回的简报无效：{reason}")]
    InvalidResponse {
        /// 具体原因，例如空白概览、重点数量越界或未知引用 ID。
        reason: String,
    },
}

/// 一份简报最多允许的重点数量。
pub const MAX_HIGHLIGHTS: usize = 5;

/// 校验通过的简报。
///
/// 字段私有：简报只能由 [`BriefInput::parse_response`] 生成，外部无法绕过校验
/// 直接构造一份“看起来合法”的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Brief {
    overview: String,
    highlights: Vec<BriefHighlight>,
}

/// 一个重点：模型只提供文字和引用关系，来源信息渲染时再从本地条目补齐。
#[derive(Debug, Clone, PartialEq, Eq)]
struct BriefHighlight {
    title: String,
    summary: String,
    /// 已校验且已按首次出现顺序去重的本地 ID（从 1 开始）。
    source_ids: Vec<usize>,
}

impl BriefInput {
    /// 校验模型返回的 `message.content` 并生成 [`Brief`]。
    ///
    /// 只接受“无 Markdown 围栏的纯 JSON”：模型若加了围栏，说明它没有遵守提示词，
    /// 这里不做兜底剥离，直接判为无效并回退原始列表（宁可回退，也不猜模型的意图）。
    ///
    /// 任何一处不合法都让整份结果失效（`Err`），而不是丢掉单个重点：
    /// 半份简报会被读者误以为完整，比明确的回退更糟。
    pub fn parse_response(&self, content: &str) -> Result<Brief, BriefError> {
        let raw: RawBrief = serde_json::from_str(content).map_err(|err| {
            // 只用行列号与受控分类：serde 的类型错误会把模型给的非法字段值原样回显。
            BriefError::invalid(format!(
                "不是合法的简报 JSON（{}）",
                describe_json_error(&err)
            ))
        })?;

        // serde 只保证“字段存在且类型正确”，语义约束必须逐项检查。
        let overview = raw.overview.trim();
        if overview.is_empty() {
            return Err(BriefError::invalid("overview 为空白"));
        }
        if raw.highlights.is_empty() {
            return Err(BriefError::invalid("highlights 少于 1 项"));
        }
        if raw.highlights.len() > MAX_HIGHLIGHTS {
            return Err(BriefError::invalid(format!(
                "highlights 多于 {MAX_HIGHLIGHTS} 项（实际 {} 项）",
                raw.highlights.len()
            )));
        }

        let mut highlights = Vec::with_capacity(raw.highlights.len());
        for (index, raw_highlight) in raw.highlights.into_iter().enumerate() {
            let position = index + 1;
            let title = raw_highlight.title.trim();
            if title.is_empty() {
                return Err(BriefError::invalid(format!(
                    "第 {position} 个重点的 title 为空白"
                )));
            }
            let summary = raw_highlight.summary.trim();
            if summary.is_empty() {
                return Err(BriefError::invalid(format!(
                    "第 {position} 个重点的 summary 为空白"
                )));
            }
            if raw_highlight.source_ids.is_empty() {
                return Err(BriefError::invalid(format!(
                    "第 {position} 个重点没有 source_ids"
                )));
            }

            let mut source_ids: Vec<usize> = Vec::with_capacity(raw_highlight.source_ids.len());
            for id in raw_highlight.source_ids {
                // 未知引用意味着模型编造了来源：整份结果不可溯源，直接失效。
                if id == 0 || id > self.items.len() {
                    return Err(BriefError::invalid(format!(
                        "第 {position} 个重点引用了不存在的 ID {id}（有效范围 1..={}）",
                        self.items.len()
                    )));
                }
                // 同一重点内重复引用只保留一次，且保持首次出现的顺序。
                if !source_ids.contains(&id) {
                    source_ids.push(id);
                }
            }

            highlights.push(BriefHighlight {
                title: title.to_string(),
                summary: summary.to_string(),
                source_ids,
            });
        }

        Ok(Brief {
            overview: overview.to_string(),
            highlights,
        })
    }

    /// 把简报写成中文文本。
    ///
    /// 每个重点下的来源名称、原始标题和链接都从本地条目查表取得：
    /// 模型返回里没有 URL 字段，因此链接不可能被模型改写。
    ///
    /// 返回 `io::Result`：写入失败（例如标准输出被关闭）由 `main.rs` 转成退出码 1。
    pub fn render(&self, brief: &Brief, out: &mut impl std::io::Write) -> std::io::Result<()> {
        writeln!(out, "AI 简报")?;
        writeln!(out, "概览：{}", brief.overview)?;

        for (index, highlight) in brief.highlights.iter().enumerate() {
            writeln!(out)?;
            writeln!(out, "重点 {}：{}", index + 1, highlight.title)?;
            writeln!(out, "{}", highlight.summary)?;
            writeln!(out, "来源：")?;
            for id in &highlight.source_ids {
                // ID 已在解析阶段校验过；这里仍用 `get` 而不是下标访问，
                // 保证即使未来有人在别处构造 `Brief` 也不会 panic。
                if let Some(item) = id.checked_sub(1).and_then(|index| self.items.get(index)) {
                    writeln!(out, "- {}：{}", item.source, item.title)?;
                    writeln!(out, "  链接：{}", item.url)?;
                }
            }
        }

        Ok(())
    }
}

/// 模型返回 JSON 的映射结构。
///
/// 字段全部必填：缺字段会让 serde 直接报错，正好等价于“结构无效”。
/// 模型多给的字段（例如妄想出来的 `url`）会被忽略，渲染时也不会被读取。
#[derive(serde::Deserialize)]
struct RawBrief {
    overview: String,
    highlights: Vec<RawHighlight>,
}

/// 模型返回的单个重点。
#[derive(serde::Deserialize)]
struct RawHighlight {
    title: String,
    summary: String,
    source_ids: Vec<usize>,
}

impl BriefError {
    /// 构造“AI 返回内容不可用”的错误，统一 `InvalidResponse` 的写法。
    fn invalid(reason: impl Into<String>) -> Self {
        Self::InvalidResponse {
            reason: reason.into(),
        }
    }
}

/// 把 `serde_json` 的错误压缩成“受控分类 + 行列号”。
///
/// 不用 `err.to_string()`：serde 会写成 `invalid type: string "<上游内容>"`，
/// 而那串内容来自模型或中间代理，属于不可信输入，不应进终端。
fn describe_json_error(err: &serde_json::Error) -> String {
    let category = match err.classify() {
        serde_json::error::Category::Io => "读取失败",
        serde_json::error::Category::Syntax => "JSON 语法错误",
        serde_json::error::Category::Data => "字段类型或结构不符",
        serde_json::error::Category::Eof => "内容提前结束",
    };
    format!("{category}，第 {} 行第 {} 列", err.line(), err.column())
}

#[cfg(test)]
mod tests {
    use super::{BriefInput, SYSTEM_PROMPT};
    use crate::model::NewsItem;
    use chrono::{Duration, FixedOffset, TimeZone};

    const TEST_KEY: &str = "oc_sk_secret_for_test";

    fn offset() -> FixedOffset {
        FixedOffset::east_opt(8 * 3600).expect("UTC+8 偏移有效")
    }

    /// 构造 `count` 条已按发布时间倒序排列的当天条目：
    /// 下标 0 最新，下标越大越旧，因此“最新 N 条”就是前 N 项。
    fn sorted_items(count: usize) -> Vec<NewsItem> {
        let newest = offset()
            .with_ymd_and_hms(2025, 1, 2, 23, 0, 0)
            .single()
            .expect("时间有效");
        (0..count)
            .map(|index| NewsItem {
                title: format!("标题 {index}"),
                source: "IT之家".to_string(),
                url: format!("https://example.com/{index}"),
                published_at: newest - Duration::minutes(index as i64),
                summary: Some(format!("摘要 {index}")),
            })
            .collect()
    }

    fn prompt_json(input: &BriefInput) -> serde_json::Value {
        let prompt = input.build_user_prompt().expect("提示词可序列化");
        serde_json::from_str(&prompt).expect("输入是 JSON")
    }

    #[test]
    fn input_ids_follow_sorted_order() {
        let items = sorted_items(3);
        let input = BriefInput::new(&items);
        let value = prompt_json(&input);
        let news = value["news"].as_array().expect("news 是数组");
        assert_eq!(news.len(), 3);
        for (index, item) in news.iter().enumerate() {
            assert_eq!(item["id"], index + 1, "ID 应按排序顺序从 1 连续递增");
            assert_eq!(item["title"], format!("标题 {index}"));
        }
    }

    #[test]
    fn input_limits_to_latest_50_items() {
        let items = sorted_items(51);
        let input = BriefInput::new(&items);
        assert!(input.is_truncated(), "51 条应标记为已截断");

        let prompt = input.build_user_prompt().expect("提示词可序列化");
        let value: serde_json::Value = serde_json::from_str(&prompt).expect("输入是 JSON");
        let news = value["news"].as_array().expect("news 是数组");
        assert_eq!(news.len(), 50, "最多发送 50 条");
        assert_eq!(news[0]["id"], 1);
        assert_eq!(news[49]["id"], 50);
        assert_eq!(news[49]["title"], "标题 49", "应取排序后最新的 50 条");
        assert!(
            news.iter().all(|item| item.get("body").is_none()),
            "AI 输入不得包含正文"
        );
        assert!(!prompt.contains(TEST_KEY), "提示词不得包含 API Key");
    }

    #[test]
    fn input_marks_truncation_only_above_50() {
        let exact = BriefInput::new(&sorted_items(50));
        assert!(!exact.is_truncated(), "正好 50 条不算截断");
        assert_eq!(
            exact
                .build_user_prompt()
                .expect("提示词可序列化")
                .matches("标题")
                .count(),
            50
        );

        let fewer = BriefInput::new(&sorted_items(2));
        assert!(!fewer.is_truncated(), "少于 50 条不算截断");

        let empty = BriefInput::new(&[]);
        assert!(!empty.is_truncated(), "没有条目不算截断");
    }

    #[test]
    fn prompt_contains_only_allowed_news_fields() {
        let input = BriefInput::new(&sorted_items(2));
        let value = prompt_json(&input);
        let news = value["news"].as_array().expect("news 是数组");
        for item in news {
            let object = item.as_object().expect("每条新闻是 JSON 对象");
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            assert_eq!(
                keys,
                vec!["id", "published_at", "source", "summary", "title", "url"],
                "发送给模型的字段必须与规约一致"
            );
        }
    }

    #[test]
    fn prompt_does_not_contain_api_key() {
        // 简报输入只由本地条目构成，没有任何配置字段，因此不存在密钥泄漏路径；
        // 这里固定住该事实，避免日后“顺手”把配置塞进提示词。
        let input = BriefInput::new(&sorted_items(3));
        let prompt = input.build_user_prompt().expect("提示词可序列化");
        assert!(!prompt.contains(TEST_KEY));
        assert!(!prompt.contains("OPENCODE_API_KEY"));
        assert!(!SYSTEM_PROMPT.contains(TEST_KEY));
    }

    #[test]
    fn prompt_requires_chinese_json_without_markdown() {
        let input = BriefInput::new(&sorted_items(1));
        let prompt = input.build_user_prompt().expect("提示词可序列化");
        assert!(prompt.contains("中文"), "提示词应要求中文输出");
        assert!(prompt.contains("JSON"), "提示词应要求 JSON 结构");
        assert!(prompt.contains("overview"));
        assert!(prompt.contains("highlights"));
        assert!(prompt.contains("source_ids"));
        assert!(prompt.contains("Markdown"), "提示词应禁止 Markdown 围栏");
        assert!(prompt.contains("正文"), "提示词应声明输入不含正文");

        assert!(SYSTEM_PROMPT.contains("中文"));
        assert!(SYSTEM_PROMPT.contains("JSON"));
        assert!(SYSTEM_PROMPT.contains("Markdown"));
    }

    use super::BriefError;

    /// 第 4 组测试用的单条条目：标题/摘要/链接都带序号，便于断言“渲染的是哪一条”。
    fn item(index: usize) -> NewsItem {
        NewsItem {
            title: format!("原始标题 {index}"),
            source: "IT之家".to_string(),
            url: format!("https://example.com/{index}"),
            published_at: offset()
                .with_ymd_and_hms(2025, 1, 2, 12, 0, 0)
                .single()
                .expect("时间有效"),
            summary: Some(format!("原始摘要 {index}")),
        }
    }

    fn valid_brief_json() -> String {
        r#"{
            "overview": "当天科技新闻集中在开源模型与芯片",
            "highlights": [
                {
                    "title": "开源模型进展",
                    "summary": "多个团队发布了新模型",
                    "source_ids": [1]
                }
            ]
        }"#
        .to_string()
    }

    /// 解析并渲染成文本，保留 `Result` 以便成功/失败两类测试共用同一入口。
    fn parse_and_render(input: &BriefInput, content: &str) -> Result<String, BriefError> {
        let brief = input.parse_response(content)?;
        let mut out: Vec<u8> = Vec::new();
        input
            .render(&brief, &mut out)
            .expect("写入内存缓冲不应失败");
        Ok(String::from_utf8(out).expect("输出是 UTF-8"))
    }

    #[test]
    fn parses_and_renders_valid_brief() {
        let items = vec![item(0)];
        let input = BriefInput::new(&items);
        let text = parse_and_render(&input, &valid_brief_json()).expect("简报有效");

        assert!(
            text.contains("当天科技新闻集中在开源模型与芯片"),
            "缺少概览：{text}"
        );
        assert!(text.contains("开源模型进展"), "缺少重点标题：{text}");
        assert!(
            text.contains("多个团队发布了新模型"),
            "缺少重点摘要：{text}"
        );
        // 来源名称、原标题与链接必须来自本地条目，模型只提供引用关系。
        assert!(text.contains("IT之家"), "缺少本地来源名称：{text}");
        assert!(text.contains("原始标题 0"), "缺少本地原始标题：{text}");
        assert!(
            text.contains("https://example.com/0"),
            "缺少本地链接：{text}"
        );
    }

    #[test]
    fn duplicate_source_ids_render_once() {
        let items = vec![item(0), item(1)];
        let input = BriefInput::new(&items);
        let content = r#"{
            "overview": "概览",
            "highlights": [
                {"title": "重点", "summary": "说明", "source_ids": [2, 2, 2]}
            ]
        }"#;
        let text = parse_and_render(&input, content).expect("重复引用仍然有效");

        assert_eq!(
            text.matches("https://example.com/1").count(),
            1,
            "同一重点内重复引用只渲染一次：{text}"
        );
        assert_eq!(
            text.matches("链接：").count(),
            1,
            "只应渲染一个来源条目：{text}"
        );
        assert!(
            !text.contains("https://example.com/0"),
            "未引用的条目不渲染：{text}"
        );
    }

    #[test]
    fn unknown_source_id_rejects_whole_brief() {
        let items = vec![item(0)];
        let input = BriefInput::new(&items);
        let content = r#"{
            "overview": "概览",
            "highlights": [
                {"title": "重点一", "summary": "说明一", "source_ids": [1]},
                {"title": "重点二", "summary": "说明二", "source_ids": [7]}
            ]
        }"#;

        let err = input
            .parse_response(content)
            .expect_err("未知引用应使整份简报无效，而不是只跳过该重点");
        match err {
            BriefError::InvalidResponse { reason } => {
                assert!(reason.contains('7'), "错误应指出未知 ID：{reason}");
            }
            other => panic!("错误类型应为 InvalidResponse：{other}"),
        }
    }

    #[test]
    fn empty_overview_is_invalid() {
        let input = BriefInput::new(&[item(0)]);
        for overview in ["", "   ", "\n\t"] {
            let content = format!(
                r#"{{"overview": "{overview}", "highlights": [{{"title": "重点", "summary": "说明", "source_ids": [1]}}]}}"#
            );
            assert!(
                input.parse_response(&content).is_err(),
                "空白概览应无效：{content}"
            );
        }
    }

    #[test]
    fn empty_highlight_text_is_invalid() {
        let input = BriefInput::new(&[item(0)]);
        for (title, summary) in [("", "说明"), ("   ", "说明"), ("重点", ""), ("重点", "  ")]
        {
            let content = format!(
                r#"{{"overview": "概览", "highlights": [{{"title": "{title}", "summary": "{summary}", "source_ids": [1]}}]}}"#
            );
            assert!(
                input.parse_response(&content).is_err(),
                "空白重点文本应无效：{content}"
            );
        }
    }

    #[test]
    fn zero_highlights_is_invalid() {
        let input = BriefInput::new(&[item(0)]);
        let content = r#"{"overview": "概览", "highlights": []}"#;
        assert!(input.parse_response(content).is_err(), "0 个重点应无效");
    }

    #[test]
    fn six_highlights_is_invalid() {
        let input = BriefInput::new(&[item(0)]);
        let highlights: Vec<String> = (1..=6)
            .map(|index| {
                format!(r#"{{"title": "重点 {index}", "summary": "说明", "source_ids": [1]}}"#)
            })
            .collect();
        let content = format!(
            r#"{{"overview": "概览", "highlights": [{}]}}"#,
            highlights.join(",")
        );
        assert!(input.parse_response(&content).is_err(), "6 个重点应无效");
    }

    #[test]
    fn empty_source_ids_is_invalid() {
        let input = BriefInput::new(&[item(0)]);
        let content = r#"{"overview": "概览", "highlights": [{"title": "重点", "summary": "说明", "source_ids": []}]}"#;
        assert!(input.parse_response(content).is_err(), "没有引用应无效");
    }

    #[test]
    fn markdown_fenced_json_is_invalid() {
        let input = BriefInput::new(&[item(0)]);
        let content = format!("```json\n{}\n```", valid_brief_json());
        assert!(
            input.parse_response(&content).is_err(),
            "带 Markdown 围栏的内容不视为有效简报"
        );
    }

    #[test]
    fn non_json_content_is_invalid() {
        let input = BriefInput::new(&[item(0)]);
        for content in ["抱歉，我无法完成这个请求。", "", "{}"] {
            assert!(
                input.parse_response(content).is_err(),
                "非 JSON 或缺少必填字段应无效：{content}"
            );
        }
    }

    /// 解析失败的错误文本只允许包含行/列与受控分类：serde 默认的 Display 会把
    /// “非预期的字段值”原样写进去，而那是上游（模型或中间代理）给出的不可信内容。
    #[test]
    fn invalid_json_error_does_not_echo_upstream_content() {
        let input = BriefInput::new(&[item(0)]);
        let contents = [
            // highlights 应该是数组，这里故意给字符串：类型错误的 Display 会回显它。
            format!(r#"{{"overview": "{TEST_KEY}", "highlights": "{TEST_KEY}"}}"#),
            // 截断的 JSON，里面嵌了虚构 Key。
            format!(r#"{{"overview": "{TEST_KEY}""#),
        ];
        for content in contents {
            let err = input
                .parse_response(&content)
                .expect_err("结构不符应无效；如果没报错，说明本次回归已失效");
            let text = err.to_string();
            assert!(!text.contains(TEST_KEY), "错误文本不得回显上游内容：{text}");
            assert!(text.contains("行"), "错误文本应保留行列信息：{text}");
        }
    }
}
