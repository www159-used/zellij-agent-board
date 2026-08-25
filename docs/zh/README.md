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

WASM 拷到 `~/.config/zellij/plugins/zellij-agent-board.wasm`。

## 快捷键

```kdl
shared {
    bind "Alt q" {
        LaunchPlugin "file:~/.config/zellij/plugins/zellij-agent-board.wasm" {
            floating true
            skip_plugin_cache true
        }
    }
}
```

`Alt+q` 打开；再按一次关掉。overview 里按 `a` 当门厅是后做的。

## 开发

```bash
cargo fmt --check
cargo lint
cargo test --lib
cargo wasm
```
