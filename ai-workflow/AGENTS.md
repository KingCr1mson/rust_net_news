# 项目工作流

用 OpenSpec 保存规约与变更进度，用 Superpowers 指导实现与审查。每个会话聚焦一个变更，下一次从该变更的 `tasks.md` 和验证证据接续。

| 场景 | 工具 | 交付标准 |
| --- | --- | --- |
| 新功能、跨模块改造、里程碑 | `openspec-propose` | 写清目标、演示步骤、失败场景、显式不交付项。任务标明可写、只读和禁止路径。范围与方案经人工确认后进入实现。 |
| 实现 | `openspec-apply-change`、`test-driven-development` | 按 `tasks.md` 实现与验证。触及关键行为时先让回归测试失败。有隔离需求且获得授权时，按 `using-git-worktrees` 准备工作树。 |
| 独立审查 | `requesting-code-review` | 由独立会话或获准启动的子 Agent 对照 `specs/`、`tasks.md`、代码差异和测试输出。记录审查所用的提交与仍未覆盖的场景。 |
| 收尾与归档 | `verification-before-completion`、`openspec-archive-change` | 运行相关验证命令，在任务中记录结论与剩余风险。验收并合入后同步规范与归档，更新或清理与实现相冲突的文档。 |

从项目已有的构建、测试和静态检查中选择与变更相关的命令。项目提供 `scripts/check-fast.sh` 或 `scripts/check-full.sh` 时，分别用于提交前快检和推送前完整检查。缺少对应脚本时，按变更的验收步骤运行并记录实际命令、结果及未覆盖场景。发版沿用项目已有流程。

`.env*` 和锁文件的变更须获得人工批准。Lefthook 会检查 Git 暂存区。允许提交已批准的改动时，可给该次提交设置 `ALLOW_PROTECTED_PATHS=1`。团队必须统一执行的检查应加入 CI，并由分支保护要求通过。

需求、实现决策和接续状态归入活跃 OpenSpec 变更。长期架构决策可记入 ADR。团队回顾时检查规范与现行实现是否一致。
