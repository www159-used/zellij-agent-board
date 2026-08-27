# Roadmap

Zellij Agent Board 规划路线图。

执行跟踪见 [GitHub Issues](https://github.com/www159-used/zellij-agent-board/issues)。

## 近期规划 (v0.5.0)

- [#4 验证 Working 耗时跨重开持久化](https://github.com/www159-used/zellij-agent-board/issues/4) —— 确认新版 `started/` 机制在多回合下计时稳定
- [#2 Agent 较多时优化搜索与跳转体验](https://github.com/www159-used/zellij-agent-board/issues/2) —— 按 `s` 仅在可视区域分配 hint label；支持先跳 session
- [#1 支持 Plan / Ask 模式待确认状态与通知联动](https://github.com/www159-used/zellij-agent-board/issues/1) —— 当 agent 停留在等待用户选择时提供专门标记与通知
- [#3 完善后台通知积压处理](https://github.com/www159-used/zellij-agent-board/issues/3) —— 解决多条通知并发或 board 关闭期间的状态对齐
- [#5 整理 v0.4.0 CHANGELOG 与 Release 说明](https://github.com/www159-used/zellij-agent-board/issues/5) —— 补齐版本更新记录

## 后续探索

- 界面居中布局
- 与 Overview 深度联动（首屏 agent 聚合管理、从 legacy session 跳转进入）
- 专属应用 Icon
- 使用习惯统计与分析

## 当前定论（暂不做）

- **不引入 SQLite / 内存双缓冲**：现有的 `places` + `places.host` 文件合并机制性能与简单度已足够
- **日常 Alt+q 不走 `skip_plugin_cache`**：仅在 Overview (Alt+y) 需要时使用
- **保持轻量桥接架构**：不随意修改 WASM 空桥与 new-pane 唤起机制

## 版本历史

- **v0.4.0**：MVC 架构改造，首帧读缓存秒开，后台异步对账；`places.host` 隔离；滚动视口优化；`started/` 记录回合起始时间
- **v0.3.x**：相对时间展示、未读 Done 标记、系统通知联动、桥接模式
- **v0.2.x**：快捷键切换、flash.nvim 风格跳转高亮
- **v0.1.x**：基础原型与 UI 框架
