//! Path safety and atomic-write helpers shared by `import` and `gen`.
//!
//! Every output Loopt writes is constrained to land under a specific
//! project-relative directory (`.loopt/workflows` for generated source,
//! `.github/workflows` for generated YAML) and must never follow a symlink
//! out of the repository, and every write is temp-file-then-rename so a
//! crash mid-write never leaves a truncated file behind.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};

/// Rewrite a user-supplied path (which may be absolute, inside or outside
/// `root`) as a path relative to `root`, or reject it.
pub fn relativize(root: &Path, path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("resolve root {}", root.display()))?;
    let canonical = path
        .canonicalize()
        .with_context(|| format!("resolve {}", path.display()))?;
    canonical
        .strip_prefix(&canonical_root)
        .map(Path::to_path_buf)
        .with_context(|| {
            format!(
                "{}: outside repository root {}",
                path.display(),
                root.display()
            )
        })
}

/// Confirm `relative` (already root-relative) cannot escape `root` through
/// `..`, drive/root components, or a symlink anywhere along its existing
/// ancestor chain, and return the joined absolute-ish path (`root` joined
/// with `relative`).
pub fn resolve_within(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.is_absolute() {
        bail!("{}: must be a relative path", relative.display());
    }
    for component in relative.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            other => bail!("{}: unsafe path component {other:?}", relative.display()),
        }
    }
    let joined = root.join(relative);
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("resolve root {}", root.display()))?;

    // Walk up to the nearest path that exists. Use symlink_metadata rather
    // than `exists`: a dangling symlink is itself an existing path and must
    // not be silently skipped as though it were a missing file.
    let mut existing_ancestor = joined.as_path();
    loop {
        match std::fs::symlink_metadata(existing_ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                existing_ancestor = existing_ancestor
                    .parent()
                    .with_context(|| format!("resolve {}", existing_ancestor.display()))?;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("resolve {}", existing_ancestor.display()));
            }
        }
    }
    let canonical_existing = existing_ancestor
        .canonicalize()
        .with_context(|| format!("resolve {}", existing_ancestor.display()))?;
    if !canonical_existing.starts_with(&canonical_root) {
        bail!(
            "{}: resolves outside of {} (symlink escape)",
            joined.display(),
            root.display()
        );
    }
    Ok(joined)
}

/// Project-relative root for vendored, cross-project shared authoring
/// SDK sources (currently `.loopt/sdk/rust` and `.loopt/sdk/go`, written
/// once by the Rust and Go language backends and reused -- via relative
/// `Cargo.toml` / `go.mod` dependencies -- by every generated project of
/// that language, instead of being duplicated under each project's own
/// `.loopt/workflows/<name>`). A second boundary alongside
/// `.loopt/workflows`, not a replacement for it: a language backend's
/// ordinary per-project files still must stay under `.loopt/workflows`.
pub const SDK_DIR: &str = ".loopt/sdk";

/// Confirm root-relative `path` stays under `root/prefix`, e.g.
/// `.loopt/workflows` or `.github/workflows`.
pub fn require_under(root: &Path, prefix: &str, path: &Path) -> Result<()> {
    let boundary = root.join(prefix);
    let joined = root.join(path);
    if !joined.starts_with(&boundary) {
        bail!("{}: must stay under {}", path.display(), boundary.display());
    }
    Ok(())
}
/// Return whether `path` names GitHub's conventional action metadata file.
///
/// Action metadata is identified by its filename, not by a fixed parent
/// directory: repository-local actions may live at the repository root,
/// under `.github/actions/<name>`, or another in-repository directory.
pub fn is_action_path(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some("action.yml") | Some("action.yaml")
    )
}

/// Require an action output to use an action metadata filename.
pub fn require_action_path(path: &Path) -> Result<()> {
    if !is_action_path(path) {
        bail!(
            "{}: action output must be named action.yml or action.yaml",
            path.display()
        );
    }
    Ok(())
}

/// Derive the relative source stem for an imported document.
///
/// Workflows retain the historical `<filename stem>` layout. Actions are
/// rooted under `actions/` and include their parent path so a root
/// `action.yml`, a workflow called `action.yml`, and actions with the same
/// basename can all be imported without one source overwriting another.
pub fn source_stem_for(path: &Path, action: bool) -> Result<PathBuf> {
    let stem = stem_for(path)?;
    if !action {
        return Ok(PathBuf::from(stem));
    }

    let mut source = PathBuf::from("actions");
    let mut parent_components = Vec::new();
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            match component {
                Component::Normal(name) => parent_components.push(name.to_owned()),
                Component::CurDir => {}
                other => bail!(
                    "{}: unsafe action parent component {other:?}",
                    path.display()
                ),
            }
        }
    }

    // Keep the conventional `.github/actions/<name>/action.*` layout short,
    // while retaining every component for other valid in-repository action
    // paths. A root action gets its own namespace as well.
    if parent_components.starts_with(&[
        std::ffi::OsString::from(".github"),
        std::ffi::OsString::from("actions"),
    ]) {
        parent_components.drain(..2);
    }
    if parent_components.is_empty() {
        source.push("root");
    } else {
        for component in parent_components {
            source.push(component);
        }
    }

    // `action.yml` and `action.yaml` may legally coexist on disk while
    // referring to different manifest outputs; keep their source paths
    // distinct rather than silently sharing one generated file.
    let source_name = match path.extension().and_then(|extension| extension.to_str()) {
        Some("yaml") => format!("{stem}-yaml"),
        _ => stem,
    };
    source.push(source_name);
    Ok(source)
}

/// Derive a safe single-component workflow stem from a YAML file's name
/// (its filename minus extension), rejecting anything that could act as a
/// path when joined under `.loopt/workflows/<stem>`.
pub fn stem_for(yaml_path: &Path) -> Result<String> {
    let stem = yaml_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .with_context(|| format!("{}: cannot derive a workflow stem", yaml_path.display()))?;
    if stem.contains('/') || stem.contains('\\') || stem == "." || stem == ".." {
        bail!("{}: unsafe workflow stem {stem:?}", yaml_path.display());
    }
    Ok(stem.to_string())
}

/// Keep Loopt's build cache out of version control.
///
/// `.loopt/.cache` holds compiled SDK artifacts (a Rust `target` directory
/// reaches ~100 MB). Appending the ignore rule to the repository's
/// `.gitignore` is idempotent and never rewrites existing content; a missing
/// `.gitignore` is created with just this rule.
pub fn ensure_gitignore(root: &Path) -> Result<()> {
    const RULE: &str = ".loopt/.cache/";
    let path = root.join(".gitignore");
    let existing = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("read {}", path.display())),
    };
    if existing
        .lines()
        .any(|line| line.trim() == RULE || line.trim() == ".loopt/.cache")
    {
        return Ok(());
    }
    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    updated.push_str(RULE);
    updated.push('\n');
    std::fs::write(&path, updated).with_context(|| format!("update {}", path.display()))?;
    Ok(())
}

/// Write `content` to `relative` atomically under `root`: write to a sibling
/// temp file, fsync it, then rename over the destination. Never leaves a
/// partially written file at the destination.
///
/// The root-relative path is resolved and checked again immediately before
/// writing. Parent symlinks are rejected so a caller's earlier preflight
/// cannot be bypassed by redirecting the directory before the write.
pub fn write_atomic(root: &Path, relative: &Path, content: &[u8]) -> Result<()> {
    static NEXT_TEMP_ID: AtomicU64 = AtomicU64::new(0);

    let destination = resolve_within(root, relative)?;
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    reject_symlink_components(root)?;
    reject_symlink_components(parent)?;
    std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    // Re-check after creating missing directories and use the resolved
    // directory for every operation below.
    reject_symlink_components(root)?;
    reject_symlink_components(parent)?;
    let actual_parent = parent
        .canonicalize()
        .with_context(|| format!("resolve {}", parent.display()))?;
    if !actual_parent.is_dir() {
        bail!("{}: not a directory", parent.display());
    }

    let file_name = destination
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{}: not a file path", relative.display()))?;
    let destination = actual_parent.join(file_name);
    if let Ok(metadata) = std::fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() {
            bail!(
                "{}: refusing to replace symlink destination",
                relative.display()
            );
        }
    }

    let process_id = std::process::id();
    let mut temp_path = None;
    let mut file = None;
    for _ in 0..1000 {
        let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
        let candidate = actual_parent.join(format!(".{file_name}.loopt-tmp-{process_id}-{id}"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(created) => {
                temp_path = Some(candidate);
                file = Some(created);
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| format!("create {}", candidate.display()));
            }
        }
    }
    let temp_path = temp_path.with_context(|| {
        format!(
            "create a unique temporary file beside {}",
            destination.display()
        )
    })?;
    let mut file = file.expect("temporary path and file are created together");

    let result = (|| -> Result<()> {
        file.write_all(content)
            .with_context(|| format!("write {}", temp_path.display()))?;
        file.sync_all()
            .with_context(|| format!("sync {}", temp_path.display()))?;
        drop(file);
        std::fs::rename(&temp_path, &destination).with_context(|| {
            format!(
                "rename {} to {}",
                temp_path.display(),
                destination.display()
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        // Only this invocation's create_new path is ever removed.
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

/// Reject symlinked components before a write. The root-relative check above
/// enforces the repository boundary; this check closes the preflight/write
/// gap for parent redirection.
fn reject_symlink_components(path: &Path) -> Result<()> {
    let mut current = if path.is_absolute() {
        PathBuf::new()
    } else {
        std::env::current_dir().context("resolve current directory")?
    };
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => current.push(prefix.as_os_str()),
            Component::RootDir => current.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::CurDir => {}
            Component::ParentDir => {
                current.pop();
            }
            Component::Normal(name) => {
                current.push(name);
                match std::fs::symlink_metadata(&current) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        bail!("{}: symlinked parent directory", path.display());
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("inspect {}", current.display()));
                    }
                }
            }
        }
    }
    Ok(())
}
