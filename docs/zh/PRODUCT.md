# agent-board 产品说明

在 Zellij 浮动窗口里看本机还在跑的 coding agent，并用 Cursor hook 更新状态，按一下跳到那个 pane。

当前仓库是框架：模块缝、状态映射、安装骨架。扫描、pipe、跳转尚未实现。

## 给谁用

人同时开着好几路 Cursor `agent`（`ww`、`lp`、`mysql_syncer` 各 session 里都有）。现在要知道谁在干活、谁做完了，并且马上落到那一格。

不是给「唤起一个日常 agent」用的，那是 [zellij-agent](../../zellij-agent/docs/zh/PRODUCT.md)（`Alt+a`）。

## 一句话目标

打开一次板子，看见各 session 里还活着的 agent 和它们的 `idle` / `working` / `done`，`Enter` 落到对应 pane。

## 和现有工具的边界

| 东西 | 干什么 | 和本产品的关系 |
|---|---|---|
| overview | 尽快跳到 session / tab | **不合并 wasm**。以后可当门厅：`Ctrl+y` 里按 `a` 打开本插件 |
| zellij-agent | 弹出/收回日常浮动 agent | 无关，不要塞功能 |
| zj-agent-mob | Claude / Codex 舰队监控 | 只模仿「hook 报状态 + 名单 + 跳转」 |

overview 继续只做 session / tab。扫描、hook、名单都在 `agent-board.wasm`。hook 的 `zellij pipe --plugin` 打到 board，可以在藏着的时候收状态。

门厅（后做，不挡第一期）：

1. `Ctrl+y` 打开 overview
2. 按一次 `a`（不要 `Space-a`）
3. overview `hide_self`，聚焦已有的 board 浮窗
4. board 里 `Esc` 先只关 board；要不要回到 overview 以后再定

第一期必须能用自己的快捷键打开（`Alt+g`）。

## 第一期做什么

### 名单

一行一个还活着的进程：

- 工具（第一期只有 Cursor `agent` / `cursor-agent`）
- session、pane 标题
- `--workspace`（能从 argv 抠就显示）
- 状态（见下）

丢掉纯 `agent ls` 选单（未打开 chat）、以及 `status` / `whoami` 等子命令。从 `ls` 里选中会话后 argv 往往仍是 `ls`，但已打开 `~/.cursor/chats/.../store.db`，扫描会保留。

`pi` / `claude` / `codex` 以后加匹配行，不挡第一期。

### 存在性

进程扫描说了算：必须带 `ZELLIJ_PANE_ID` 和 `ZELLIJ_SESSION_NAME`。hook / spool **不能**凭空造行。进程没了，行就没了。

### Hook 状态

用户级 `~/.cursor/hooks.json` 挂上 `scripts/agent-board-hook.sh`（不写进各仓库）。

脚本：没有 pane id 就退出；永远 exit 0，不挡 agent。

- 本 session：`zellij pipe` 到本插件
- 跨 session：`$TMPDIR` 下一行 spool（原子 `mv`），跟扫描同一次 `run_command` 读

| Cursor hook | 板上状态 |
|---|---|
| `sessionStart` | `idle` |
| `beforeSubmitPrompt` | `working` |
| `preToolUse` / `postToolUse` / `beforeShellExecution` | `working`（详情可带当前工具） |
| `preCompact` | `compact` |
| `stop` / `afterAgentResponse` | `done` |
| `sessionEnd` | 以进程是否还在为准 |
| `postToolUseFailure` | 仍 `working` |

没 hook 过、但进程在：`found`（正常）。任务标题只在 turn 边界读 `transcript_path`。

第一期不承诺 `waiting`（Cursor 没有 Claude 的 `Notification` / `PermissionRequest`）。

没装 hook 时名单仍在，全是 `found`。装了并重启过 agent 之后，状态会动。面板顶上可以一行提示「hooks 未装」，不要做成安装向导。

### 跳转与开关

- `hjkl` / 方向键移动，`Enter` 或数字键跳（跨 session 用 `switch_session_with_focus`）
- 再按打开键或 `q` / `Esc` 关掉；浮动层恢复打开前的显隐（对齐 overview）

## 明确不做

- 把扫描 / hook / 名单画进 overview.wasm
- overview 里用 `Space` leader
- 面板里的 install 向导、`a`/`r` 批准、`y`/`m` 代回复、二次确认杀进程
- 桌面通知
- Zellij Ribbon 底栏（普通文字 footer）
- 打开默认进搜索

## 怎样算做成

- 人在 `ww`，打开一次 board，能看到 `lp` 里还活着的 `agent`，以及 hook 报的 `working` / `done` / `idle`
- `Enter` 一次落到那个 pane
- 没 hook 也能列出进程；有 hook 后状态会变

## 约束

- Zellij 0.44+，`wasm32-wasip1`
- Cursor hook 先按本机 macOS CLI；Linux 同版本曾经不跑
- 不要在每次 `update` 后面扫进程（抢 stdin）；跟 pane / session 事件走，有跨 session 行时再低频读 spool
- 快捷键用 `file:` URL；不要抢 overview 的 `Ctrl+y`、zellij-agent 的 `Alt+a`

## 仓库里现在有什么

```
src/lib.rs           板子状态机缝（Dismiss 已接上）
src/status.rs        Cursor hook → 状态
src/agent.rs         行身份、过滤 `agent ls`、抠 workspace
src/discover.rs      扫描行解析（空实现）
src/floating_state.rs 浮动层恢复
src/main.rs          WASM 适配：加载、权限、占位文案、q/Esc 关掉
scripts/install.sh   拷 wasm
scripts/install-hooks.sh / agent-board-hook.sh  占位
```

实现顺序建议：探测 hook 是否在本机 CLI 开火 → 扫描出名单 → pipe / spool → 跳转 → overview 门厅。
