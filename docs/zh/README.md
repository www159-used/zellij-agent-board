# zellij-agent-board

[English](../../README.md) · 中文

Zellij 浮动面板：列出还在跑的 coding agent，用 Cursor hook 更新状态，按一下跳到对应 pane。

当前是框架。产品说明见 [PRODUCT.md](PRODUCT.md)。

这不是 [zellij-agent](../../../zellij-agent/docs/zh/README.md)（日常浮动 agent），也不并进 overview 的 wasm。

## 安装

需要 Zellij 0.44+。

```bash
./scripts/install.sh
./scripts/install-hooks.sh
```

WASM 桥和宿主 `board-tui` 都会拷到 `~/.config/zellij/plugins/`。WASM 路径可用 `ZELLIJ_AGENT_BOARD_PLUGIN_PATH` 覆盖；TUI 可用 `ZELLIJ_AGENT_BOARD_TUI` 或插件配置里的 `tui`。

`install-hooks.sh` 会为 Cursor（`~/.cursor/hooks.json`）和 CodeBuddy（`~/.codebuddy/settings.json`）注册 agent hook，可传 `cursor` 或 `codebuddy` 只装其一，默认两者都装。

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

`Alt+q` 打开；再按一次关掉。overview 里按 `a` 当门厅是后做的。

日常不要加 `skip_plugin_cache true`，否则每次 Alt+q 都从磁盘重载 WASM，占用会往上叠。只有改插件本身时才打开。

WASM 会 `hide_self`，再在 command pane 里开 `board-tui`。Cursor hook 只写 `$TMPDIR/zellij-agent-board-spool`，禁止 `zellij pipe --plugin`。跳转是 `zellij pipe --name zellij-agent-board -- JUMP <session> <pane>`，只投给已经在跑的桥。

## 开发

```bash
cargo fmt --check
cargo lint
cargo test --lib
cargo e2e
cargo run --bin board-tui -- --replay e2e/scenes/jump-first-row.scene
./scripts/e2e-zellij.sh
cargo wasm
cargo build --release --bin board-tui
```

`e2e/scenes/` 是 host 场景：每步按声明尺寸画出当前帧，`expect` 检查点只看这一帧。`board-tui --replay` 不用 TTY。`./scripts/e2e-zellij.sh` 在一次性 session 里加载 WASM，dump 板底栏 chrome，再发 `q` 确认 `board-tui` 关掉。无头 session 没法点插件 Allow，所以 TUI pane 由 layout 拉起。没装 `zellij` 就跳过；要强制失败就设 `ZAB_E2E_ZELLIJ_REQUIRED=1`。
