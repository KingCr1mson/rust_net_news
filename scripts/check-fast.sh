#!/usr/bin/env bash
# 提交前快检：格式、静态检查、离线测试。任一失败即非零退出，供 Lefthook 拦截。
set -euo pipefail
cd "$(dirname "$0")/.."

echo "==> cargo fmt --check"
cargo fmt --check

echo "==> cargo clippy --all-targets -- -D warnings"
cargo clippy --all-targets -- -D warnings

echo "==> cargo test"
cargo test
