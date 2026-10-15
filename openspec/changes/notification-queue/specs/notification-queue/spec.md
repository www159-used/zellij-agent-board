# Notification Queue — Draft Invariants

本文件是设计草案，只列不依赖队列粒度与消费策略的共同约束。入队范围、聚合、处理完成的含义和重启承诺仍在 `design.md` 的 Open Questions 中，当前不构成完整可实施规格。

## ADDED Requirements

### Requirement: Display alone does not consume pending information

The system SHALL 保留已入队且尚未按最终选定策略处理的信息；提示展示或短暂提示消失本身 MUST NOT 被当作消费操作。

#### Scenario: A transient notification disappears

- **WHEN** 用户未执行处理动作，而终端提示已结束展示
- **THEN** 对应待处理信息仍能通过队列入口找到

#### Scenario: The board is opened without handling an item

- **WHEN** 用户仅打开看板查看待处理入口
- **THEN** 仅打开或绘制界面不会消耗待处理信息

### Requirement: A status update is not the queue source of truth

The system SHALL 将待处理信息与每个 pane 可被覆盖的最新状态区分保存；后续 Hook 覆盖最新状态本身 MUST NOT 导致所有尚待处理的信息不可恢复。具体聚合方式仍待决定。

#### Scenario: Latest status is replaced

- **WHEN** 一个已入队项仍待处理，同 pane 的工具 Hook 覆盖了最新状态 spool
- **THEN** 队列仍保有足以发现该待处理信息的记录或聚合投影

### Requirement: Handling a prior item cannot silently consume new information

The system SHALL 将一次处理动作关联到用户实际处理的项或版本；同 Agent 在此之后到达的新信息 MUST NOT 被旧动作静默消费。

#### Scenario: New information arrives during handling

- **WHEN** 用户对当前所见项执行最终选定的处理动作，同时同 Agent 新入队一条信息
- **THEN** 旧动作仅影响它所针对的项或版本，新信息仍有可发现的待处理表达

### Requirement: Notification state does not create a live Agent

The system MUST 保持 Scan 对 live Agent 存在的所有权；持有通知不得单独把历史或未知 Agent 加入 live Agent 列表。

#### Scenario: A notification references a missing Agent

- **WHEN** 队列包含某 pane 的信息，但 Scan 中没有该 live Agent
- **THEN** 看板不会仅因为存在通知而伪造一个 live Agent；通知本身如何呈现或失效须由后续策略明确
