# Notification Queue

## Why

用户会错过短暂显示的 notify，希望它成为一个可继续处理的队列。当前最新 Hook 和终端通知都不能表达“还有哪些事情没注意到”。

## What Changes

已确认：

- 引入 OpenSpec，先设计、评审通知队列，再决定实现。
- 通知需要有队列语义，短暂展示结束不应导致待处理信息丢失。
- 用户明确选择“先只设计，暂不决定”什么动作算处理完。

本次设计拟覆盖：

- 队列按事件保留，还是按 Agent 合并；哪些事情值得入队。
- 看板关闭时如何提示，打开时如何浏览、定位与处理。
- 什么动作消费通知；新回合、退出、重复 Hook、重启如何影响待处理项。
- 验收场景与可靠性目标，然后再选择存储及投递方式。

以上拟覆盖项中的产品策略尚未确认。`i` / `x` 按键、完成/失败/待授权三类全收、文件队列和手动确认均来自 PR #11 的试作，不是已接受需求。

## Capabilities

### New Capabilities

- `notification-queue`：保留待注意信息，并提供可继续处理的通知入口。当前 delta 仅描述不依赖消费策略的共同约束。

### Modified Capabilities

暂无。`agent-status` 仅记录现有行为，是否改变它的已读规则须在消费策略确定后另写 delta。

## Impact

可能涉及 Hook、daemon、持久化模块、host TUI 和相关验证。是否改变终端通知策略、Hook 数据格式或引入存储变更，均待设计决定。

设计从 [PR #10](https://github.com/www159-used/zellij-agent-board/pull/10) 的行为出发。[PR #11](https://github.com/www159-used/zellij-agent-board/pull/11) 保留为试作参考。本分支只引入流程与设计文档，不包含试作代码；尚不创建实现任务，不进入 apply 或 archive。
