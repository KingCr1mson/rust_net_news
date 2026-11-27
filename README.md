# rust_net_news

命令行工具：并发抓取 **IT之家**（`https://www.ithome.com/rss/`）与 **Hacker News**
（`https://hnrss.org/frontpage`）的 RSS，解析为统一结构，只保留**本地 UTC+8 当天**
（当日 00:00:00 至次日 00:00:00，不含）的条目，按发布时间从新到旧输出。

在此之上新增 `brief` 子命令：把当天新闻交给 **OpenCode**（OpenAI Chat Completions 兼容
接口）生成一份中文简报；AI 失败时自动回退为完整的原始文本列表。

不做逐条摘要、正文抓取、缓存、定时任务与自动重试。

## 环境要求

- Rust 1.99 或更新（使用 edition 2021）
- 运行时可访问 `www.ithome.com` 与 `hnrss.org`

## 运行

```bash
# 文本输出（默认）
cargo run

# JSON 输出
cargo run -- --json

# 查看帮助
cargo run -- --help

# 生成当天中文简报（需要下面三个环境变量中的两个必填项）
cargo run -- brief
```

文本输出示例（每条包含标题、来源、发布时间与链接，条目间用 `----` 分隔）：

```
索尼微软开放《GTA 6》云游戏：PlayStation Portal 无 PS5 也可串流，需 PS Plus 三档高级订阅
来源：IT之家
发布：2026-11-24 12:58
链接：https://www.ithome.com/1/031/466.htm
----
Drone Maps of Salvage Yards
来源：Hacker News
发布：2026-11-24 12:00
链接：https://salvageyarddrone.wordpress.com/
```

当天没有任何条目时，文本输出 `当天没有新闻`。

JSON 输出是一个数组，字段固定为 `title`、`source`、`url`、`published_at`（RFC 3339，
带时区偏移）、`summary`（无摘要时为 `null`）：

```json
[
  {
    "title": "Drone Maps of Salvage Yards",
    "source": "Hacker News",
    "url": "https://salvageyarddrone.wordpress.com/",
    "published_at": "2026-11-24T12:00:28+08:00",
    "summary": "Article URL: https://salvageyarddrone.wordpress.com/ ..."
  }
]
```

当天没有条目时输出 `[]`。

## AI 简报（`brief` 子命令）

```bash
export OPENCODE_API_KEY='...'      # 必填，只用于 Authorization: Bearer 头
export OPENCODE_MODEL='kimi-k3'    # 必填，不内置默认模型
# export OPENCODE_BASE_URL='https://opencode.ai/inference'   # 可选，默认即此值
cargo run -- brief
```

默认列表与 `--json` **不需要**任何 OpenCode 配置，也不会发起 AI 请求；只有显式运行
`brief` 才会读取这些变量。

| 配置 | 必填 | 说明 |
| --- | --- | --- |
| `OPENCODE_API_KEY` | 是 | API Key。不能为空白，也不能含控制字符或换行；不写入任何输出 |
| `OPENCODE_MODEL` | 是 | 模型 ID。没有默认值，避免隐藏模型选择与费用 |
| `OPENCODE_BASE_URL` | 否 | 默认 `https://opencode.ai/inference`（按量付费端点）。必须是 HTTP(S) 地址，不能带 `?`、`#` 或 `user:pass@`，可以带路径前缀 |

行为约定：

- 只使用排序后**最新 50 条**当天新闻生成简报，超过 50 条时在 stderr 提示；原文列表可能回退时仍是完整列表。
- 每次 `brief` **至多一次**非流式请求，输出 token 上限 **1200**，总超时 **60 秒**，**不自动重试**。
- 发送给模型的字段仅：`title`、`source`、`url`、`published_at`、`summary`，以及本地生成的 `id`；**不发送正文**。
- 模型只提供概览、重点标题/摘要和引用 ID；来源名称、原始标题和链接一律由本地条目补回，模型无法替换链接。
- 当天没有条目时输出 `当天没有新闻`，退出 0，不调用 AI。
- 部分源失败：保留每个失败源的 stderr 警告，用成功源继续。
- 全部源失败：不调用 AI，stderr 列出每个源，退出 1。
- AI 超时、网络错误、非 2xx（含 401/429/500）、响应协议无效或简报校验失败：stderr 输出
  `AI 简报生成失败：...` 与 `回退为原始新闻列表`，stdout 输出**完整**当天原文列表，退出 **0**。
  因此判断输出是不是 AI 简报，请看 stderr，而不能只看退出码。
- 回退列表或简报写出（含 flush）失败时退出 1。

**费用与内容提示**：`brief` 会产生按量计费的第三方 API 调用；AI 生成内容可能有事实性错误，
请人工核验后再使用。不要提交真实 Key，也不要把含 Key 的文件放进仓库。

## 退出码与错误输出

| 情况 | 标准输出 | 标准错误 | 退出码 |
| --- | --- | --- | --- |
| 全部源成功 | 条目 | 空 | 0 |
| 部分源失败 | 成功源的条目 | 每个失败源及原因 | 0 |
| 全部源失败 | 空 | 每个失败源及原因 | 非零（1） |
| 参数错误 | 空 | 用法说明 | 非零（2） |
| `brief` 配置缺失或无效 | 空 | 配置错误（含变量名，不含 Key） | 非零（2） |
| `brief` AI 失败并回退成功 | 完整原文列表 | AI 失败 + 回退提示 | 0 |
| `brief` 简报或回退列表写出失败 | 部分或空 | 写出失败原因 | 非零（1） |

## 测试

```bash
# 离线测试：解析、过滤、排序、渲染，以及用 wiremock 模拟 HTTP
cargo test

# 真实联网测试（访问 ithome.com 与 hnrss.org），默认被忽略
cargo test -- --ignored

# 提交前快检
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

离线测试使用 `tests/fixtures/` 下的真实源节选（`ithome.xml`、`hnrss.xml`），
每个文件另含两条构造条目，用于验证"缺链接"与"缺发布时间"的丢弃规则。

## 项目结构

```
src/
  lib.rs      公开模块
  main.rs     clap 参数与子命令、流程编排、退出码
  model.rs    NewsItem
  error.rs    SourceError
  source.rs   NewsSource trait 与内置源
  parse.rs    feed-rs 解析与摘要清理
  fetch.rs    reqwest 请求与 JoinSet 并发
  today.rs    day_range、filter_today、排序
  render.rs   文本与 JSON 输出
  opencode.rs OpenCode 配置与一次 Chat Completions 请求（只做传输）
  brief.rs    简报输入裁剪、提示词、响应校验与本地渲染（只做领域逻辑）
tests/
  fixtures/   离线样例 RSS
  fetch.rs    wiremock 模拟 HTTP 的集成测试
  cli.rs      命令行参数与 brief 配置错误测试
  opencode.rs wiremock 模拟 OpenCode 的请求/失败测试
  live.rs     真实联网测试（#[ignore]）
```

## 已知限制

- 新闻源只返回最新 N 条，当天较早的条目可能不在 feed 中。
- 缺少发布时间的条目会被丢弃（无法判断是否属于当天）。
- "当天"固定按 UTC+8 计算，不读取系统时区，也不支持其他时区。
- 摘要去除 HTML 标签后按字符截断到 200 字符。

规约与变更记录见 `openspec/changes/archive/2026-11-24-rust-net-news/`
与 `openspec/changes/add-ai-news-briefing/`。
