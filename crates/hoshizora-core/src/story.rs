//! Condensed, human-readable flows.

use hoshizora_model::api::Story;

use crate::Index;

impl Index {
    /// What `from` does, as a short sequence of steps grouped by process.
    pub fn story(&self, from: &str, depth: u32) -> Option<Story> {
        let _ = depth;
        self.node(from)?;
        Some(Story {
            root: from.to_string(),
            ..Story::default()
        })
    }
}
