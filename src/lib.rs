//! rust_net_news：抓取新闻源当天的条目并按统一格式输出。
//!
//! 模块划分见 `openspec/changes/rust-net-news/design.md`：
//! 获取（fetch）、解析（parse）、过滤与排序（today）、输出（render）彼此独立，
//! 便于离线单元测试。

pub mod error;
pub mod fetch;
pub mod model;
pub mod parse;
pub mod render;
pub mod source;
pub mod today;
