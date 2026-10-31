//! `.loopt/manifest.json`: the durable map from authored source to
//! generated YAML output that lets `loopt gen` run without re-stating
//! `loopt import`'s choices, and lets both commands refuse to clobber work
//! they did not write.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::paths;
use crate::plugin::PluginSpec;

pub const MANIFEST_VERSION: u32 = 1;
pub const MANIFEST_PATH: &str = ".loopt/manifest.json";

/// One tracked source/output pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// Language id: one of the four built-ins, or a custom plugin's id.
    pub language: String,
    /// Project-relative path to the authoring source entrypoint.
    pub source: PathBuf,
    /// Project-relative path to the generated workflow YAML.
    pub output: PathBuf,
    /// The exact YAML text Loopt last wrote (or accepted as already
    /// matching) at `output`. Lets `gen` detect edits made outside Loopt
    /// before it would overwrite the file, and skip a no-op rewrite.
    pub last_accepted_yaml: String,
    /// SHA-256 over every generated file's path and content, in file-list
    /// order. Lets `import` detect hand-edits to `source` before it would
    /// overwrite them. Empty for a source that was authored from scratch
    /// (never produced by `import`'s `emit`).
    #[serde(default)]
    pub source_fingerprint: String,
    /// Set only for custom (non-built-in) languages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plugin: Option<PluginSpec>,
}

/// The whole manifest file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<ManifestEntry>,
}

impl Manifest {
    /// Load `.loopt/manifest.json` under `root`, or an empty manifest if it
    /// does not exist yet.
    pub fn load(root: &Path) -> Result<Manifest> {
        let path = paths::resolve_within(root, Path::new(MANIFEST_PATH))
            .context("resolve .loopt/manifest.json")?;
        if !path.is_file() {
            return Ok(Manifest {
                version: MANIFEST_VERSION,
                entries: Vec::new(),
            });
        }
        let text =
            std::fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let manifest: Manifest =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        if manifest.version != MANIFEST_VERSION {
            bail!(
                "{}: unsupported manifest version {} (expected {MANIFEST_VERSION})",
                path.display(),
                manifest.version
            );
        }
        Ok(manifest)
    }

    /// Write the manifest atomically.
    pub fn save(&self, root: &Path) -> Result<()> {
        let mut text =
            serde_json::to_string_pretty(self).context("serialize .loopt/manifest.json")?;
        text.push('\n');
        paths::write_atomic(root, Path::new(MANIFEST_PATH), text.as_bytes())
            .context("write .loopt/manifest.json")
    }

    pub fn entry_for_source(&self, source: &Path) -> Option<&ManifestEntry> {
        self.entries.iter().find(|entry| entry.source == source)
    }

    /// Insert `entry`, replacing any existing entry for the same source.
    pub fn upsert(&mut self, entry: ManifestEntry) {
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.source == entry.source)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }
}

/// SHA-256 over an ordered set of (path, content) pairs, used to fingerprint
/// a generated source tree so a later run can tell it apart from a hand
/// edit.
pub fn fingerprint_files<'a>(files: impl IntoIterator<Item = (&'a Path, &'a str)>) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (path, content) in files {
        hasher.update(path.to_string_lossy().as_bytes());
        hasher.update([0u8]);
        hasher.update(content.as_bytes());
        hasher.update([0u8]);
    }
    format!("{:x}", hasher.finalize())
}
