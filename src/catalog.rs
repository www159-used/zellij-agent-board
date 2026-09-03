//! CLI catalog. Callers ask questions; they never list bins or event names.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

const BUILTIN_TOML: &str = include_str!("../adapters/catalog.toml");

/// Shared install fingerprint. The OpenCode plugin file embeds this
/// because it shells out to `zellij-agent-board-hook.sh`.
const HOOK_MARKER: &str = "zellij-agent-board-hook";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    adapters: Vec<Adapter>,
    protocols: BTreeMap<String, Protocol>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adapter {
    pub id: String,
    pub badge: String,
    pub color: Option<String>,
    pub bins: Vec<String>,
    pub protocol: String,
    pub settings: String,
    pub hook_dir: String,
    pub skip: Vec<String>,
    pub chat_store_needle: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Protocol {
    pub install: String,
    pub events: Vec<String>,
    pub plugin: Option<String>,
    pub map: BTreeMap<String, String>,
}

#[derive(Debug)]
pub struct CatalogError(String);

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CatalogError {}

#[derive(serde::Deserialize, Default)]
struct CatalogFile {
    #[serde(default)]
    defaults: Defaults,
    #[serde(default)]
    protocol: BTreeMap<String, ProtocolFile>,
    #[serde(default)]
    adapter: Vec<AdapterFile>,
}

#[derive(serde::Deserialize, Default)]
struct Defaults {
    #[serde(default)]
    skip: Vec<String>,
}

#[derive(serde::Deserialize, Default)]
struct ProtocolFile {
    #[serde(default)]
    install: String,
    #[serde(default)]
    events: Vec<String>,
    #[serde(default)]
    plugin: Option<String>,
    #[serde(default)]
    map: BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
struct AdapterFile {
    id: String,
    badge: String,
    #[serde(default)]
    color: Option<String>,
    bins: Vec<String>,
    protocol: String,
    settings: String,
    hook_dir: String,
    #[serde(default)]
    skip: Option<Vec<String>>,
    #[serde(default)]
    ls_holds_chat_store: bool,
    #[serde(default)]
    chat_store_needle: Option<String>,
}

impl Catalog {
    pub fn builtin() -> Self {
        Self::parse(BUILTIN_TOML).expect("packed adapters/catalog.toml is valid")
    }

    pub fn parse(text: &str) -> Result<Self, CatalogError> {
        let mut catalog = Self {
            adapters: Vec::new(),
            protocols: BTreeMap::new(),
        };
        catalog.ingest_file(parse_catalog(text)?);
        Ok(catalog)
    }

    pub fn load() -> Self {
        Self::load_from(Some(&user_adapter_dir()))
    }

    pub fn load_from(user_dir: Option<&Path>) -> Self {
        let mut catalog = Self::builtin();
        let Some(dir) = user_dir else {
            return catalog;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return catalog;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("toml"))
            .collect();
        paths.sort();
        for path in paths {
            if let Ok(text) = std::fs::read_to_string(&path) {
                let _ = catalog.merge_user(&text);
            }
        }
        catalog
    }

    pub fn wants_bin(&self, comm: &str) -> bool {
        self.adapter_for_bin(comm).is_some()
    }

    pub fn keep_process(&self, argv: &[String]) -> bool {
        let bin = bin_name(argv);
        self.wants_bin(bin) && !self.is_skip(argv)
    }

    /// SCAN already decided existence. Unknown bins stay (user drop-ins
    /// on the host); known bins still drop one-shot subcommands.
    pub fn keep_row(&self, argv: &[String]) -> bool {
        if self.adapter_for_bin(bin_name(argv)).is_none() {
            return true;
        }
        !self.is_skip(argv)
    }

    pub fn adapter_for_bin(&self, comm: &str) -> Option<&Adapter> {
        self.adapters
            .iter()
            .rev()
            .find(|adapter| adapter.bins.iter().any(|bin| bin == comm))
    }

    pub fn badge_for(&self, tool: &str) -> (String, Option<String>) {
        match self.adapter_for_bin(tool) {
            Some(adapter) => (adapter.badge.clone(), adapter.color.clone()),
            None => (fallback_badge(tool), None),
        }
    }

    pub fn hook_installed(&self, home: &Path) -> bool {
        self.hook_config_paths(home).into_iter().any(|path| {
            std::fs::read_to_string(path)
                .map(|text| text.contains(HOOK_MARKER))
                .unwrap_or(false)
        })
    }

    pub fn hook_config_paths(&self, home: &Path) -> Vec<PathBuf> {
        let mut paths: Vec<PathBuf> = self
            .adapters
            .iter()
            .map(|adapter| expand_tilde(&adapter.settings, home))
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn normalize_event(&self, event: &str) -> String {
        self.protocols
            .values()
            .find_map(|protocol| protocol.map.get(event).cloned())
            .unwrap_or_else(|| event.to_string())
    }

    pub fn event_map(&self) -> BTreeMap<String, String> {
        let mut map = BTreeMap::new();
        for protocol in self.protocols.values() {
            map.extend(protocol.map.clone());
        }
        map
    }

    pub fn adapters(&self) -> &[Adapter] {
        &self.adapters
    }

    pub fn protocol(&self, id: &str) -> Option<&Protocol> {
        self.protocols.get(id)
    }

    fn is_skip(&self, argv: &[String]) -> bool {
        let Some(adapter) = self.adapter_for_bin(bin_name(argv)) else {
            return false;
        };
        argv.iter()
            .skip(1)
            .any(|arg| adapter.skip.iter().any(|skip| skip == arg))
    }

    fn merge_user(&mut self, text: &str) -> Result<(), CatalogError> {
        match parse_catalog(text) {
            Ok(file) if !file.adapter.is_empty() || !file.protocol.is_empty() => {
                self.ingest_file(file);
                Ok(())
            }
            _ => match toml::from_str::<AdapterFile>(text) {
                Ok(adapter) => {
                    self.upsert_adapter(adapter, &[]);
                    Ok(())
                }
                Err(err) => Err(CatalogError(err.to_string())),
            },
        }
    }

    fn ingest_file(&mut self, file: CatalogFile) {
        for (id, protocol) in file.protocol {
            self.protocols.insert(
                id,
                Protocol {
                    install: protocol.install,
                    events: protocol.events,
                    plugin: protocol.plugin,
                    map: protocol.map,
                },
            );
        }
        for adapter in file.adapter {
            self.upsert_adapter(adapter, &file.defaults.skip);
        }
    }

    fn upsert_adapter(&mut self, file: AdapterFile, defaults: &[String]) {
        let skip = file.skip.unwrap_or_else(|| defaults.to_vec());
        let chat_store_needle = if file.ls_holds_chat_store {
            file.chat_store_needle
                .or_else(|| Some("/.cursor/chats/".into()))
        } else {
            file.chat_store_needle
        };
        let adapter = Adapter {
            id: file.id,
            badge: file.badge,
            color: file.color,
            bins: file.bins,
            protocol: file.protocol,
            settings: file.settings,
            hook_dir: file.hook_dir,
            skip,
            chat_store_needle,
        };
        if let Some(existing) = self.adapters.iter_mut().find(|item| item.id == adapter.id) {
            *existing = adapter;
        } else {
            self.adapters.push(adapter);
        }
    }
}

pub fn keep_process(argv: &[String]) -> bool {
    catalog().keep_process(argv)
}

pub fn keep_row(argv: &[String]) -> bool {
    catalog().keep_row(argv)
}

pub fn badge_for(tool: &str) -> (String, Option<String>) {
    catalog().badge_for(tool)
}

pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        #[cfg(all(not(target_arch = "wasm32"), not(test)))]
        {
            Catalog::load()
        }
        #[cfg(any(target_arch = "wasm32", test))]
        {
            Catalog::builtin()
        }
    })
}

pub fn user_adapter_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("zellij-agent-board/adapters");
        }
    }
    home_dir().join(".config/zellij-agent-board/adapters")
}

pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if path == "~" {
        home.to_path_buf()
    } else {
        PathBuf::from(path)
    }
}

fn parse_catalog(text: &str) -> Result<CatalogFile, CatalogError> {
    toml::from_str(text).map_err(|err| CatalogError(err.to_string()))
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(Into::into)
        .unwrap_or_else(|_| "/tmp".into())
}

fn bin_name(argv: &[String]) -> &str {
    argv.first()
        .map(|bin| bin.rsplit('/').next().unwrap_or(bin))
        .unwrap_or("")
}

fn fallback_badge(tool: &str) -> String {
    let badge: String = tool
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(2)
        .collect::<String>()
        .to_ascii_uppercase();
    if badge.is_empty() {
        "??".into()
    } else {
        badge
    }
}

#[cfg(test)]
mod tests {
    use super::{keep_process, keep_row, Catalog};
    use std::fs;
    use std::path::PathBuf;

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn builtin_has_the_four_families() {
        let catalog = Catalog::builtin();
        let ids: Vec<_> = catalog
            .adapters()
            .iter()
            .map(|adapter| adapter.id.as_str())
            .collect();
        assert_eq!(ids, ["cursor", "codebuddy", "claude", "opencode"]);
        assert!(catalog.wants_bin("agent"));
        assert!(catalog.wants_bin("codebuddy"));
        assert!(catalog.wants_bin("claude"));
        assert!(catalog.wants_bin("opencode"));
        assert!(!catalog.wants_bin("vim"));
    }

    #[test]
    fn cc_family_shares_event_names() {
        let catalog = Catalog::builtin();
        assert_eq!(
            catalog.normalize_event("UserPromptSubmit"),
            "beforeSubmitPrompt"
        );
        assert_eq!(catalog.normalize_event("session.idle"), "stop");
        assert_eq!(catalog.normalize_event("stop"), "stop");
        assert_eq!(
            catalog.protocol("codebuddy").map(|p| p.install.as_str()),
            None
        );
        assert_eq!(
            catalog
                .adapter_for_bin("claude")
                .map(|adapter| adapter.protocol.as_str()),
            Some("cc")
        );
        assert_eq!(
            catalog
                .adapter_for_bin("codebuddy")
                .map(|adapter| adapter.protocol.as_str()),
            Some("cc")
        );
    }

    #[test]
    fn badges_and_cursor_chat_store_stay_on_the_adapter() {
        let catalog = Catalog::builtin();
        assert_eq!(catalog.badge_for("agent"), ("CA".into(), None));
        assert_eq!(
            catalog.badge_for("codebuddy"),
            ("CB".into(), Some("#86b6f2".into()))
        );
        assert_eq!(catalog.badge_for("claude").0, "CC");
        assert_eq!(catalog.badge_for("opencode").0, "OC");
        assert_eq!(
            catalog
                .adapter_for_bin("agent")
                .and_then(|adapter| adapter.chat_store_needle.clone()),
            Some("/.cursor/chats/".into())
        );
        assert!(catalog
            .adapter_for_bin("claude")
            .unwrap()
            .chat_store_needle
            .is_none());
    }

    #[test]
    fn keep_process_drops_one_shots_keep_row_trusts_unknown_scan() {
        assert!(keep_process(&argv(&[
            "/Users/ww/.local/bin/agent",
            "--workspace",
            "/tmp"
        ])));
        assert!(!keep_process(&argv(&["vim", "src/main.rs"])));
        assert!(!keep_process(&argv(&["claude", "-p", "hello"])));
        assert!(!keep_process(&argv(&["opencode", "run", "hello"])));
        assert!(keep_process(&argv(&["claude"])));
        assert!(keep_process(&argv(&["opencode", "/tmp/proj"])));
        assert!(keep_row(&argv(&["opencode-next"])));
        assert!(!keep_row(&argv(&["agent", "status"])));
    }

    #[test]
    fn user_drop_in_adds_a_cc_cli_without_touching_builtin() {
        let dir = std::env::temp_dir().join(format!("zab-catalog-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("mycli.toml"),
            r##"
id = "mycli"
badge = "MY"
color = "#aabbcc"
bins = ["mycli"]
protocol = "cc"
settings = "~/.mycli/settings.json"
hook_dir = "~/.mycli/hooks"
"##,
        )
        .unwrap();
        let catalog = Catalog::load_from(Some(&dir));
        assert_eq!(catalog.badge_for("mycli").0, "MY");
        assert!(catalog.keep_process(&argv(&["mycli"])));
        assert_eq!(
            catalog
                .adapter_for_bin("mycli")
                .map(|adapter| adapter.protocol.as_str()),
            Some("cc")
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn event_map_file_matches_catalog() {
        let catalog = Catalog::builtin();
        let text = include_str!("../scripts/event-map.txt");
        let mut file = std::collections::BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (from, to) = line.split_once('=').expect(line);
            file.insert(from.to_string(), to.to_string());
        }
        assert_eq!(file, catalog.event_map());
    }

    #[test]
    fn opencode_plugin_embeds_the_hook_marker() {
        let plugin = include_str!("../scripts/opencode-plugin.js");
        assert!(plugin.contains(super::HOOK_MARKER));
    }

    #[test]
    fn hook_installed_requires_the_hook_marker() {
        let dir = std::env::temp_dir().join(format!("zab-hooks-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".config/opencode/plugins")).unwrap();
        fs::write(
            dir.join(".config/opencode/plugins/zellij-agent-board.js"),
            "export const x = 'unrelated zellij-agent-board mention';\n",
        )
        .unwrap();
        let catalog = Catalog::builtin();
        assert!(!catalog.hook_installed(&dir));
        fs::write(
            dir.join(".config/opencode/plugins/zellij-agent-board.js"),
            include_str!("../scripts/opencode-plugin.js"),
        )
        .unwrap();
        assert!(catalog.hook_installed(&dir));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hook_paths_expand_home() {
        let catalog = Catalog::builtin();
        let paths = catalog.hook_config_paths(&PathBuf::from("/Users/ww"));
        assert!(paths
            .iter()
            .any(|path| path.ends_with(".cursor/hooks.json")));
        assert!(paths
            .iter()
            .any(|path| path.ends_with(".claude/settings.json")));
        assert!(paths
            .iter()
            .any(|path| path.ends_with("opencode/plugins/zellij-agent-board.js")));
    }
}
