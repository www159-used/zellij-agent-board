//! Discover and run every `scenes/*.scene` file through [`zellij_agent_board::run_scene`].
use std::fs;
use std::path::PathBuf;

use zellij_agent_board::run_scene;

#[test]
fn all_scenes_pass() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenes");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap_or_else(|err| panic!("{}: {err}", dir.display()))
        .map(|entry| entry.expect("scene entry").path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("scene"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no .scene files in {}", dir.display());
    for path in files {
        let source = fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("{}: {err}", path.display());
        });
        if let Err(err) = run_scene(&source) {
            panic!("{}: {err}", path.display());
        }
    }
}
