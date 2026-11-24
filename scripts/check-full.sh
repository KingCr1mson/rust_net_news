#!/usr/bin/env bash
# 推送前完整检查：快检的全部内容，加上全部目标的构建与 OpenSpec 规约校验。
#
# 说明：本脚本刻意不包含联网测试（`cargo test -- --ignored`）。联网用例依赖真实外网
# （www.ithome.com、hnrss.org），偶发超过规约规定的 10 秒超时，属已接受的外部依赖风险；
# 将其放入推送门禁会使门禁随机失败。联网测试需人工按需单独执行。
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --check

echo "==> cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

echo "==> cargo test"
cargo test

echo "==> cargo build --all-targets"
cargo build --all-targets

echo "==> openspec validate --all --strict"
if ! command -v openspec >/dev/null 2>&1; then
  echo "错误：未找到 openspec 命令，无法校验规约。请安装 OpenSpec 后重试。" >&2
  exit 1
fi
# 校验全部活跃变更与主规约：归档后本变更不再存在，写死变更名会让门禁失败。
openspec validate --all --strict
