# zellij-agent-board

[English](../../README.md) · 中文

Zellij 浮动看板：列出正在运行的 coding agent，通过 hook 更新状态，支持搜索及跨会话跳转到对应 pane。

通过声明式适配目录支持 Cursor、CodeBuddy、Claude Code、OpenCode 和 Codex。进程扫描决定 Agent 的增删，hook 只更新已有 Agent。后续工作见 [ROADMAP.md](../../ROADMAP.md)。

本项目负责看板；独立的 `zellij-agent` 项目负责浮动 agent 启动器。

## 安装

需要 Zellij 0.44+，hook 安装器需要 Python 3.11+。从源码构建还需要 `rust-toolchain.toml` 指定的 Rust 工具链。

发布流水线生成 Linux x86_64/ARM64（基于 Ubuntu 24.04 构建）和 macOS Intel/Apple Silicon 的 `.tar.gz` 安装包。每个包包含 WASM 桥、对应平台的 `board-tui` 和 hook 安装器。解压适合自己平台的包，在其目录中执行下面的命令即可，无需 Rust。单独的 `.wasm` 仅适用于已经安装匹配宿主 TUI 的情况。

```bash
./scripts/install.sh
./scripts/install-hooks.sh
```

`install.sh` 使用包内二进制；在源码目录中运行时则先构建。两者都安装到 `~/.config/zellij/plugins/`。WASM 路径可用 `ZELLIJ_AGENT_BOARD_PLUGIN_PATH` 覆盖，TUI 安装在旁边。运行时 TUI 路径可用 `ZELLIJ_AGENT_BOARD_TUI` 或插件配置里的 `tui` 覆盖。

`install-hooks.sh` 按 `adapters/catalog.toml` 注册 hook（Cursor、CodeBuddy、Claude Code、OpenCode、Codex）。可传 adapter id 只装其一，默认全部。新的 cc 系 CLI 丢一份 TOML 到 `~/.config/zellij-agent-board/adapters/` 即可。

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

## 运行状态

TUI 首先读取扫描缓存，随后约每两秒请求一次后台 reconcile。每个 TUI 同时保留一个子进程，回收后才启动下一轮；进程锁保证多个看板之间只有一个 reconcile 写入。扫描和标题快照通过原子替换发布。首帧缺失的当前会话标题只补到内存中。

标题、上次扫描、焦点及 seen/started 标记，按优先级存放在 `$ZAB_STATE_DIR`、`$XDG_CACHE_HOME/zellij-agent-board` 或 `~/.cache/zellij-agent-board`。Hook 将最近一条通知写入 `$TMPDIR/zellij-agent-board-spool`，并单独维护本轮开始时间，不启动 WASM 插件。未读完成状态可以发出终端通知。

Codex `Interrupt` 以及 Cursor `stop`/`afterAgentResponse` 且 `status=aborted` 时显示为 `■ stopped`。Cursor `status=error`、Claude/CodeBuddy `StopFailure`、OpenCode `session.error` 显示为 `✗ failed`。两者都清除本轮计时，不标记为完成，也不发送完成通知。授权提示（`PermissionRequest`、CodeBuddy `Notification` 的 `permission_prompt`、OpenCode `permission.asked`）显示为 `● waiting`，并保留本轮起点，以便恢复后继续计时。Claude/CodeBuddy `Notification` 的 `idle_prompt` 显示为 `◑ idle-wait`，清回合且不发完成通知。升级后对所用 CLI 重新运行 `./scripts/install-hooks.sh`，并重启这些会话。安装这些 hook 之前发出的通知无法追溯恢复。

## 开发

```bash
cargo fmt --check
cargo lint
cargo test --locked --lib --bin board-tui
python3 scripts/test-hooks.py
cargo e2e
cargo run --bin board-tui -- --replay e2e/scenes/slash-search-moves.scene
./scripts/e2e-zellij.sh
cargo wasm
cargo build --release --bin board-tui
```

`e2e/scenes/` 是宿主场景：每步按声明尺寸绘制当前帧，`expect` 检查点只看这一帧。`board-tui --replay` 不需要 TTY。`./scripts/e2e-zellij.sh` 在一次性 session 中检查底栏绘制，再发 `q` 确认 TUI 关闭。无头 session 无法授予插件权限，因此直接通过 `new-pane` 启动 TUI，WASM 加载允许跳过。没有 `zellij` 时脚本跳过；设 `ZAB_E2E_ZELLIJ_REQUIRED=1` 可强制失败。授权、Alt+q 开关和跨会话跳转仍需交互验证。

通过 `bash scripts/package-release.sh VERSION TARGET WASM TUI [OUT_DIR]` 生成平台安装包和 SHA-256 校验文件，再用 `bash scripts/test-release-package.sh ARCHIVE` 在临时目录中验证解压、安装和宿主程序启动。发布流水线会在每个平台通过此检查后才发布。
