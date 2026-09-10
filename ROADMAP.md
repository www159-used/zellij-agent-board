# Roadmap

## 当前已实现

- 声明式适配 Cursor、CodeBuddy、Claude Code、OpenCode、Codex、Reasonix；扫描决定 Agent 是否存在，hook 更新状态。
- vim 风格移动、分页、可见区域 Flash、增量搜索和全列表选择器。
- `started/` 保留本轮开始时间，`seen/` 保留已读完成状态，扫描缓存支持重开首帧。
- 默认居中的大浮窗，可配置宽高和位置。
- 原子发布快照，后台扫描子进程回收，Linux/macOS 平台安装包及安装验证。

## 后续工作

- [#4](https://github.com/www159-used/zellij-agent-board/issues/4) **Working 计时的真实环境验证**：持久化与重开回归测试已有，继续覆盖各 CLI 的真实 hook 链路。
- [#2](https://github.com/www159-used/zellij-agent-board/issues/2) **大量 Agent 的搜索体验**：分页、可见区域标签和全列表选择器已有，继续评估 session 分组导航。
- [#1](https://github.com/www159-used/zellij-agent-board/issues/1) **Plan / 模式切换新状态**：当 agent 处于 plan 模式要我选择（AskQuestion）或切换模式时，展示特殊标记，或联动 notification 提醒。
- [#3](https://github.com/www159-used/zellij-agent-board/issues/3) **Notification 积压与 Tick 处理**：解决通知来不及处理、短时间内连续产生通知的问题，用现代 noti 信息 / tick 方式平稳消费对齐。
- [#5](https://github.com/www159-used/zellij-agent-board/issues/5) **补齐 v0.4.0 发版说明**：把 MVC 缓存、places.host、滚动修复等记入 CHANGELOG。
- **真实 Zellij 交互回归**：补齐授权、Alt+q 开关及跨会话跳转；现有无头检查只验证 TUI 绘制和退出。
- **核心模型边界**：将 Board 中的存储副作用移到宿主层，再按搜索、滚动等职责拆分。

以上关联历史议题用于追踪背景，不代表远端 issue 的当前关闭状态。

## 值得研究的问题（后面看）

- **与 Overview 的联动**：甚至可以让 overview 首次渲染 agent 管理，legacy session tab 跳转进入 `space l`。
- **产品设计**：设计专属 Icon。
- **产品维护**：简单的使用数据统计。

## 暂时不做 / 避坑原则

- **保持文件存储**：标题、已读、Working 起点和上次 SCAN 默认在 `~/.cache/zellij-agent-board`。首帧读取缓存；reconcile 在锁内发布扫描和标题快照，TUI 和 hook 单独维护 seen/started 标记。
- **日常 Alt+q 别加 `skip_plugin_cache`**：避免反复加载 WASM 导致内存和发热堆积（仅 overview Alt+y 需要）。
- **保持轻量桥接**（空桥 + new-pane）：优先保证缓存首帧与低开销。
