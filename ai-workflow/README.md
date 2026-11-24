# ai-workflow

方案 A 为 Linux 软件仓库装配 OpenSpec、Superpowers 和可选的 Lefthook。OpenSpec 记录变更范围和验收，Superpowers 提供实现和审查技能，Lefthook 在 Git 操作时执行项目配置的检查。

## 安装

准备 Git、Bash 和 [OpenSpec 2.1.1 的 Linux 可执行文件](https://github.com/Fission-AI/OpenSpec/releases/tag/v2.1.1)。需要本地 Git 门禁时，也将 [Lefthook 的 Linux 可执行文件](https://github.com/evilmartians/lefthook/releases/tag/v2.2.2) 放入 `PATH`。可用 `openspec --version` 和 `lefthook version` 检查终端使用的版本。装配脚本从 GitHub 拉取固定版本的 Superpowers。整套装配可在没有 Node.js 的环境中运行。

将业务项目初始化为 Git 仓库后，运行以下命令。

```bash
/path/to/ai-workflow/bootstrap.sh /path/to/your-project
```

脚本用 OpenSpec 的 `core` 配置初始化 Codex 项目技能，并将 Superpowers `v6.1.1` 固定提交的技能和许可证复制到 `.agents/skills/`。脚本保留已有规则、规约、同名技能和门禁配置。请按输出提示合并。如已配置自定义 Git hooks 或 `core.hooksPath`，请在项目现有的门禁中接入所需检查。

## 配置项目检查

下面两个文件都可按需提供，由业务仓库负责定义实际命令。示例中的 `test-fast` 和 `test` 是目标项目已有的 Make 目标。

- `scripts/check-fast.sh` 在提交前运行，可用于编译受影响模块或执行短用例。Make 项目可以写 `make test-fast`。
- `scripts/check-full.sh` 在推送前运行，可用于构建和集成测试。Make 项目可以写 `make test`。CMake 项目可在同一个脚本中配置、构建，再执行 `ctest --test-dir build --output-on-failure`。

Lefthook 从仓库根目录用 Bash 执行脚本。脚本应把失败退出码传给钩子。不存在的脚本会提示跳过，提交时的受保护路径检查仍会运行。也可以直接运行 `bash scripts/check-fast.sh` 或 `bash scripts/check-full.sh`。

## 日常开发与验收

1. 用 `openspec-propose` 写明范围、验收步骤和显式不交付项，经人工确认后进入 `openspec-apply-change`。
2. 实现后更新 `tasks.md`，记录实际构建、测试和手工验收结果。`openspec validate <change-id> --strict` 检查规约文件。独立审查者对照规约、代码差异和验证结果核对实现。
3. 验收并合入后归档 OpenSpec 变更。发版沿用项目约定，例如 Git tag、构建制品、系统软件包或现有 CI 流程。

Git hooks 需在每个克隆中安装。代码检查脚本读取当前工作区。团队要求的验证可在 CI 中对提交执行相同脚本。

## 从旧模板迁移

已装配过旧版本时，将现有 `AGENTS.md` 和 `lefthook.yml` 中的测试和发布条目替换为上面的项目命令，再按项目约定安装或更新 Git hooks。若业务项目此前仅为工作流引入 `knip.json`、`.changeset/` 或 Node.js 依赖，请检查是否还有其他用途，再由项目移除对应文件或依赖。
