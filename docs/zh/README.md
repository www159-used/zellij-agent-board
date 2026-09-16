# zellij-agent-board

[English](../../README.md) · 中文

Zellij 浮动看板：列出正在运行的 coding agent，通过 hook 更新状态，支持搜索及跨会话跳转到对应 pane。

通过声明式适配目录支持 Cursor、CodeBuddy、Claude Code、OpenCode、Codex 和 Reasonix。进程扫描决定 Agent 的增删，hook 只更新已有 Agent。后续工作见 [ROADMAP.md](../../ROADMAP.md)。

本项目负责看板；独立的 `zellij-agent` 项目负责浮动 agent 启动器。

## 安装

需要 Zellij 0.44+ 和 GNU Make 3.81+，hook 安装器需要 Python 3.11+。从源码构建还需要 `rust-toolchain.toml` 指定的 Rust 工具链。

发布流水线生成 Linux x86_64/ARM64（基于 Ubuntu 24.04 构建）和 macOS Intel/Apple Silicon 的 `.tar.gz` 安装包。每个包包含 WASM 桥、对应平台的 `board-tui` 和 hook 安装器。解压适合自己平台的包，在其目录中执行下面的命令即可，无需 Rust。单独的 `.wasm` 仅适用于已经安装匹配宿主 TUI 的情况。

```bash
make install
make install-hooks
```

`make install` 使用包内二进制；在源码目录中运行时则先构建。两者都安装到 `~/.config/zellij/plugins/`。WASM 路径可用 `ZELLIJ_AGENT_BOARD_PLUGIN_PATH` 覆盖，TUI 安装在旁边。运行时 TUI 路径可用 `ZELLIJ_AGENT_BOARD_TUI` 或插件配置里的 `tui` 覆盖。

`make install-hooks` 按 `adapters/catalog.toml` 注册 hook（Cursor、CodeBuddy、Claude Code、OpenCode、Codex、Reasonix）。用 `make install-hooks ADAPTER=codex` 可只装一个适配器，默认 `ADAPTER=all` 安装全部。新的 cc 系 CLI 丢一份 TOML 到 `~/.config/zellij-agent-board/adapters/` 即可。

## 快捷键

```kdl
shared {
    bind "Alt q" {
        LaunchPlugin "file:~/.config/zellij/plugins/zellij-agent-board.wasm" {
            floating true
        }
    }
}
```

`Alt+q` 打开；再按一次关掉。若与现有快捷键冲突，可自行修改。

打开时自动选中当前 pane 对应的 Agent，并滚动到它的位置；当前 pane 没有 Agent 时，选中最近一次通过面板跳转的 Agent，跨 session、关闭重开也会保留；没有历史或该 Agent 已退出时，默认选中第一行。开始键盘操作、鼠标点击或滚动后，停止自动定位。

日常不要加 `skip_plugin_cache true`，否则每次 Alt+q 都从磁盘重载 WASM，占用会往上叠。只有改插件本身时才打开。

WASM 会隐藏自身，通过 `new-pane --floating --close-on-exit` 打开 `board-tui`。跳转使用 `zellij pipe --name zellij-agent-board -- JUMP <session> <pane>`，只投给已经运行的桥。

| 按键 | 功能 |
| --- | --- |
| `j/k`、方向键、`gg/G` | 移动选择 |
| `Ctrl+d/u`、`Ctrl+f/b` | 半页或整页移动 |
| `/`，随后 `n/N` | 增量搜索、下一个/上一个匹配 |
| `s` | 在可见列表内使用 Flash 标签跳转 |
| `p`，随后 `Tab` | 打开全列表选择器，在查询和标签间切换 |
| `Enter`、鼠标点击 | 跳转到 Agent |
| `?` | 查看帮助 |
| `Esc`、`q` | 关闭看板；`Esc` 优先取消当前浮层，搜索时 `q` 仍作为输入 |

Picker 支持鼠标滚轮，以及右侧滑块的点击和拖动滚动；键盘操作保持原样。

## 运行状态

TUI 自动启动或连接本地 `board-tui --daemon`，通过 Unix socket 上的 HTTP 接口读取已提交快照。只有 daemon 打开 redb，数据库缓存限制为 4 MiB；扫描和标题在同一个事务中可靠落盘。daemon 同时最多执行一轮后台扫描，多个看板共享结果，慢扫描不会阻塞快照读取。打开看板时约每两秒请求刷新；关闭后 daemon 保持可用，但不会自行持续扫描。

`state.redb` 按优先级存放在 `$ZAB_STATE_DIR`、`$XDG_DATA_HOME/zellij-agent-board` 或 `~/.local/share/zellij-agent-board`。首次初始化时，事务导入旧缓存目录的 `scan`、`places` 和 `places.host`。旧文件保留供回退使用，初始化后不再导入。数据库损坏或 schema 不受支持时明确报错，不会重建空库。

focus 和 seen/started 标记仍存放在 `$ZAB_STATE_DIR`、`$XDG_CACHE_HOME/zellij-agent-board` 或 `~/.cache/zellij-agent-board`。本次打开前的焦点直接传给对应的 TUI。Hook 继续通过 `$TMPDIR/zellij-agent-board-spool` 上报通知，不直接打开数据库；未读完成仍可发送终端通知。

`board-tui --snapshot` 输出已提交快照的 JSON，也可通过 `curl --unix-socket` 请求 `GET /v1/snapshot`；`--reconcile` 请求后台刷新，成功只表示已接收请求。升级二进制后，关闭看板并运行 `board-tui --daemon-stop`，再打开看板以启动新版 daemon。详见 [存储设计](../design/host-state.md)。

Codex `Interrupt` 以及 Cursor `stop`/`afterAgentResponse` 且 `status=aborted` 时显示为 `■ stopped`。Cursor `status=error`、Claude/CodeBuddy `StopFailure`、OpenCode `session.error` 显示为 `✗ failed`。两者都清除本轮计时，不标记为完成，也不发送完成通知。授权提示（`PermissionRequest`、CodeBuddy `Notification` 的 `permission_prompt`、OpenCode `permission.asked`）显示为 `● waiting`，并保留本轮起点，以便恢复后继续计时。Claude/CodeBuddy `Notification` 的 `idle_prompt` 显示为 `◑ idle-wait`，清回合且不发完成通知。升级后对所用 CLI 重新运行 `make install-hooks`，并重启这些会话。安装这些 hook 之前发出的通知无法追溯恢复。

## 开发

```bash
make help
make check
make build
make e2e
make replay SCENE=crates/e2e-scenes/scenes/slash-search-moves.scene
make e2e-zellij
```

`make check` 运行 Rust 格式检查、Clippy、产品测试（含 daemon 套件）、进程内场景回放和 hook 测试。`make fmt` 格式化 Rust。宿主工具对应 `make run`、`make stats`、`make scan`、`make reconcile` 和 `make catalog`；可用 `make hook EVENT=stop < payload.json` 回放 hook。

`crates/e2e-scenes/` 在进程内回放 Board 行为：每步按声明尺寸绘制当前帧，`expect` 检查结果；`board-tui --replay` 不需要 TTY。`make e2e-zellij`（`cargo test -p e2e-zellij`）跑真实附着 PTY 的 Zellij 场景。缺少 Zellij 或环境启动失败都会报错。见 [scene E2E](../../crates/e2e-scenes/README.md) 与 [Zellij E2E](../../crates/e2e-zellij/README.md)。

本地打包先按 Rust target 构建，再打包并验证：

```bash
make build TARGET=aarch64-apple-darwin
make package VERSION=v0.8.1 TARGET=aarch64-apple-darwin
make test-package VERSION=v0.8.1 TARGET=aarch64-apple-darwin
make package-wasm VERSION=v0.8.1
```

`make package` 用已有二进制生成安装包和 SHA-256 校验文件，可用 `WASM`、`TUI`、`OUT_DIR` 指定输入和输出路径。`make test-package ARCHIVE=path.tar.gz` 在临时目录中验证解压、安装和宿主程序启动。`make package-wasm` 把独立 WASM 和校验文件写入 `ASSET_DIR`，默认 `target/release-assets`。发布流水线会在每个平台通过此检查后才发布。

## 使用统计

宿主 TUI 将基础行为记录到本机 JSONL，不上传。运行 `board-tui --stats` 查看汇总；设置 `ZELLIJ_AGENT_BOARD_NO_STATS` 关闭采集，`ZELLIJ_AGENT_BOARD_STATS` 可覆盖路径。默认位置是 `$XDG_DATA_HOME/zellij-agent-board/usage.jsonl`，未设置时为 `~/.local/share/zellij-agent-board/usage.jsonl`。

每次打开生成新的访问 ID，没有持久安装 ID。记录操作、变化后的状态、跳转请求和投递结果；不保存输入内容、标题、路径和真实 session/pane 标识。旧版日志可能仍含 session 名，不自动改写。汇总从基础事件推导模式进入次数；投递成功不等于实际聚焦成功，缺失关闭事件的访问单独计数。当前日志尚未轮转。详见[设计说明](../design/usage-analytics.md)。
