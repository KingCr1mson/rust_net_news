# Design

## Context

项目目前是空的 Git 仓库，只有 OpenSpec 与 Superpowers 装配文件，没有任何 Rust 代码。运行环境为 Linux，Rust 1.99，cargo 通过镜像源获取依赖，GitHub 与两个新闻源可访问。本变更是学习项目，代码应便于阅读，并把"获取 / 解析 / 过滤 / 输出"各环节拆开，方便逐步理解和测试。See proposal.md - Why。

## Goals / Non-Goals

**Goals:**
- 每一环节为纯函数或可注入依赖，使解析、过滤、排序、格式化可以不联网单元测试。
- 新闻源通过一个 trait 抽象，新增源只需实现 trait，不修改获取与输出逻辑。
- 异步获取，多个源并发请求，单源失败互不影响。

**Non-Goals:**
- 不调用任何 AI 接口，不做摘要生成或分析（第二阶段）。
- 不实现 HTML 网页抓取，只消费 RSS / Atom。
- 不实现缓存、重试、定时运行。

## Decisions

### D1：新闻源用 trait 抽象，解析与 HTTP 分离

`NewsSource` trait 提供 `name()`、`url()` 和一个默认的 `parse(&self, body: &[u8]) -> Result<Vec<NewsItem>, SourceError>`。两个内置源都是 RSS，使用默认实现即可。获取逻辑只依赖 trait，不关心具体源。

- 备选：每个源写一个独立函数。拒绝原因：获取和输出的代码会按源重复，且违反 proposal 中"统一接口"的要求。
- 备选：`async_trait` 处理异步方法。拒绝原因：解析是同步纯函数，HTTP 在通用获取函数里完成，因此 trait 本身无需 async，也就不需要 `async_trait` 依赖。

### D2：RSS / Atom 解析使用 `feed-rs`

`feed-rs` 同时支持 RSS 2.0、Atom，并把 pubDate 解析为时间戳。解析失败（内容不是 feed）时返回 `SourceError::Parse`，对应 spec 中"内容无法解析"的场景。

- 备选：`rss` crate 只支持 RSS，且不处理 Atom。拒绝原因：hnrss 与 IT 之家的日期格式不同，`feed-rs` 对两者都能处理。
- 备选：`quick-xml` 手写解析。拒绝原因：工作量大、易错，对学习目标没有额外收益。

### D3："当天"用固定的 UTC+8 偏移计算

`day_range(now)` 接收当前时间（`DateTime<Utc>`），换算到 `FixedOffset::east_opt(8 * 3600)`，取当日 00:00:00 与次日 00:00:00 作为左闭右开区间。`filter_today` 接收条目列表与该区间。

- 备选：`chrono::Local` 读取系统时区。拒绝原因：结果依赖运行机器的配置，单元测试不稳定。用户环境固定在 Asia/Shanghai，固定偏移已足够。
- 备选：在测试中注入 `now`。这一点采纳：`day_range` 与 `filter_today` 都是纯函数，测试直接传入固定时间。

### D4：获取层用 `reqwest` + `tokio::task::JoinSet` 并发

每个源的请求设置 10 秒超时（`RequestBuilder::timeout`），并附带 User-Agent，避免部分站点拒绝默认 UA。所有源通过 `JoinSet` 并发执行，结果按源名称收集，每个源得到一个 `Result`。

- 备选：`futures::join_all`。拒绝原因：需要额外依赖，`JoinSet` 已在 tokio 中提供。
- 备选：顺序请求。拒绝原因：两个源的延迟会相加，与 spec 中"并发执行"冲突。

### D5：统一条目结构与摘要处理

`NewsItem` 使用 `serde` 派生 `Serialize` / `Deserialize`，`published_at` 为 `DateTime<FixedOffset>`，序列化为 RFC 3339。摘要取 feed 的 description / summary，去除 HTML 标签并压缩空白，截断为 200 个字符（按字符而非字节截断，避免切断中文）。

- 备选：保留原始 HTML。拒绝原因：终端输出和 AI 后续处理都需要纯文本。

### D6：错误与退出码

库层用 `thiserror` 定义 `SourceError`（`Http`、`Status`、`Timeout`、`Parse`）。二进制入口不使用 `anyhow`：`run()` 显式返回退出码（`u8`），全部源失败返回 1 并把每个源的原因写到标准错误，部分失败返回 0 并把失败源列在标准错误，参数错误由 clap 返回 2。

- 备选：`main` 返回 `anyhow::Result`。拒绝原因：失败时 stderr 固定为 `Error: ...` 格式，且无法表达"部分失败仍返回 0"；错误已是类型化的 `SourceError`，附加上下文的收益不大。因此不引入 `anyhow` 依赖。

### D7：命令行用 `clap` derive，格式化集中在 `render` 模块

`--json` 为唯一的格式开关。文本与 JSON 两种渲染都放在 `render` 模块，接收 `&[NewsItem]` 并写入 `impl Write`，便于测试捕获输出。

### 模块划分

```
src/
  lib.rs      公开模块
  main.rs     clap 参数、组装流程、退出码
  model.rs    NewsItem
  error.rs    SourceError
  source.rs   NewsSource trait 与内置源 IT之家、Hacker News
  parse.rs    默认 parse 实现所依赖的 feed-rs 解析与摘要清理
  fetch.rs    reqwest 请求与 JoinSet 并发
  today.rs    day_range、filter_today、排序
  render.rs   文本与 JSON 输出
tests/
  fixtures/   IT之家与 hnrss 的样例 RSS（离线单元测试用）
  fetch.rs    wiremock 模拟 HTTP 的集成测试
  live.rs     真实联网测试，标记 #[ignore]
```

## Risks / Trade-offs

- **[风险] 新闻源只返回最新 N 条，当天较早的条目可能不在 feed 中** → 文档说明该限制；第一版接受，后续可通过增加源或分页改进。
- **[风险] RSS 条目缺少发布时间时被丢弃，可能漏掉部分新闻** → spec 已明确丢弃，代价是对源质量有依赖。
- **[风险] 第三方站点改版或限流导致失败** → 单源失败不影响整体；超时与失败原因均输出到标准错误，便于排查。
- **[风险] 固定 UTC+8 不适用于其他时区用户** → 当前用户固定在上海，若将来需要多时区，再改为读取配置。
- **[权衡] 摘要截断为 200 字符** → 牺牲部分细节，换取终端输出可读。

## Testing Strategy

- 单元测试（离线）：用 `tests/fixtures` 中的样例 RSS 验证解析、丢弃规则、摘要清理；用固定 `now` 验证当天区间的边界；验证排序与两种渲染。
- 集成测试（离线）：`wiremock` 模拟单源成功、超时、非 2xx，验证部分失败与全部失败的结果。
- 真实联网测试（`#[ignore]`）：访问两个真实源，手动执行 `cargo test -- --ignored`，验证能否获取数据。
