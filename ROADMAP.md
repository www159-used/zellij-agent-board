# Roadmap

## 接下来做（v0.5.0）

- [#4](https://github.com/www159-used/zellij-agent-board/issues/4) **Working 时间重新打开会重置**：验证新加的 `started/` 机制，确保退出重开 board 后计时仍然连续，不再从 0 开始。
- [#2](https://github.com/www159-used/zellij-agent-board/issues/2) **Agent 太多显示不下 / 搜索逻辑改造**：
  - 引入更完善的 vim 风格快捷键；
  - `s` 搜索只支持当前可见区域（避免快捷 label 标在屏幕外）；
  - 搜 session 支持先跳到对应 session 分组。
- [#1](https://github.com/www159-used/zellij-agent-board/issues/1) **Plan / 模式切换新状态**：当 agent 处于 plan 模式要我选择（AskQuestion）或切换模式时，展示特殊标记，或联动 notification 提醒。
- [#3](https://github.com/www159-used/zellij-agent-board/issues/3) **Notification 积压与 Tick 处理**：解决通知来不及处理、短时间内连续产生通知的问题，用现代 noti 信息 / tick 方式平稳消费对齐。
- [#5](https://github.com/www159-used/zellij-agent-board/issues/5) **补齐 v0.4.0 发版说明**：把 MVC 缓存、places.host、滚动修复等记入 CHANGELOG。

## 值得研究的问题（后面看）

- **居中布局**：支持弹窗窗口在终端屏幕居中展示。
- **与 Overview 的联动**：甚至可以让 overview 首次渲染 agent 管理，legacy session tab 跳转进入 `space l`。
- **产品设计**：设计专属 Icon。
- **产品维护**：简单的使用数据统计。

## 暂时不做 / 避坑原则

- **不搞 SQLite / 内存双缓冲**：现有的 `places` + `places.host` 双文件读时合并就够快够稳。
- **日常 Alt+q 别加 `skip_plugin_cache`**：避免反复加载 WASM 导致内存和发热堆积（仅 overview Alt+y 需要）。
- **保持轻量桥接**（空桥 + new-pane）：保证秒开与低开销。
