#!/usr/bin/env bash
set -euo pipefail

readonly openspec_version=2.1.1
readonly superpowers_tag=v6.1.1
readonly superpowers_commit=d884ae04edebef577e82ff7c4e143debd0bbec99

fail() { printf '装配失败：%s\n' "$*" >&2; exit 1; }
exists() { [[ -e "$1" || -L "$1" ]]; }

copy_template() {
  local template=$1 candidate
  for candidate in "$@"; do
    if exists "$candidate"; then
      if [[ ! -f "$candidate" || -L "$candidate" ]] ||
         ! cmp -s -- "$template_dir/$template" "$candidate"; then
        printf '保留现有 %s，请按需合并模板 %s。\n' "$candidate" "$template"
      fi
      return 0
    fi
  done
  cp -- "$template_dir/$template" "$template"
}

if [[ ${1:-} == -h || ${1:-} == --help ]]; then
  printf '用法：%s [目标 Git 仓库根目录，默认当前目录]\n' "$0"
  exit 0
fi
[[ $# -le 1 ]] || fail '请提供一个目标路径。'
command -v git >/dev/null || fail '缺少 Git。'

template_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
cd -- "${1:-.}"
target_dir=$(pwd -P)
git_root=$(git rev-parse --show-toplevel 2>/dev/null) || fail '请先在目标目录初始化 Git。'
[[ $(cd -- "$git_root" && pwd -P) == "$target_dir" ]] || fail '请指定 Git 仓库根目录。'
for skill_path in .agents .agents/skills; do
  if exists "$skill_path" && [[ ! -d "$skill_path" || -L "$skill_path" ]]; then
    fail "$skill_path 必须是目标项目内的普通目录。"
  fi
done

printf '\n==> 初始化 OpenSpec %s\n' "$openspec_version"
shopt -s nullglob
openspec_skills=(.agents/skills/openspec-*)
if exists openspec || exists .agents/skills/.openspec-target || ((${#openspec_skills[@]})); then
  printf '保留现有 OpenSpec。可用 openspec update 更新已配置的技能。\n'
else
  command -v openspec >/dev/null || fail "请将 OpenSpec $openspec_version 的 Linux 可执行文件加入 PATH。"
  installed_openspec=$(openspec --version) || fail '无法读取 OpenSpec 版本。'
  [[ "$installed_openspec" == "$openspec_version" ]] ||
    fail "当前 OpenSpec 为 $installed_openspec，初始化需要 $openspec_version。请调整 PATH。"
  OPENSPEC_TELEMETRY=0 openspec init . --tools codex --scope project --profile core --no-animation
fi

printf '\n==> 同步 Superpowers %s 项目技能\n' "$superpowers_tag"
tmp_dir=$(mktemp -d)
trap 'rm -rf -- "$tmp_dir"' EXIT
git -c advice.detachedHead=false clone --quiet --depth 1 --branch "$superpowers_tag" --single-branch \
  -- https://github.com/obra/superpowers.git "$tmp_dir/superpowers"
[[ $(git -C "$tmp_dir/superpowers" rev-parse HEAD) == "$superpowers_commit" ]] \
  || fail 'Superpowers 标签与固定提交不匹配。'
mkdir -p .agents/skills
for skill in "$tmp_dir/superpowers/skills/"*/; do
  name=$(basename -- "$skill")
  destination=".agents/skills/$name"
  if exists "$destination"; then
    if [[ -L "$destination" ]] || ! diff -qr -- "$skill" "$destination" >/dev/null 2>&1; then
      printf '保留同名项目技能 %s，请按需检查差异。\n' "$name"
    fi
  else
    cp -R -- "$skill" "$destination"
  fi
done
if ! exists .agents/skills/.superpowers-LICENSE; then
  cp -- "$tmp_dir/superpowers/LICENSE" .agents/skills/.superpowers-LICENSE
fi
copy_template AGENTS.md

printf '\n==> 装配 Git 门禁\n'
copy_template lefthook.yml lefthook.{yaml,toml,json} \
  .lefthook.{yml,yaml,toml,json} .config/lefthook.{yml,yaml,toml,json}

install_hooks() {
  if [[ ! -f lefthook.yml || -L lefthook.yml ]] || ! cmp -s -- "$template_dir/lefthook.yml" lefthook.yml; then
    printf '现有 Lefthook 配置由项目管理，请合并模板后按项目约定安装 Git hooks。\n'
    return
  fi
  if git config --get core.hooksPath >/dev/null; then
    printf '保留现有 core.hooksPath，请按项目约定合并并启用门禁。\n'
    return
  fi
  local hooks_dir hook hook_path
  hooks_dir=$(git rev-parse --git-path hooks)
  for hook in pre-commit pre-push; do
    hook_path="$hooks_dir/$hook"
    if exists "$hook_path" && { [[ ! -f "$hook_path" || -L "$hook_path" ]] ||
      ! grep -Fq "call_lefthook run \"$hook\"" "$hook_path"; }; then
      printf '保留现有 Git hook %s，请按项目约定合并并启用门禁。\n' "$hook_path"
      return
    fi
  done
  if ! command -v lefthook >/dev/null; then
    printf '已写入 lefthook.yml。启用门禁前请将 Lefthook 加入 PATH，再重新运行装配脚本。\n'
    return
  fi
  lefthook install pre-commit pre-push
}
install_hooks

printf '\n装配完成。请检查 git status 和现有配置的合并提示。\n'
