use std::path::Path;

use hoshizora_model::FileFacts;

/// Walks `root` (respecting .gitignore, skipping vendored/build dirs) and parses every supported file.
pub fn index_tree(root: &Path) -> anyhow::Result<Vec<FileFacts>> {
    let _ = root;
    unimplemented!("lane A")
}
