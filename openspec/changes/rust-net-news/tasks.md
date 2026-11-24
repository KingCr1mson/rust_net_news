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

- [x] 1.1 `cargo new --lib` 结构：在项目根创建 `Cargo.toml`（name = `rust_net_news`，edition 2021 或更新，声明 lib 与 bin），并验证 `cargo build` 通过
- [x] 1.2 添加依赖（tokio、reqwest、feed-rs、serde、serde_json、clap、chrono、anyhow、thiserror），并验证 `cargo build` 通过；dev-dependencies 添加 wiremock 与 tokio test 支持（`clap` 使用 `4.6.9`，见验证记录中的依赖说明）
- [x] 1.3 添加 `.gitignore`（忽略 `target/`），并验证 `git check-ignore target` 返回命中
- [x] 1.4 创建 `src/lib.rs`、`src/main.rs` 与 design.md 中列出的模块空壳，并验证 `cargo build` 通过、`cargo clippy -- -D warnings` 无警告

## 2. 数据模型与错误

- [x] 2.1 实现 `NewsItem`（title、source、url、published_at 为 `DateTime<FixedOffset>`、summary 可空），派生 Serialize / Deserialize；单元测试验证 JSON 序列化字段名与 RFC 3339 时间格式，满足 news-output 中 JSON 输出的字段要求
- [x] 2.2 实现 `SourceError`（Http、Status、Timeout、Parse），单元测试验证每种错误的 Display 文本包含源名称与原因

## 3. 解析与摘要清理

- [x] 3.1 准备离线样例 `tests/fixtures/ithome.xml` 与 `tests/fixtures/hnrss.xml`，内容取自真实源的节选，并包含一条缺链接、一条缺发布时间的条目
- [x] 3.2 实现 `NewsSource` trait 与内置源 IT之家、Hacker News 的名称和 URL 常量；单元测试验证两个源的名称与 URL 正确
- [x] 3.3 实现默认 `parse`：基于 feed-rs 解析，丢弃缺标题、缺链接、缺发布时间的条目；单元测试用 fixtures 验证丢弃规则与字段映射，并验证非 feed 内容返回 `SourceError::Parse`（对应 news-fetching 的解析场景）
- [x] 3.4 实现摘要清理：去除 HTML 标签、压缩空白、按字符截断至 200 字符；单元测试验证中文不被截断成非法字符，且 HTML 标签被移除

## 4. 当天过滤与排序

- [x] 4.1 实现 `day_range(now: DateTime<Utc>) -> (DateTime<FixedOffset>, DateTime<FixedOffset>)`，按 UTC+8 取当日左闭右开区间；单元测试覆盖 00:00:00、23:59:59、次日 00:00:00 三个边界
- [x] 4.2 实现 `filter_today(items, range)` 与排序（published_at 倒序，同时间按 source、title 字典序）；单元测试验证保留当天条目、丢弃昨天与次日条目，以及排序规则
- [x] 4.3 运行 `cargo test` 验证 3.x 与 4.x 的单元测试全部通过

## 5. 异步获取与并发

- [x] 5.1 实现单源获取函数：reqwest 客户端、10 秒超时、自定义 User-Agent，返回 `Result<Vec<NewsItem>, SourceError>`；使用 wiremock 集成测试验证成功、超时、非 2xx 三种情况
- [x] 5.2 实现 `fetch_all(sources)`，使用 `tokio::task::JoinSet` 并发请求并收集每个源的结果；集成测试验证一个源失败时其他源结果仍返回，且失败源及原因被记录
- [x] 5.3 集成测试验证全部源失败时返回的错误包含每个源的失败原因
- [x] 5.4 运行 `cargo test` 验证 5.x 集成测试全部通过

## 6. 输出渲染

- [x] 6.1 实现文本渲染：每条包含标题、来源、发布时间（UTC+8，`YYYY-MM-DD HH:MM`）、链接，条目间有分隔；空列表输出"当天没有新闻"；单元测试通过写入 `Vec<u8>` 验证内容
- [x] 6.2 实现 JSON 渲染：数组形式，字段为 title、source、url、published_at、summary（空时为 null）；单元测试验证空数组输出 `[]`
- [x] 6.3 运行 `cargo test` 验证 6.x 单元测试全部通过

## 7. 命令行入口

- [x] 7.1 实现 clap 参数（`--json`）与主流程：获取、过滤、排序、渲染；全部源失败时以非零退出码结束，部分失败时把失败源写入标准错误并返回 0；集成测试或 `assert_cmd` 验证退出码与标准错误内容（若不引入 `assert_cmd`，则用 `cargo run` 手动验证，并在任务记录中注明）
- [x] 7.2 手动运行 `cargo run` 与 `cargo run -- --json`，验证对真实源的输出格式正确，并记录实际运行结果（条目数、失败源）到本文件的"验证记录"

## 8. 真实联网测试

- [x] 8.1 编写 `tests/live.rs`，标记 `#[ignore]`，验证对两个真实源的请求至少返回一个条目（若当天无条目则只验证解析成功）；运行 `cargo test -- --ignored` 并记录结果

## 9. 文档与规约收尾

- [x] 9.1 编写 `README.md`：项目简介、运行命令、`--json` 用法、如何运行离线与联网测试，并验证文档中的命令可按原样执行
- [x] 9.2 运行 `openspec validate rust-net-news --strict`，并在本文件记录结论
- [x] 9.3 运行快检 `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`，并记录结论与剩余风险

## Workflow follow-up

- 独立审查：由 codex `gpt-6.2-sol` 对照 specs、tasks 与代码差异做 code review，结论记录在本文件。
- 审查意见未全部解决前，不进入归档。
- 验收并合入后执行 `openspec-archive-change`，同步规约。
- 第二阶段（AI 梳理分析）与更多新闻源另立变更。

## 验证记录

日期：2026-11-24（UTC+8）。工作目录 `/data/t3/test/rust_test`，未 push。

### 提交

| 分组 | 提交 |
| --- | --- |
| 1 脚手架 | `9a329ea` feat: 初始化 rust_net_news 项目脚手架与依赖（任务 1.1-1.4） |
| 2 模型与错误 | `3b1738b` feat: 实现 NewsItem 与 SourceError（任务 2.1-2.2） |
| 3 解析与摘要 | `fe46ad9` feat: 新闻源 trait、RSS 解析与摘要清理（任务 3.1-3.4） |
| 4 当天过滤与排序 | `97ab06f` feat: 当天区间过滤与倒序排序（任务 4.1-4.3）；`6d039a2` chore: 修正 today 模块未使用的 import |
| 5 异步获取 | `18ce580` feat: 单源获取与多源并发获取（任务 5.1-5.4） |
| 6 输出渲染 | `b73a09f` feat: 文本与 JSON 输出渲染（任务 6.1-6.3） |
| 7 命令行入口 | `0cb6e0b` feat: 命令行入口、退出码与 JSON 开关（任务 7.1-7.2） |
| 8 联网测试 | `e7a7c1b` test: 增加真实联网测试（任务 8.1） |

`Cargo.lock` 未提交（锁文件需人工批准），保留在工作区。

### 依赖说明（与任务书版本的差异）

- `clap`：原为 `=4.6.7`，理由是镜像缺 `clap_derive 4.6.9`。独立审查在同一镜像下验证后已解除精确锁定，
  现为 `clap = { version = "4.6.9", features = ["derive"] }`。验证过程：
  `curl -s https://rsproxy.cn/index/cl/ap/clap_derive` 与 `.../cl/ap/clap` 的索引均列出 `4.6.9`；
  `cargo update -p clap` 输出 `Updating clap v4.6.7 -> v4.6.9`、`Updating clap_derive v4.6.7 -> v4.6.9`；
  `cargo build` 与 `cargo test` 均通过（`clap_builder` 仍解析为 4.6.7，由 clap 4.6.9 的 `^4.6.7` 依赖范围决定，兼容）。
- `reqwest 0.13.5`：0.13 已将 `rustls-tls` 特性改名为 `rustls`，实际配置为
  `default-features = false, features = ["rustls", "http2"]`。
- `tokio 1.54.1`、`feed-rs 3.0.0`、`serde 1.0.229`、`serde_json 1.0.154`、`chrono 0.4.45`、
  `anyhow 1.0.105`、`thiserror 2.0.21`、dev 依赖 `wiremock 0.6.5`。
- 未引入 `assert_cmd`：命令行参数测试改用 `env!("CARGO_BIN_EXE_rust_net_news")` 直接启动二进制（`tests/cli.rs`）。

### 分组验证

- 1.x：`cargo build` 通过；`cargo clippy -- -D warnings` 无警告；`git check-ignore target` 输出 `target`。
- 2.x：`cargo test` 中 `model::tests::{json_uses_required_field_names, published_at_is_rfc3339_with_offset, summary_can_be_null_and_round_trips}`、
  `error::tests::{http_error_shows_source_and_reason, status_error_shows_source_and_status, timeout_error_shows_source_and_reason, parse_error_shows_source_and_reason, error_exposes_source_name}` 全部通过。
- 3.x：`tests/fixtures/ithome.xml`、`tests/fixtures/hnrss.xml` 取自 2026-11-24 真实源节选（各 3 条），
  并各含一条缺链接、一条缺发布时间的构造条目（文件内有注释标明）。解析测试验证 5 条中保留 3 条；
  非 feed 内容返回 `SourceError::Parse`；摘要清理测试验证去标签、压空白、按字符截断到 200（中文不出现替换字符）。
- 4.x：`day_range` 覆盖当天 00:00:00、23:59:59、次日 00:00:00 三个边界，以及 UTC 晚上对应本地次日；过滤与排序（含同时间按来源、标题排序）测试通过。
- 5.x：wiremock 集成测试覆盖单源成功、超时（100ms 注入超时）、非 2xx（503）、非 feed 解析失败、部分失败保留成功源、全部失败时错误包含每个源的原因，7 项通过。
- 6.x：文本渲染（空列表、必需字段、分隔、UTC+8 显示）与 JSON 渲染（`[]`、字段集合、`summary: null`）测试通过。
- 7.x：
  - `tests/cli.rs` 验证未知参数以非零退出码结束且标准错误含用法说明，`--help` 输出包含 `--json`。
  - `src/main.rs` 的 `run()` 测试覆盖全部成功、部分失败、全部失败、`--json`、当天无条目（文本与 JSON）6 种情况。

### 手动运行（真实源）

- `cargo run` → 退出码 0，标准错误为空，条目 80 条（IT之家 60、Hacker News 20），按发布时间倒序，
  文本每条包含标题、来源、`YYYY-MM-DD HH:MM`（UTC+8）、链接，条目间以 `----` 分隔。
- `cargo run -- --json` → 退出码 0，标准错误为空，JSON 数组合法，条目 80 条，字段集合为
  `published_at, source, summary, title, url`，`published_at` 形如 `2026-11-24T12:58:53+08:00`，
  倒序检查通过（本次运行的条目摘要均非空，`summary: null` 由离线测试覆盖）。
- `cargo run -- --help` → 输出用法与 `--json` 说明，退出码 0。

### 联网测试

- `cargo test -- --ignored`：3 项（IT之家、Hacker News、两源合并）通过，耗时约 5 秒；合并用例打印“两个源合计解析 80 条”。
- 首次执行时 `live_all_builtin_sources_succeed` 曾失败一次（3 个联网测试并发请求真实站点，总耗时 10.02 秒，
  有一源请求触及 10 秒超时）；把断言补充为输出失败原因后单独重跑通过，随后两次全量重跑均通过。
  剩余风险：联网测试在源站限流或网络抖动时会不稳定，属于外部依赖的既有风险。

### 快检与规约

- `cargo fmt --check`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过，无警告。
- `cargo test`：59 项离线测试通过（lib 42、tests/cli.rs 2、tests/fetch.rs 9、main.rs 6），3 项联网测试按 `#[ignore]` 忽略。
- `openspec validate rust-net-news --strict`：`Change 'rust-net-news' is valid`。

### 独立审查修复（问题 A–E）

审查结论为不通过（审查时 HEAD = `ce96237`）。本轮修复提交（未 push）：

| 问题 | 提交 | 内容 |
| --- | --- | --- |
| A（阻塞） | `dd96ddb` | 缺少发布时间不再回退到 `updated`，只认 `published` |
| B（阻塞） | `ea67fee` | JSON Feed 等非 RSS/Atom 内容记为 `SourceError::Parse` |
| C | `633ae46` | 读取响应体超时归类为 `SourceError::Timeout` |
| D | `919d522` | 排序增加链接兜底键，结果与输入顺序无关 |
| D | `65108d1` | 补充缺标题丢弃与并发获取用例 |
| E | `cd037a1` | 解除 clap 精确锁定，升级 4.6.9 |

每个问题均先写失败测试（RED）再改实现（GREEN）：

- 问题 A：`parse::tests::{drops_rss_item_with_only_updated_time, drops_atom_entry_with_only_updated_time}`。
  RED：`只有更新时间、没有发布时间的 RSS 条目必须被丢弃，实际保留：["只有更新时间：RSS"]`（Atom 同）。
  GREEN：两条均通过，两条 fixtures 仍各解析出 3 条。
- 问题 B：`parse::tests::json_feed_is_parse_error`（最小合法 JSON Feed 1.1）。
  RED：`JSON Feed 不是 RSS / Atom，应视为源解析失败: [NewsItem { title: "JSON Feed 条目", ... }]`。
  GREEN：返回 `SourceError::Parse`，`Display` 含源名称与“解析失败”；IT之家与 hnrss fixtures 解析不受影响。
- 问题 C：`fetch::tests::fetch_reports_timeout_while_reading_body`（本地 TCP 服务返回 200 与 4 KiB 声明长度的部分响应体后挂起，注入 150ms 超时）。
  RED：`读响应体超时应归类为 Timeout：源 IT之家 请求失败：error decoding response body for url (...)`。
  GREEN：归为 `SourceError::Timeout`。（实现：`classify_request_error` 重命名为 `classify_error`，发送与读体共用。）
- 问题 D：
  - `today::tests::identical_keys_are_ordered_independently_of_input_order`。RED：正反两种输入顺序得到不同输出
    （`left: [c, a, b]` / `right: [b, a, c]`）。GREEN：增加 `url` 兜底键后两种输入顺序输出一致。
  - `parse::tests::drops_items_missing_title`：缺 `<title>` 与只有空白标题的两个条目被丢弃，同源其他条目保留。
    该用例补的是覆盖缺口，实现原本已正确，直接通过（无 RED 阶段）。
  - `fetch::tests::fetch_all_runs_sources_concurrently`：两个源各延迟 300ms，总耗时 < 500ms（串行约 600ms）；
    单跑 3 次稳定通过，全量启用时约 0.35s。
- 问题 E：见上文“依赖说明”。`curl https://rsproxy.cn/index/cl/ap/clap_derive` 索引含 4.6.9；
  `cargo update -p clap` → `clap v4.6.7 -> v4.6.9`、`clap_derive v4.6.7 -> v4.6.9`；`cargo build`、`cargo test` 通过。

本轮验证（修复后）：

- `cargo fmt --check`：通过。
- `cargo clippy --all-targets -- -D warnings`：通过，无警告。
- `cargo test`：59 项离线测试通过（lib 42、tests/cli.rs 2、tests/fetch.rs 9、main.rs 6），3 项联网测试忽略。
- `cargo test -- --ignored --test-threads=1`：首次执行 2 项通过、`live_all_builtin_sources_succeed` 失败
  （`实际失败：[Timeout { source_name: "Hacker News", timeout_secs: 10 }]`）；立即重跑 3 项全部通过（3.10s）。
  该用例同时请求两个真实站点，hnrss.org 偶发超过 10 秒，属既有外部依赖风险，未为通过而改断言。
- `cargo run`：退出码 0，标准错误为空，文本 399 行即 80 条（每条 4 行 + `----` 分隔），首条为当天最新。
- `cargo run -- --json`：退出码 0，标准错误为空，合法 JSON 数组 80 条，字段集合 `published_at, source, summary, title, url`，
  `published_at` 形如 `2026-11-24T13:28:50+08:00`，倒序检查通过，来源为 `Hacker News` 与 `IT之家`。
- `openspec validate rust-net-news --strict`：`Change 'rust-net-news' is valid`。
- 工作区：`Cargo.lock` 仍未提交（锁文件需人工批准），另有他人留下的未提交修改 `openspec/changes/rust-net-news/design.md`（D6），本轮未动。

### 未覆盖的场景与剩余风险

- 未覆盖 XML 命名空间、Atom 格式的单独回归（`feed-rs` 负责解析；当前两个内置源均为 RSS 2.0）。
- 条目时间只取 `published`（RSS 的 `pubDate`、Atom 的 `published`）；只有 `updated` / `atom:updated` 的条目按规约丢弃，
  因此只提供 `updated` 的纯 Atom 源会解析出 0 条。当前两个内置源均为 RSS 2.0，不受影响；未来接入 Atom 源需确认其提供 `published`。
- 排序在发布时间、来源、标题之外增加链接作为兜底键，保证三个既有键完全相同的条目得到与输入顺序无关的确定性输出；
  规约只约束前三个键，兜底键不与其冲突。
- 并发用例用“两个源各延迟 300ms、总耗时 < 500ms”区分并发与串行（串行约 600ms）。若 CI 机器极慢该断言可能误报，
  目前余量为 190ms。
- JSON Feed（`feed-rs` 能解析）现按非 RSS/Atom 处理，记为 `SourceError::Parse`；若将来需要支持需另立变更。
- 摘要 HTML 标签为简单字符扫描（非标准 HTML 解析器），未处理 `&amp;` 等实体解码与 CDATA 边界情况。
- 时区固定 UTC+8，不支持其他时区。
- 新闻源只返回最新 N 条，当天较早的条目可能不在 feed 中。
- 真实源内容与日期会随时间变化：fixtures 录制于 2026-11-24，若上游字段样式大改，fixtures 需要重新录制。
- `Cargo.lock` 未纳入版本库，构建结果的依赖解析未锁定；是否提交需人工决策。
