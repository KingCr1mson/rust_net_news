# rust_net_news 学习指南

这份文档写给刚开始学 Rust、还没有项目经验、也没有写过网络程序或异步程序的读者。
它的目标是：你读完后，能说清楚这个程序每一步在做什么、代码为什么这样组织，
并知道遇到不懂的库或概念时该去哪里深入学。

文档不替代官方教程。遇到不懂的地方，请按照文末的"延伸阅读"去查，不必一次全部理解。

---

## 目录

1. [这个程序做什么](#1-这个程序做什么)
2. [先认识项目的文件结构](#2-先认识项目的文件结构)
3. [运行起来看效果](#3-运行起来看效果)
4. [程序的整体流程](#4-程序的整体流程)
5. [逐个模块讲解](#5-逐个模块讲解)
6. [几个必须先懂的 Rust 概念](#6-几个必须先懂的-rust-概念)
7. [异步编程入门](#7-异步编程入门)
8. [网络请求入门](#8-网络请求入门)
9. [测试是怎么组织的](#9-测试是怎么组织的)
10. [依赖库一览与学习网址](#10-依赖库一览与学习网址)
11. [建议的阅读顺序与练习](#11-建议的阅读顺序与练习)
12. [常见疑问](#12-常见疑问)
13. [第二阶段：AI 简报](#13-第二阶段ai-简报)
14. [延伸阅读](#14-延伸阅读)

---

## 1. 这个程序做什么

运行 `cargo run`，程序会：

1. 同时去两个网站拉取新闻列表（它们以 **RSS** 格式提供）：
   - IT之家（中文科技新闻）
   - Hacker News（英文科技新闻，通过 hnrss 转成 RSS）
2. 把两边的每条新闻整理成**统一的格式**：标题、来源、链接、发布时间、摘要。
3. 只保留**今天**（北京时间，UTC+8）发布的新闻。
4. 按发布时间从新到旧排序，打印到终端。

加上 `--json` 参数，输出的是 JSON 格式，方便以后交给其他程序（比如将来的 AI 分析模块）处理。

**关于 RSS**：RSS 是网站提供的"订阅列表"，本质是一段 XML 文本，里面按顺序列出最新文章的标题、链接、时间等。
你可以在浏览器打开 `https://www.ithome.com/rss/` 看到原始内容。程序做的事就是：下载这段 XML，读懂它，转成 Rust 的结构体。

---

## 2. 先认识项目的文件结构

```
rust_net_news/
├── Cargo.toml          项目配置与依赖列表（相当于 package.json）
├── Cargo.lock          依赖的精确版本（自动生成，不要手改）
├── src/                程序源码
│   ├── lib.rs          库的入口：声明有哪些模块
│   ├── main.rs         命令行程序的入口：程序从这里开始运行
│   ├── model.rs        统一的新闻条目结构 NewsItem
│   ├── error.rs        错误类型 SourceError
│   ├── source.rs       新闻源：IT之家、Hacker News 的定义
│   ├── fetch.rs        网络请求与并发
│   ├── parse.rs        解析 RSS 内容
│   ├── today.rs        "今天"的时间范围、过滤与排序
│   ├── render.rs       输出为文本或 JSON
│   ├── opencode.rs     OpenCode 配置与一次 Chat Completions 请求（只做传输）
│   └── brief.rs        简报输入裁剪、提示词、响应校验与本地渲染（只做领域逻辑）
├── tests/              集成测试（像外部用户一样调用程序）
│   ├── fixtures/       离线测试用的真实 RSS 样例
│   ├── fetch.rs        获取逻辑的测试（使用本地假服务器）
│   ├── cli.rs          命令行参数与 brief 配置错误的测试
│   ├── opencode.rs     用 wiremock 模拟 OpenCode 的请求与失败测试
│   └── live.rs         真实联网测试（默认不运行）
├── scripts/            提交前检查脚本
├── openspec/           需求与规约文档（项目的"设计依据"）
└── docs/               就是本文件所在的目录
```

**两个概念要分清：**
- **库（lib）** 与 **二进制（bin）**：`src/lib.rs` 是库，里面是可复用的功能；`src/main.rs` 是可执行程序，它调用库。
  这样做的好处是：库可以被测试直接调用，不必每次都启动整个程序。
- **模块（mod）**：每个 `.rs` 文件是一个模块，`lib.rs` 里用 `pub mod xxx;` 把它们声明出来。

---

## 3. 运行起来看效果

在项目根目录执行：

```bash
cargo run              # 文本输出（默认）
cargo run -- --json    # JSON 输出，注意 -- 之后的参数才传给程序
cargo run -- --help    # 查看帮助
cargo run -- brief     # 生成当天的 AI 中文简报（需要 OPENCODE_* 配置）
```

**注意**：`cargo run -- --json` 中间的 `--` 是给 cargo 的分隔符，意思是"后面的参数交给我们自己的程序"。

运行测试：

```bash
cargo test                      # 离线测试（不联网，快速）
cargo test -- --ignored         # 真实联网测试（需要网络）
bash scripts/check-fast.sh      # 格式检查 + 静态检查 + 测试，提交前用
```

---

## 4. 程序的整体流程

把 `main.rs` 的 `run()` 函数看成一条流水线：

```
  builtin_sources()          ← 1. 准备两个新闻源
        │
        ▼
  fetch_all(client, sources)  ← 2. 并发下载两个 RSS（网络）
        │
        ├─ 某个源失败 → 记到 failures，继续
        └─ 全部失败  → 返回错误，退出码 1
        │
        ▼
  parse_feed(...)             ← 3. 把 XML 解析成 NewsItem（在 fetch 内部调用）
        │
        ▼
  filter_today(items, day_range(now))  ← 4. 只留今天的
        │
        ▼
  sort_items(&mut items)      ← 5. 按时间从新到旧排序
        │
        ▼
  render_text / render_json   ← 6. 输出到终端
```

这条流水线的每一步都是一个独立函数，输入输出清楚。**这是这个项目最值得学习的设计习惯**：
把"联网""解析""过滤""输出"拆开，每一块都能单独测试。

第二阶段新增的 `brief` 子命令复用前五步，只在"排序"之后分成两条出口：

```
  collect_today_items()        ← 1~5 步（与默认列表共用的 helper）
        │
        ├─ Vec::is_empty() → 输出“当天没有新闻”，退出 0，不调用 AI
        │
        ▼
  BriefInput::new(&items)      ← 只取最新 50 条，并为每条分配稳定 ID 1..=N
        │
        ▼
  build_user_prompt()          ← 生成 JSON 提示词（只有允许外发的字段）
        │
        ▼
  opencode::request_brief_with_timeout()   ← 唯一一次非流式请求（1200 token / 60 秒）
        │
        ▼
  BriefInput::parse_response() ← 校验结构、检查引用 ID 是否都存在
        │
        ├─ Ok(brief)  → render() 写出中文简报，退出 0
        └─ Err(reason) → stderr 写“AI 简报生成失败”+“回退为原始新闻列表”，
                         stdout 用 render_text 输出**完整**「items」，退出 0
```

---

## 5. 逐个模块讲解

建议按下面的顺序读：**先读数据结构，再读工具函数，最后读流程**。

### 5.1 `model.rs`：统一的新闻条目

```rust
pub struct NewsItem {
    pub title: String,
    pub source: String,
    pub url: String,
    pub published_at: DateTime<FixedOffset>,
    pub summary: Option<String>,
}
```

- `struct` 是结构体，把相关的数据打包在一起。
- `pub` 表示"公开"，其他模块可以访问这个字段。
- `String` 是可增长的文本；`Option<String>` 表示"可能有，也可能没有"（没有时就是 `None`）。
- `DateTime<FixedOffset>` 是带时区的时间。`FixedOffset` 表示固定的时差，这里是 UTC+8。
- `#[derive(Serialize, Deserialize)]` 让这个结构体可以自动转成 JSON，或从 JSON 还原。这是 `serde` 库的功能。

### 5.2 `error.rs`：错误类型

```rust
pub enum SourceError {
    Http { source_name, reason },
    Status { source_name, status },
    Timeout { source_name, timeout_secs },
    Parse { source_name, reason },
}
```

- `enum` 是"枚举"，表示"几种可能情况之一"。这里一个源的失败只可能是：网络错误、HTTP 状态码不对、超时、解析失败。
- `match` 语句用来逐一处理这几种情况，Rust 会检查你是否处理了全部分支（这是 Rust 很有用的特点）。
- `#[error("...")]` 来自 `thiserror` 库，用来自动生成错误的文字描述。

### 5.3 `source.rs`：新闻源

```rust
pub trait NewsSource: Send + Sync {
    fn name(&self) -> &'static str;
    fn url(&self) -> &str;
    fn parse(&self, body: &[u8]) -> Result<Vec<NewsItem>, SourceError> {
        parse_feed(body, self.name())   // 默认实现
    }
}
```

- **trait** 相当于其他语言的"接口"：规定一个类型必须提供哪些方法。
- `IthomeSource` 和 `HackerNewsSource` 都实现了这个 trait。
- 好处：以后要加新的 RSS 源，只需写一个新结构体，实现 `name()` 和 `url()`，解析部分自动复用。
- `Send + Sync` 的意思是"可以安全地跨线程共享"，异步并发时需要。现在可以先记住，不必深究。

`builtin_sources()` 返回一个 `Vec<Arc<dyn NewsSource>>`。这里 `dyn` 表示"某个实现了该 trait 的类型，具体类型运行时决定"，`Arc` 是"可以被多处共享的指针"。
这是 Rust 里表达"多种不同类型放在一起"的常用写法。

### 5.4 `fetch.rs`：网络请求与并发

这是理解难度最高的模块，我们在第 7、8 节详细讲。先看概览：

- `build_client()`：创建一个 HTTP 客户端。
- `fetch_source()`：请求一个源，检查状态码，读取内容，交给 `source.parse()`。
- `fetch_all()`：同时请求所有源，收集结果。

### 5.5 `parse.rs`：解析 RSS

- `parse_feed(body, source_name)`：把 RSS 字节内容变成 `Vec<NewsItem>`。
- 解析本身交给 `feed-rs` 库完成，我们的代码负责**过滤和清洗**：
  - 缺标题的条目丢掉；
  - 缺链接的条目丢掉；
  - 缺发布时间的条目丢掉（注意：不能用"最后修改时间"替代，因为那不是发布时间）；
  - 摘要去掉 HTML 标签（比如 `<p>`、`<img>`），压缩空白，并截断到 200 个字符。
- `clean_summary` 是一个纯函数：输入字符串，输出字符串，没有副作用。这类函数最容易测试。

### 5.6 `today.rs`：今天的时间范围、过滤与排序

- `day_range(now)`：根据当前时间算出"今天 00:00 到明天 00:00"（UTC+8）。
- `filter_today(items, range)`：只保留落在这个区间里的条目。区间是**左闭右开**的，即包含 00:00:00，不包含次日 00:00:00。
- `sort_items`：排序规则是"发布时间从新到旧；时间相同则按来源、再按标题的字典序"。

注意 `day_range` 接收的是 `now` 参数，而不是自己去读系统时间。这是有意设计的：
**测试时可以传入任意时间**，不依赖真实的当前时刻。这是测试设计的一个重要技巧。

### 5.7 `render.rs`：输出

- `render_text`：输出人类可读的文本。
- `render_json`：输出 JSON。
- 两个函数都接收 `out: &mut impl Write`，即"任何可以写入的对象"。
  终端输出时传入标准输出，测试时传入一个 `Vec<u8>`（内存缓冲区）。这样测试就能"捕获"输出内容来比对。

### 5.8 `opencode.rs`：配置与传输

- `OpenCodeConfig`：三个字段来自环境变量。`api_key` 是私有字段，`Debug` 手写成 `REDACTED`，
  所以 `println!("{config:?}")` 不会泄露凭据。
- `OpenCodeConfig::new(...)` 返回 `Result`：所有配置问题（空 Key、空模型、非法 Base URL、
  Key 含换行等）都在这里变成 `OpenCodeError::Config`，由 `main` 转成退出码 2。
  这样做的原因：配置错误必须在任何网络请求之前暴露。
- `request_brief_with_timeout(...)`：一次 `POST <base-url>/openai/v1/chat/completions`，
  `Bearer` 认证、`max_tokens = 1200`、`stream = false`、总超时（覆盖读响应体）。
  它不重试，因为重试意味着可能重复计费。
- 错误只用受控文案：不拼接 `reqwest`/`serde_json` 的 `Display`，避免把请求 URL
  或上游返回的非法字段值写进终端。

### 5.9 `brief.rs`：简报的领域规则

- `BriefInput::new(&[NewsItem])`：借用切片、只克隆前 `MAX_INPUT_ITEMS`（50）条。
  **下标即稳定 ID**：第 0 条的 ID 是 1。提示词里的 ID 和校验引用时的 ID 都来自同一份顺序，
  所以模型返回的 `source_ids` 才能可靠映射回本地条目。
- `build_user_prompt()`：把允许外发的字段序列化成 JSON。字段名就是协议，改字段名等于改提示词。
- `parse_response(&str)`：先把 content 解析成 `RawBrief`（serde 只保证"字段存在且类型对"），
  再逐条检查语义：概览非空、重点 1–5 个、每项标题/摘要非空、`source_ids` 非空，
  且每个 ID 都必须落在 `1..=items.len()`。**任何一处不合法都让整份结果作废**，
  因为半份简报会被误读成完整简报，比明确回退更糟。
- `render(&Brief, out)`：只输出模型给的文字 + 由本地条目查表得到的来源名称/标题/URL。
  模型响应里根本没有 URL 字段，所以链接不可能被模型改写。

### 5.10 `main.rs`：程序入口

- `Args`：用 `clap` 库定义顶层参数 `--json` 与可选子命令 `brief`。
  `args_conflicts_with_subcommands = true` 让 `--json brief` 直接成为用法错误（退出 2）。
- `main()`：程序从这里开始。它用 `#[tokio::main]` 标记，表示"启动异步运行时"（后面第 7 节解释）。
- `run()`：默认列表流程，返回退出码。
- `collect_today_items()`：`run` 与 `brief` 共用的"获取 → 当天过滤 → 排序"，顺带写源失败警告。
- `run_brief()` / `run_brief_with_timeout()`：简报流程。超时可注入，测试用 100ms，
  生产固定 60 秒；AI 失败时回退完整列表并退出 0。
  - 全部源失败：返回 `1`，错误写到标准错误（stderr）。
  - 部分失败：返回 `0`，成功的条目仍然输出，失败的源写到 stderr。
  - 参数错误：返回 `2`（由 clap 自动处理）。

**为什么要区分标准输出和标准错误？** 标准输出（stdout）放"数据"，标准错误（stderr）放"日志和错误"。
这样你可以把结果重定向到文件，比如 `cargo run -- --json > news.json`，错误信息仍然显示在终端。

---

## 6. 几个必须先懂的 Rust 概念

以下概念在本项目中频繁出现。如果你还没学过，建议先看 The Rust Book 对应章节。

### 6.1 所有权与借用

Rust 的核心特性：每个值只有一个"拥有者"，离开作用域时自动释放。

- `&x`：借用（只读引用）。
- `&mut x`：可变借用（可修改）。
- 本项目中 `fn filter_today(items: impl IntoIterator<Item = NewsItem>, ...)` 拿走了 `items` 的所有权，所以调用后原来的变量不能再用。
- `fn sort_items(items: &mut [NewsItem])` 只是借来可变引用，原来的 `Vec` 仍然属于调用者。

**学习建议**：读代码时，看到 `&`、`&mut`、没有 `&` 的参数，就问自己："这个函数拿走了数据，还是只是借用？"

### 6.2 `Result` 与 `?`

```rust
let response = client.get(url).send().await?;
```

- `Result<T, E>` 表示"成功得到 `T`，或者失败得到 `E`"。Rust 没有异常，错误是普通的返回值。
- `?` 的意思："如果是错误，立刻从当前函数返回这个错误；如果成功，取出里面的值"。
- 本项目中 `.map_err(|err| classify_error(...))` 是把 reqwest 的错误转换成我们自己的 `SourceError`。

### 6.3 `Option`

```rust
let url = entry.links.first()...?;    // first() 返回 Option，没有元素时为 None
```

- `Option<T>` 表示"可能有值（`Some(x)`），也可能没有（`None`）"。
- Rust 用它代替 `null`，强迫你处理"没有值"的情况。

### 6.4 闭包 `|x| ...`

```rust
.filter(|item| item.published_at >= start && item.published_at < end)
```

- `|item| ...` 是匿名函数（闭包）。这里表示"对每个条目，判断它是否在区间内"。
- 这种写法属于**迭代器**风格：`.iter()`、`.map()`、`.filter()`、`.collect()` 等。它们是 Rust 处理集合的主要方式。

### 6.5 `trait` 与 `impl`

- `impl Trait for Type`：为某个类型实现某个 trait。
- `impl Iterator` 与 `dyn Trait`：分别表示"静态分发"和"动态分发"。入门时先知道两者都是"某类东西"的写法即可。

### 6.6 宏（`!` 结尾的东西）

`println!`、`vec!`、`format!`、`writeln!` 都是宏。它们在编译时展开成代码。
`#[derive(...)]`、`#[tokio::main]`、`#[test]` 是属性宏，用来自动生成或修改代码。

---

## 7. 异步编程入门

这是本项目里最需要耐心理解的部分。

### 7.1 为什么需要异步？

假设要下载两个网页，每个要 1 秒：

- **同步（一个接一个）**：先等 1 秒，再等 1 秒，共 2 秒。
- **异步（同时进行）**：两个请求同时发出，都等待网络，总共约 1 秒。

网络请求的大部分时间在"等待"，而不是在"计算"。异步让程序在等待时可以去做别的事情。

### 7.2 `async` 与 `.await`

```rust
async fn fetch_source(...) -> Result<Vec<NewsItem>, SourceError> {
    let response = client.get(url).send().await?;
    ...
}
```

- `async fn` 声明一个**异步函数**。调用它时，它不会立刻执行，而是返回一个 **Future**（可以理解为"未来会完成的工作的凭证"）。
- `.await` 表示"在这里等待这个 Future 完成，期间让出 CPU，去执行其他任务"。
- **只能在 `async` 函数里使用 `.await`。**

Future 是惰性的：不 `.await` 或不交给运行时，它就不会执行。这是初学者最容易踩的坑之一。

### 7.3 运行时：tokio

Rust 本身不带异步运行时，需要第三方库提供。本项目使用 **tokio**：

- `#[tokio::main]` 把 `main` 函数包装成异步入口，自动创建运行时。
- 运行时负责调度所有 Future，决定什么时候执行哪个任务。

你可以把运行时想象成一个"任务调度员"：它手里有很多任务，哪个在等待网络，就先去处理别的任务。

### 7.4 并发：JoinSet

`fetch.rs` 中的 `fetch_all`：

```rust
let mut join_set = JoinSet::new();
for (index, source) in sources.iter().enumerate() {
    join_set.spawn(async move { ... });   // 启动一个任务，不等它完成
}
while let Some(joined) = join_set.join_next().await {   // 逐个收集完成的任务
    ...
}
```

- `spawn` 启动一个异步任务，立刻返回，不阻塞。
- `join_next().await` 等待"下一个完成的任务"的结果。
- 两个源就这样同时请求，而不是一个接一个。

**`move` 关键字**：`async move { ... }` 表示把闭包里用到的变量所有权移入任务。因为任务可能比当前函数活得更久，必须拥有自己的数据。

**`clone()` 与 `Arc::clone`**：每个任务都要用同一个 `client` 和同一个源。`client.clone()` 并不是深拷贝，而是复制一个"指向同一个连接池的句柄"，开销很小；`Arc::clone(source)` 则是把源的引用计数加一，让多个任务共享同一个源对象。两者的共同点是：只复制"指针"，不复制底层数据。

### 7.5 `Send` 与 `Sync`

异步任务可能在不同线程上执行，所以数据必须能安全地跨线程传递。
`NewsSource: Send + Sync` 就是要求实现者满足这一点。
初学阶段，遇到 "future cannot be sent between threads safely" 之类的编译错误，通常是某个变量持有了不允许跨线程的类型，记住这个关键词去搜索即可。

---

## 8. 网络请求入门

### 8.1 HTTP 的基本过程

浏览器访问网页时做的事，我们的程序也在做：

1. 发送请求：`GET https://www.ithome.com/rss/`。
2. 服务器返回响应：状态码（如 `200 OK`、`404 Not Found`）、响应头、响应体（内容）。
3. 状态码 `2xx` 表示成功；`3xx` 表示跳转；`4xx`、`5xx` 表示错误。

### 8.2 reqwest

`fetch_source` 中的关键代码：

```rust
let response = client
    .get(source.url())
    .timeout(timeout)
    .send()
    .await?;             // 发送请求，等待响应头

let status = response.status();
if !status.is_success() { return Err(...) }   // 状态码不是 2xx，记为失败

let body = response.bytes().await?;            // 读取响应体（内容）
source.parse(&body)                            // 解析
```

- `send().await` 只等到**响应头**到达。
- `bytes().await` 再等待**响应体**全部读完。
- 两步都可能超时，所以 `classify_error` 对两个阶段都做了处理。

### 8.3 超时

每个源的请求设置 10 秒超时。超过时间，`reqwest` 返回超时错误，我们转换成 `SourceError::Timeout`。
超时是网络程序必须有的保护：否则一个卡住的服务器会让程序永远等下去。

### 8.4 User-Agent

`build_client` 设置了 `User-Agent: rust_net_news/0.1.0`。这是请求头，告诉服务器"我是什么程序"。
一些网站会拒绝没有 UA 或 UA 异常的请求，所以主动设置是常见做法。

### 8.5 重定向

网站经常把 `http://` 跳到 `https://`，或者域名迁移。`reqwest` 默认会自动跟随跳转（最多 10 次），
我们的规约也写明了：**以跳转后的最终响应状态判定成功与否**。这一点有专门的测试（`fetch_follows_redirect_to_successful_feed`）。

---

## 9. 测试是怎么组织的

Rust 的测试分三类：

| 类型 | 位置 | 特点 | 本项目的例子 |
| --- | --- | --- | --- |
| 单元测试 | 源文件内的 `#[cfg(test)] mod tests` | 测试单个函数，可访问私有内容 | `today.rs` 的 `day_range_*` |
| 集成测试 | `tests/` 目录 | 像外部用户一样调用 `lib` 的公开接口 | `tests/fetch.rs` |
| 忽略的测试 | 标注 `#[ignore]` | 默认不运行，需要时手动运行 | `tests/live.rs` |

### 9.1 怎么读一个测试

以 `today.rs` 中的测试为例：

```rust
#[test]
fn day_range_at_next_midnight() {
    let now = local(2026, 11, 25, 0, 0, 0).with_timezone(&Utc);   // 构造一个"已知的时间"
    let (start, end) = day_range(now);                             // 调用被测函数
    assert_eq!(start, local(2026, 11, 25, 0, 0, 0));               // 断言结果
    assert_eq!(end, local(2026, 11, 26, 0, 0, 0));
}
```

读测试的顺序：**准备数据 → 调用函数 → 断言结果**。这个结构称为 "Arrange / Act / Assert"，非常通用。

### 9.2 假服务器：wiremock

`tests/fetch.rs` 使用 `wiremock` 库启动一个**本地假服务器**：

```rust
let server = MockServer::start().await;
Mock::given(method("GET")).and(path("/feed"))
    .respond_with(ResponseTemplate::new(200).set_body_bytes(...))
    .mount(&server).await;
```

这样测试不需要真实联网，结果稳定。你可以精确控制服务器返回什么（成功、500、超时、跳转），
从而测试程序在各种情况下的行为。

### 9.3 固定的样例数据

`tests/fixtures/` 里是真实 RSS 内容的节选，测试直接读取它们。
`include_bytes!` 宏会在编译时把文件内容嵌入程序，所以测试不需要运行时读文件。

### 9.4 为什么不直接测真实网站？

真实网站会变化、会超时、当天可能没有新闻。把它们放进常规测试，会让测试时好时坏。
所以项目把真实联网测试标记为 `#[ignore]`，需要时用 `cargo test -- --ignored` 手动跑。

---

## 10. 依赖库一览与学习网址

以下是 `Cargo.toml` 中每个依赖的作用和官方网址。遇到不懂的地方，先看对应库的文档。

| 库 | 作用 | 在本项目中的位置 | 学习网址 |
| --- | --- | --- | --- |
| **tokio** | 异步运行时，调度异步任务 | `main.rs`、`fetch.rs` | 官网教程：https://tokio.rs/tokio/tutorial<br>API 文档：https://docs.rs/tokio |
| **reqwest** | HTTP 客户端，发送网络请求 | `fetch.rs` | https://docs.rs/reqwest<br>示例：https://github.com/seanmonstar/reqwest/tree/master/examples |
| **feed-rs** | 解析 RSS / Atom / JSON Feed | `parse.rs` | https://docs.rs/feed-rs |
| **serde** | 序列化与反序列化框架 | `model.rs` | 官网：https://serde.rs<br>（重点看 "Derive" 页面） |
| **serde_json** | JSON 的读写 | `render.rs`、`model.rs` | https://docs.rs/serde_json |
| **clap** | 命令行参数解析 | `main.rs` | 官方教程：https://docs.rs/clap/latest/clap/_tutorial/index.html |
| **chrono** | 日期与时间，处理时区 | `today.rs`、`model.rs`、`parse.rs` | https://docs.rs/chrono |
| **thiserror** | 方便地定义错误类型 | `error.rs` | https://docs.rs/thiserror |
| **wiremock** | 测试用的假 HTTP 服务器（仅测试） | `tests/fetch.rs` | https://docs.rs/wiremock |

**关于 `reqwest` 的 `rustls` 特性**：`Cargo.toml` 中写的是 `features = ["rustls"]`，表示使用纯 Rust 实现的 TLS（HTTPS 加密）库，
而不是系统自带的 OpenSSL。这样在不同 Linux 系统上都更容易编译。你可以在 reqwest 文档的 "Feature flags" 部分了解更多。

**标准库（不需要额外安装，但非常重要）**：
- `std::time::Duration`：表示时长，如 `Duration::from_secs(10)`。
- `std::io::Write`：可写入的抽象，`render.rs` 大量使用。
- `std::sync::Arc`：多线程共享的引用计数指针。
- `std::process::ExitCode`：程序退出码。

这些文档在 https://doc.rust-lang.org/std/ 。

### 10.1 Rust 语言本身的学习资料

- **The Rust Programming Language**（官方教程，中文版可在线阅读）：https://doc.rust-lang.org/book/
- **Rust by Example**（边看代码边学）：https://doc.rust-lang.org/rust-by-example/
- **Rust 异步编程**（async book）：https://rust-lang.github.io/async-book/
- **Rustlings**（小练习）：https://github.com/rust-lang/rustlings
- **Cargo 手册**：https://doc.rust-lang.org/cargo/

---

## 11. 建议的阅读顺序与练习

### 11.1 阅读顺序

1. `src/model.rs`：数据长什么样。
2. `src/error.rs`：错误有哪几种。
3. `src/today.rs`：纯函数，最容易读懂，且测试最完整。
4. `src/parse.rs`：RSS 怎样变成 `NewsItem`。
5. `src/render.rs`：输出怎样拼接。
6. `src/source.rs`：trait 的用法。
7. `src/fetch.rs`：异步与网络（最难，放最后）。
8. `src/main.rs`：把以上串起来。
9. `tests/fetch.rs`：看假服务器如何模拟各种情况。

第二阶段的建议顺序：**`src/opencode.rs` → `src/brief.rs` → `src/main.rs`**。

1. `opencode.rs`：先看"一次 HTTP 请求怎么写、配置错误怎么提前暴露"（呼应第 8 节 reqwest / 超时）。
2. `brief.rs`：再看"怎样把模型输出变成可信的领域结构"（呼应 6.2 `Result`、6.3 `Option`、serde）。
3. `main.rs`：最后看编排与回退（呼应第 4 节流水线与 6.1 借用）。
4. `tests/opencode.rs`、`src/main.rs` 的 `#[cfg(test)]`：看 wiremock 如何验证请求次数、
   认证头、超时，以及 writer 失败如何影响退出码。

### 11.2 动手练习（由易到难）

每个练习都建议先自己写，再和项目对照。**修改代码前先在一个新分支或副本里做**，避免弄乱主线。

1. **改摘要长度**：把 `SUMMARY_MAX_CHARS` 从 200 改成 50，运行 `cargo run`，观察摘要的变化。
2. **加一个字段**：给 `NewsItem` 增加一个 `author: Option<String>`，让 JSON 输出它（提示：`feed-rs` 的 `Entry` 有 `authors` 字段）。
3. **改排序**：把排序改成"按标题字母顺序"，并在 `today.rs` 的测试中补充断言。
4. **改超时**：把 `FETCH_TIMEOUT_SECS` 改成 1 秒，然后运行 `cargo run`，观察是否会出现超时错误，以及输出是否仍然正常。
5. **加一个新闻源**：写一个 `struct ExampleSource;`，实现 `NewsSource`。
   提示：找一个 RSS 地址（比如任何网站的 `/feed` 或 `/rss`），加到 `builtin_sources()` 里。
   **注意**：这是一个"写代码 + 写测试"的练习，目前项目规约只有两个源，正式添加需要另立 OpenSpec 变更。
6. **跑一次测试并读懂输出**：`cargo test fetch_follows_redirect` 只运行一个测试，观察输出。
7. **故意让测试失败**：把 `filter_today` 里的 `<` 改成 `<=`，运行 `cargo test`，看哪个测试失败，并解释原因。改完记得还原。

### 11.2b 第二阶段练习（全部离线，不要放真实 Key）

这三个练习都只跑 `cargo test`，**不会访问真实 AI 服务**：

1. **构造无效引用，观察回退**：在 `src/brief.rs` 的测试里把某份简报 JSON 的
   `source_ids` 改成 `[99]`，运行 `cargo test --lib brief::tests`，确认
   `parse_response` 返回 `Err`；再到 `src/main.rs` 的 `brief_unknown_source_id_falls_back`
   观察 CLI 层面如何变成"AI 简报生成失败 + 回退列表 + 退出 0"。
2. **把输入上限改成 2**：临时把 `MAX_INPUT_ITEMS` 改成 2，运行
   `cargo test --lib brief::tests`。你会看到 `input_limits_to_latest_50_items` 失败，
   再看提示词 JSON，确认只发了 2 条。改完记得还原（这是一次"故意让测试失败"的练习）。
3. **给 mock AI 加 429**：在 `tests/opencode.rs` 里把 `reports_non_2xx_without_retry`
   的状态码换成你自己的值，或在 `src/main.rs` 的 `brief_ai_429_falls_back_to_original_list`
   里改状态码，确认 `server.received_requests()` 仍然只有 **1** 条 —— 也就是"不自动重试"。

**注意**：不要把真实 `OPENCODE_API_KEY` 写进代码、测试或文档；测试只使用
明确的虚构值（测试源码里写作 `oc_sk_` 加 `secret_for_test`），而且会断言它不会出现在输出里。

### 11.3 调试的基本工具

- `cargo check`：只检查编译错误，比 `cargo build` 快。
- `cargo clippy`：静态检查，会给出改进建议，读它的建议是很好的学习方式。
- `cargo test -- --nocapture`：测试时显示 `println!` 的输出。
- `cargo run -- --json | head`：查看前几行输出。
- 编译器错误信息很详细，通常会给出建议，**先认真读完**再动手。

---

## 12. 常见疑问

**Q1：为什么 `main` 不直接写 `async fn main`？**
Rust 标准库不允许 `main` 直接是异步的。`#[tokio::main]` 宏帮你把它改写成"创建运行时，然后运行异步的 main"。

**Q2：`&'static str` 是什么？**
`'static` 是生命周期标记，表示"这个字符串在整个程序运行期间都有效"。源名称是写死在代码里的常量，所以是 `'static`。
初学时可以先忽略，把它当作"字符串字面量的类型"。

**Q3：为什么 `NewsSource` trait 的 `parse` 有默认实现？**
因为两个内置源都是 RSS，解析逻辑完全相同。默认实现避免重复代码。如果某个源需要特殊解析，可以在它的 `impl` 里覆盖这个方法。

**Q4：为什么时间要用 `FixedOffset` 而不是 `Local`？**
`Local` 会读取运行机器的时区设置，结果依赖环境，测试时很难稳定。
这个项目只服务于 UTC+8 用户，所以固定偏移最简单、最可预测。代价是不能自动适应其他时区。

**Q5：`Cargo.lock` 为什么要提交到 Git？**
它记录依赖的精确版本，保证每次构建使用相同的库。对于"应用程序"（而非"发布给别人用的库"），通常都应该提交它。

**Q6：`openspec/` 目录是什么？**
这是项目的需求与规约记录。每个功能都有"提案 → 规约 → 设计 → 任务"的文档。
如果你想知道"为什么要这样设计"，可以先读 `openspec/changes/archive/2026-11-24-rust-net-news/design.md`，它记录了每个技术决策的理由和被拒绝的备选方案。

**Q7：什么是 RSS 里的 `pubDate`？**
这是 RSS 中表示发布时间的字段，格式是 RFC 2822（如 `Tue, 24 Nov 2026 05:01:00 GMT`）。
`feed-rs` 会把它解析成统一的时间类型，我们再转换成 UTC+8。

**Q8b：`brief` 失败为什么退出码还是 0？**
因为回退输出的原始列表本身是可用结果；"这次是不是 AI 简报"由 stderr 说明。
如果你要写脚本判断，请看 stderr 是否包含"AI 简报生成失败"。

**Q8c：为什么模型不能给我们 URL？**
因为模型输出不可信。程序只接受引用 ID，再用本地条目补回来源名称、标题和链接，
这样链接一定指向真实抓到的新闻。

**Q8：为什么只有两个新闻源？**
这是第一阶段的范围决定。其他源会在后续变更中加入，这样可以逐个验证，避免一次引入太多不确定因素。

---

## 13. 第二阶段：AI 简报

这一节把上面各章的通用知识落到新代码上。

**数据流**（与第 4 节呼应）：`collect_today_items` → `BriefInput::new` →
`build_user_prompt` → `opencode::request_brief_with_timeout` → `parse_response` → `render`。

**新增的几个 Rust 概念**：

- **serde 双层结构**：`opencode.rs` 的 `ChatRequest`（`Serialize`）把 Rust 字段变成 JSON 键；
  `brief.rs` 的 `RawBrief`（`Deserialize`）把 JSON 变成 Rust 结构。同一份 JSON 用两个类型，
  是因为"发出去的"和"收进来的"是两份不同的协议。
- **`Option` 表达协议允许的缺失**：`ChatResponseMessage::content` 是 `Option<String>`，
  因为协议允许它是 `null`；`None` 必须显式判成错误，而不是当成空字符串。
- **只解析需要的东西**：响应里的 `choices` 先用 `Vec<serde_json::Value>` 收下，
  只对第一个元素做严格解析 —— 这样后面畸形的 choice 不会让整份响应失败。
- **`and_then(|()| out.flush())`**：`render` 成功不代表数据真的到了终端，
  所以写出之后还要 flush，两步中任一步失败都返回退出码 1。
- **测试替身**：`FailingWriter` 让 `write`/`flush` 返回错误，用来验证退出码；
  wiremock 的 `received_requests()` 用来验证"只请求一次"。

**为什么分成两个模块**：`opencode.rs` 只懂 HTTP，`brief.rs` 只懂简报结构，
`main.rs` 只懂编排。这样 `brief.rs` 的测试不需要网络，`opencode.rs` 的测试不需要
构造真实新闻，失败时也能一眼看出是哪一层的错。

## 14. 延伸阅读

按主题整理，建议按顺序学习：

1. **Rust 基础**：The Rust Book 第 1–10 章（变量、所有权、结构体、枚举、`Result`、trait、泛型、生命周期）。
   https://doc.rust-lang.org/book/
2. **迭代器与闭包**：The Rust Book 第 13 章。
3. **Cargo 与模块**：The Rust Book 第 7 章（模块系统）与第 14 章（Cargo）。
4. **错误处理**：The Rust Book 第 9 章。
5. **测试**：The Rust Book 第 11 章。
6. **异步编程**：
   - Async Book：https://rust-lang.github.io/async-book/
   - Tokio 教程：https://tokio.rs/tokio/tutorial （重点看 "Spawning"、"Shared state"、"Channels"）
7. **网络编程**：
   - reqwest 文档：https://docs.rs/reqwest
   - 理解 HTTP：MDN 的 HTTP 概述 https://developer.mozilla.org/zh-CN/docs/Web/HTTP/Overview
8. **serde**：https://serde.rs/ 的 "Derive" 与 "Field attributes" 页面。
9. **RSS 格式**：RSS 2.0 规范 https://www.rssboard.org/rss-specification

---

*本文档对应的代码版本：`add-ai-news-briefing` 变更（第二阶段 AI 简报）实现之后的 `master`
分支；第一阶段内容对应 `85e6674`。如果代码有改动，请以源码为准。*
