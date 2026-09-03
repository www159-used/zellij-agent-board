# Declarative CLI catalog

The board watches several coding-agent CLIs. Adapters and protocol families live in TOML (built-in plus `~/.config/zellij-agent-board/adapters/`) so a new cc-family CLI is a file, not a Rust change. Shell `agents.d/` plugins were rejected: install and event maps would duplicate per script instead of sitting on the protocol family.
