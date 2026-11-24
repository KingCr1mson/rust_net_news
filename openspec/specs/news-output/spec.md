# news-output Specification

## Purpose

以命令行方式输出当天的统一新闻条目，支持人类可读文本和机器可读 JSON 两种格式，并以退出码反映运行结果。

## Requirements

### Requirement: 默认以文本输出条目
系统 SHALL 在未指定格式参数时，将每条新闻以文本形式输出到标准输出，每条至少包含标题、来源、发布时间（UTC+8，格式 `YYYY-MM-DD HH:MM`）和链接。条目之间 SHALL 有可区分的分隔。

#### Scenario: 默认文本输出
- **WHEN** 用户运行命令且不带格式参数，且存在当天条目
- **THEN** 标准输出包含每条条目的标题、来源、发布时间与链接，并以退出码 0 结束

#### Scenario: 没有当天条目
- **WHEN** 用户运行命令，但所有源都没有当天条目
- **THEN** 标准输出显示"当天没有新闻"，并以退出码 0 结束

### Requirement: 支持 JSON 输出
系统 SHALL 在用户指定 `--json` 参数时，以 JSON 数组输出统一结构的条目。每个对象 MUST 包含 `title`、`source`、`url`、`published_at`（RFC 3339 格式，带时区偏移）和 `summary` 字段；`summary` 为空时输出 `null`。

#### Scenario: JSON 输出结构
- **WHEN** 用户运行命令并指定 `--json`
- **THEN** 标准输出为合法 JSON 数组，每个元素包含 title、source、url、published_at、summary 字段

#### Scenario: JSON 无条目
- **WHEN** 用户指定 `--json`，且没有当天条目
- **THEN** 标准输出为空数组 `[]`，并以退出码 0 结束

### Requirement: 错误信息与退出码
系统 SHALL 将错误信息输出到标准错误。当所有新闻源都失败时，系统 MUST 以非零退出码结束。当部分源失败时，系统 SHALL 输出成功源的结果，并以退出码 0 结束，同时在标准错误中列出失败的源。

#### Scenario: 全部失败
- **WHEN** 所有新闻源都失败
- **THEN** 标准错误包含每个源的失败原因，且退出码非零

#### Scenario: 部分失败
- **WHEN** 部分新闻源失败，其余成功
- **THEN** 标准输出包含成功源的条目，标准错误列出失败源，退出码为 0

#### Scenario: 参数错误
- **WHEN** 用户传入无法识别的参数
- **THEN** 系统将用法说明输出到标准错误，并以非零退出码结束
