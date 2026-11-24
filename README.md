# rust_net_news

命令行工具：并发抓取 **IT之家**（`https://www.ithome.com/rss/`）与 **Hacker News**
（`https://hnrss.org/frontpage`）的 RSS，解析为统一结构，只保留**本地 UTC+8 当天**
（当日 00:00:00 至次日 00:00:00，不含）的条目，按发布时间从新到旧输出。

第一阶段只做"获取 + 统一格式化输出"，不做 AI 梳理、分析、摘要生成、缓存、定时任务与重试。

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

## 退出码与错误输出

| 情况 | 标准输出 | 标准错误 | 退出码 |
| --- | --- | --- | --- |
| 全部源成功 | 条目 | 空 | 0 |
| 部分源失败 | 成功源的条目 | 每个失败源及原因 | 0 |
| 全部源失败 | 空 | 每个失败源及原因 | 非零（1） |
| 参数错误 | 空 | 用法说明 | 非零（2） |

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
  main.rs     clap 参数、组装流程、退出码
  model.rs    NewsItem
  error.rs    SourceError
  source.rs   NewsSource trait 与内置源
  parse.rs    feed-rs 解析与摘要清理
  fetch.rs    reqwest 请求与 JoinSet 并发
  today.rs    day_range、filter_today、排序
  render.rs   文本与 JSON 输出
tests/
  fixtures/   离线样例 RSS
  fetch.rs    wiremock 模拟 HTTP 的集成测试
  cli.rs      命令行参数测试
  live.rs     真实联网测试（#[ignore]）
```

## 已知限制

- 新闻源只返回最新 N 条，当天较早的条目可能不在 feed 中。
- 缺少发布时间的条目会被丢弃（无法判断是否属于当天）。
- "当天"固定按 UTC+8 计算，不读取系统时区，也不支持其他时区。
- 摘要去除 HTML 标签后按字符截断到 200 字符。

规约与变更记录见 `openspec/changes/rust-net-news/`。
