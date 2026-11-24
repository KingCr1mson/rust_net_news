# Proposal

## Why

用户想在命令行快速了解当天的时事。现有工具链中没有可直接使用的统一入口，需要从公开的新闻源获取当天的条目，并按统一格式输出。本变更是第一阶段：只完成"获取网络数据并统一格式化输出"，AI 梳理与分析留到第二阶段。

## What Changes

- 新建 Rust 项目 `rust_net_news`（Cargo 管理，lib + bin 结构），提供命令行程序。
- 新增异步新闻获取能力：从两个新闻源并发拉取条目，解析为统一的新闻数据结构。
- 新增"当天"过滤：只输出发布时间落在本地自然日（Asia/Shanghai，即 UTC+8 的当天 00:00 至次日 00:00）内的条目。
- 新增命令行输出：默认以人类可读的文本列出标题、来源、发布时间与链接；`--json` 输出统一结构的 JSON 数组。
- 第一版内置两个新闻源：
  - IT之家 RSS（`https://www.ithome.com/rss/`，中文）
  - Hacker News 的 hnrss 频道（`https://hnrss.org/frontpage`，英文）
- 新闻源以统一的接口抽象，后续新增源不需修改已有源的实现。
- **不交付**：AI 梳理、分析、摘要生成；更多新闻源；定时任务；缓存；图形界面；重试机制。

## Capabilities

### New Capabilities

- `news-fetching`: 从配置的新闻源异步拉取条目，解析为统一结构，并按"当天"条件过滤。
- `news-output`: 将统一结构的新闻条目以文本或 JSON 形式输出到标准输出，并以明确的退出码反映成功或失败。

### Modified Capabilities

（无。项目尚无已有规约。）

## Impact

- **新增代码**：`Cargo.toml`、`src/lib.rs`、`src/main.rs`、新闻源与解析模块、测试与样例数据。
- **依赖（拟定）**：`tokio`（异步运行时）、`reqwest`（HTTP 客户端，启用 rustls）、`feed-rs`（RSS/Atom 解析）、`serde` / `serde_json`（统一结构与 JSON 输出）、`clap`（命令行参数）、`chrono`（时间与时区）、`anyhow` 与 `thiserror`（错误处理）。具体版本在 design 阶段确定。
- **外部依赖**：运行时需访问 `ithome.com` 与 `hnrss.org`。这两个源不需要 API key，但都依赖第三方可用性。
- **仓库**：新增 `.gitignore`（忽略 `target/`），并纳入 Git 管控。
