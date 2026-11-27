//! rust_net_news：抓取新闻源当天的条目并按统一格式输出，可选生成 AI 简报。
//!
//! 模块划分见 `openspec/changes/archive/2026-11-24-rust-net-news/design.md`：
//! 获取（fetch）、解析（parse）、过滤与排序（today）、输出（render）彼此独立，
//! 便于离线单元测试。
//!
//! AI 简报（`brief` 子命令）在同一套“获取 → 当天过滤 → 排序”之上增加两层：
//!
//! - [`opencode`]：OpenCode 运行时配置与一次 Chat Completions 请求，只负责传输；
//! - [`brief`]：简报的领域结构与校验，负责输入裁剪、提示词、响应校验和本地渲染。
//!
//! 两者职责边界单向：`brief` 只把提示词字符串交给 `opencode` 发送，`opencode` 不依赖
//! 简报领域类型（详见 `src/opencode.rs` 与 `src/brief.rs` 的模块注释）。流程编排、
//! 配置读取与失败回退留在二进制入口 `src/main.rs`。

pub mod brief;
pub mod error;
pub mod fetch;
pub mod model;
pub mod opencode;
pub mod parse;
pub mod render;
pub mod source;
pub mod today;
