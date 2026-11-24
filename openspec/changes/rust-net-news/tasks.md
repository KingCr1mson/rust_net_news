# Tasks

## 路径权限

- 可写：`Cargo.toml`、`Cargo.lock`、`src/`、`tests/`、`.gitignore`、`README.md`、`openspec/changes/rust-net-news/`
- 只读：`ai-workflow/`、`.agents/`、`openspec/specs/`
- 禁止：`.env*`、`lefthook.yml`（受保护路径；`Cargo.lock` 视为锁文件，提交须人工批准，见 AGENTS.md）

## 验证命令

- 快检：`cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- 联网验证：`cargo test -- --ignored`（需要访问 ithome.com 与 hnrss.org）
- 规约：`openspec validate rust-net-news --strict`

## 1. 项目脚手架

- [ ] 1.1 `cargo new --lib` 结构：在项目根创建 `Cargo.toml`（name = `rust_net_news`，edition 2021 或更新，声明 lib 与 bin），并验证 `cargo build` 通过
- [ ] 1.2 添加依赖（tokio、reqwest、feed-rs、serde、serde_json、clap、chrono、anyhow、thiserror），并验证 `cargo build` 通过；dev-dependencies 添加 wiremock 与 tokio test 支持
- [ ] 1.3 添加 `.gitignore`（忽略 `target/`），并验证 `git check-ignore target` 返回命中
- [ ] 1.4 创建 `src/lib.rs`、`src/main.rs` 与 design.md 中列出的模块空壳，并验证 `cargo build` 通过、`cargo clippy -- -D warnings` 无警告

## 2. 数据模型与错误

- [ ] 2.1 实现 `NewsItem`（title、source、url、published_at 为 `DateTime<FixedOffset>`、summary 可空），派生 Serialize / Deserialize；单元测试验证 JSON 序列化字段名与 RFC 3339 时间格式，满足 news-output 中 JSON 输出的字段要求
- [ ] 2.2 实现 `SourceError`（Http、Status、Timeout、Parse），单元测试验证每种错误的 Display 文本包含源名称与原因

## 3. 解析与摘要清理

- [ ] 3.1 准备离线样例 `tests/fixtures/ithome.xml` 与 `tests/fixtures/hnrss.xml`，内容取自真实源的节选，并包含一条缺链接、一条缺发布时间的条目
- [ ] 3.2 实现 `NewsSource` trait 与内置源 IT之家、Hacker News 的名称和 URL 常量；单元测试验证两个源的名称与 URL 正确
- [ ] 3.3 实现默认 `parse`：基于 feed-rs 解析，丢弃缺标题、缺链接、缺发布时间的条目；单元测试用 fixtures 验证丢弃规则与字段映射，并验证非 feed 内容返回 `SourceError::Parse`（对应 news-fetching 的解析场景）
- [ ] 3.4 实现摘要清理：去除 HTML 标签、压缩空白、按字符截断至 200 字符；单元测试验证中文不被截断成非法字符，且 HTML 标签被移除

## 4. 当天过滤与排序

- [ ] 4.1 实现 `day_range(now: DateTime<Utc>) -> (DateTime<FixedOffset>, DateTime<FixedOffset>)`，按 UTC+8 取当日左闭右开区间；单元测试覆盖 00:00:00、23:59:59、次日 00:00:00 三个边界
- [ ] 4.2 实现 `filter_today(items, range)` 与排序（published_at 倒序，同时间按 source、title 字典序）；单元测试验证保留当天条目、丢弃昨天与次日条目，以及排序规则
- [ ] 4.3 运行 `cargo test` 验证 3.x 与 4.x 的单元测试全部通过

## 5. 异步获取与并发

- [ ] 5.1 实现单源获取函数：reqwest 客户端、10 秒超时、自定义 User-Agent，返回 `Result<Vec<NewsItem>, SourceError>`；使用 wiremock 集成测试验证成功、超时、非 2xx 三种情况
- [ ] 5.2 实现 `fetch_all(sources)`，使用 `tokio::task::JoinSet` 并发请求并收集每个源的结果；集成测试验证一个源失败时其他源结果仍返回，且失败源及原因被记录
- [ ] 5.3 集成测试验证全部源失败时返回的错误包含每个源的失败原因
- [ ] 5.4 运行 `cargo test` 验证 5.x 集成测试全部通过

## 6. 输出渲染

- [ ] 6.1 实现文本渲染：每条包含标题、来源、发布时间（UTC+8，`YYYY-MM-DD HH:MM`）、链接，条目间有分隔；空列表输出"当天没有新闻"；单元测试通过写入 `Vec<u8>` 验证内容
- [ ] 6.2 实现 JSON 渲染：数组形式，字段为 title、source、url、published_at、summary（空时为 null）；单元测试验证空数组输出 `[]`
- [ ] 6.3 运行 `cargo test` 验证 6.x 单元测试全部通过

## 7. 命令行入口

- [ ] 7.1 实现 clap 参数（`--json`）与主流程：获取、过滤、排序、渲染；全部源失败时以非零退出码结束，部分失败时把失败源写入标准错误并返回 0；集成测试或 `assert_cmd` 验证退出码与标准错误内容（若不引入 `assert_cmd`，则用 `cargo run` 手动验证，并在任务记录中注明）
- [ ] 7.2 手动运行 `cargo run` 与 `cargo run -- --json`，验证对真实源的输出格式正确，并记录实际运行结果（条目数、失败源）到本文件的"验证记录"

## 8. 真实联网测试

- [ ] 8.1 编写 `tests/live.rs`，标记 `#[ignore]`，验证对两个真实源的请求至少返回一个条目（若当天无条目则只验证解析成功）；运行 `cargo test -- --ignored` 并记录结果

## 9. 文档与规约收尾

- [ ] 9.1 编写 `README.md`：项目简介、运行命令、`--json` 用法、如何运行离线与联网测试，并验证文档中的命令可按原样执行
- [ ] 9.2 运行 `openspec validate rust-net-news --strict`，并在本文件记录结论
- [ ] 9.3 运行快检 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`，并记录结论与剩余风险

## Workflow follow-up

- 独立审查：由 codex `gpt-6.2-sol` 对照 specs、tasks 与代码差异做 code review，结论记录在本文件。
- 审查意见未全部解决前，不进入归档。
- 验收并合入后执行 `openspec-archive-change`，同步规约。
- 第二阶段（AI 梳理分析）与更多新闻源另立变更。
