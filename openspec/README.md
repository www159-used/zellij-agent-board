# OpenSpec workflow

本仓库使用 OpenSpec 1.6.0 的 `spec-driven` schema。[官方接入说明](https://openspec.dev/docs/setup)介绍初始化和各工具的入口。

- `specs/`：当前行为基线。首批只覆盖 Agent 状态，不声称已完整描述整个产品。
- `changes/notification-queue/`：通知队列的探索、草案约束和候选设计。
- `config.yaml`：项目上下文及各类文档规则。
- `.codex/skills/`、`.cursor/skills/`：生成的工作流技能；`.cursor/commands/` 为对应命令。

本机已有 CLI。其他机器可安装相同版本：

```sh
npm install -g @fission-ai/openspec@1.6.0
openspec list
openspec status --change notification-queue
openspec validate --all --strict --no-interactive
```

重新初始化工具入口：

```sh
openspec init --tools codex,cursor --profile core
```

Codex 可调用 `$openspec-explore`，Cursor 可调用 `/opsx-explore`。新入口可能需要重启客户端才会显示。

## 当前进度

用户要求先设计，并明确暂不决定通知怎样才算处理完。通知队列的 `proposal.md`、草案 `specs/` 和 `design.md` 已准备，`tasks.md` 有意尚未生成。CLI 将文档标为存在或验证通过，只证明结构可解析，不代表设计被接受或可以实施。

下一步先讨论队列粒度、看板关闭时的提醒、处理动作及过时规则。形成共识后补齐验收场景，再生成实现任务。PR #11 只是试作参考。本设计分支基于 #10，不包含那份试作实现。

后续流程为 explore → proposal/specs/design 评审 → tasks → apply → 验证 → sync/archive。不能在未决定消费策略时把草案同步成当前行为。
