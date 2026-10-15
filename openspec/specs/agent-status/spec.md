# Agent Status

## Purpose

记录 PR #10（`aebe1cc`）已存在的 Agent 状态和通知投递行为，作为通知队列设计的现状基线。此文件不包含 PR #11 的试作行为。

## Requirements

### Requirement: Scan owns live Agent existence

The system MUST use Scan to establish live Agents identified by `(session, pane)`. Hook 状态消息 MUST NOT 单独创建一个 live Agent。

#### Scenario: An unknown Agent sends a hook

- **WHEN** Hook 指向从未被 Scan 发现的 `(session, pane)`
- **THEN** 看板不因该 Hook 新建一个 live Agent

### Requirement: Latest hook is a status projection

Hook MUST 将每个 pane 的最新状态写入临时 spool；后续 Hook 可以替换该 pane 的前一条状态。此通道不承诺保留每次通知。

#### Scenario: A tool hook follows completion

- **WHEN** 同一 pane 的完成 Hook 后又发送工具 Hook
- **THEN** 最新状态 spool 可以只保留工具 Hook

### Requirement: Completion attention is best effort

未读的 `stop` 完成事件 MAY 通过 pane 的 tty 发出 OSC 9 终端通知。配套的 `afterAgentResponse` MUST NOT 再触发同一完成提示；终端弹出展示不代表建立了持久化待处理队列。

#### Scenario: Cursor reports paired completion events

- **WHEN** 同一完成过程依次发送 `stop` 和 `afterAgentResponse`
- **THEN** 只有 `stop` 有资格触发该完成的终端通知

### Requirement: Failure and permission are distinct statuses

失败和中断 MUST NOT 被当作完成发送完成通知。授权请求 MUST 显示为 waiting，并保留本轮起点。

#### Scenario: A permission request interrupts working

- **WHEN** 已知 Agent 在工作中发出授权请求
- **THEN** 看板显示 waiting，保留工作计时起点，不将其记为完成
