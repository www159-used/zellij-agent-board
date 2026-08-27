# Roadmap

## 接下来做（v0.5.0）

- [#4](https://github.com/www159-used/zellij-agent-board/issues/4) **Working 计时跨重开**：日常多测测新加的 `started/` 机制，确认中途重开 board 不会把时间重置成 0。
- [#2](https://github.com/www159-used/zellij-agent-board/issues/2) **多 Agent 搜索优化**：按 `s` 只在当前屏幕看得到的 agent 上标快捷键；支持输 session 名先滚动到该分组。
- [#1](https://github.com/www159-used/zellij-agent-board/issues/1) **Plan / Ask 状态提醒**：Agent 等用户选方案或确认时，给个醒目状态或发通知，一眼看出在等人。
- [#3](https://github.com/www159-used/zellij-agent-board/issues/3) **通知积压平滑处理**：通知来得急或者 board 关着时，再打开能把状态顺畅对齐，不丢事件。
- [#5](https://github.com/www159-used/zellij-agent-board/issues/5) **补 v0.4.0 发版记录**：把首帧缓存、places.host、滚动修复等写进 CHANGELOG。

## 后面有空再看

- 弹窗界面居中
- 和 Overview 深度联动（首屏直接当 agent 门厅、legacy tab 跳进 space）
- 做个好看的 icon
- 简单的本地使用统计

## 暂时不做 / 避坑原则

- **不搞 SQLite / 内存双缓冲**：现有的 `places` + `places.host` 双文件读时合并就够快够稳了。
- **日常 Alt+q 别加 `skip_plugin_cache`**：会导致每次重复加载 WASM 堆积内存，只有 Alt+y overview 需要。
- **不改动当前的轻量桥接方式**（空桥 + new-pane）：保证秒开和低发热。
