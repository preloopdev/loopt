//! `loopt import`: parse existing workflow YAML with the shared authoring
//! API, emit native per-language source, and record the mapping in
//! `.loopt/manifest.json`. Never modifies the original YAML.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use preloop_gha_authoring::WorkflowDocument;

use crate::languages::{GeneratedFile, GeneratedSource, Language};
use crate::manifest::{self, DocumentKind, Manifest, ManifestEntry};
use crate::paths;
use crate::plugin::PluginSpec;

const WORKFLOWS_DIR: &str = ".github/workflows";
const SOURCE_DIR: &str = ".loopt/workflows";

struct YamlInput {
    path: PathBuf,
    document_kind: DocumentKind,
}

pub struct ImportOptions {
    pub paths: Vec<PathBuf>,
    pub language: String,
    pub force: bool,
    pub plugin: Option<PluginSpec>,
}

struct Planned {
    yaml_path: PathBuf,
    generated: GeneratedSource,
    /// Fingerprint of all generated destinations observed during planning.
    original_fingerprint: Option<String>,
    entry: ManifestEntry,
}

pub fn run(root: &Path, options: ImportOptions) -> Result<()> {
    let yaml_inputs = discover_yaml_inputs(root, &options.paths)?;
    if yaml_inputs.is_empty() {
        bail!(
            "no workflow or action YAML to import (pass paths, or add files under {WORKFLOWS_DIR})"
        );
    }

    let (language, plugin) = resolve_language(root, &options)?;
    let mut manifest = Manifest::load(root)?;

    // Phase 1: build the whole write plan before touching disk, so a
    // failure partway through never leaves some sources written and others
    // not.
    let mut planned: Vec<Planned> = Vec::new();
    let mut seen_destinations: BTreeMap<PathBuf, String> = BTreeMap::new();

    for input in &yaml_inputs {
        let yaml_path = &input.path;
        let yaml_text = std::fs::read_to_string(root.join(yaml_path))
            .with_context(|| format!("read {}", yaml_path.display()))?;
        let document = WorkflowDocument::parse(&yaml_text)
            .with_context(|| format!("{}: not valid workflow/action YAML", yaml_path.display()))?;

        let stem = paths::source_stem_for(yaml_path, input.document_kind == DocumentKind::Action)?;
        let stem_path = PathBuf::from(SOURCE_DIR).join(&stem);

        let generated = match &plugin {
            Some(plugin) => plugin
                .emit(root, document.value(), &stem_path)
                .with_context(|| format!("{}: plugin emit failed", yaml_path.display()))?,
            None => language
                .expect("language or plugin resolved")
                .emit(document.value(), &stem_path)
                .with_context(|| format!("{}: emit failed", yaml_path.display()))?,
        };

        if !generated
            .files
            .iter()
            .any(|file| file.path == generated.entrypoint)
        {
            bail!(
                "{}: emitted files do not include the declared entrypoint {}",
                yaml_path.display(),
                generated.entrypoint.display()
            );
        }
        let original_fingerprint = read_fingerprint(root, &generated)?;
        for file in &generated.files {
            let under_source = paths::require_under(root, SOURCE_DIR, &file.path).is_ok();
            let under_sdk = paths::require_under(root, paths::SDK_DIR, &file.path).is_ok();
            if !under_source && !under_sdk {
                bail!(
                    "{}: unsafe generated path {} (must stay under {} or {})",
                    yaml_path.display(),
                    file.path.display(),
                    SOURCE_DIR,
                    paths::SDK_DIR
                );
            }
            paths::resolve_within(root, &file.path)
                .with_context(|| format!("{}: unsafe generated path", file.path.display()))?;
            match seen_destinations.get(&file.path) {
                // A vendored shared-SDK file (see `paths::SDK_DIR`) is
                // identical, byte-for-byte, across every document of its
                // language, so a repeat of one already seen in this run is
                // expected, not a conflict.
                Some(existing_content) if existing_content == &file.content => {}
                Some(_) => bail!(
                    "{}: duplicate authoring source destination {} with different content",
                    yaml_path.display(),
                    file.path.display()
                ),
                None => {
                    seen_destinations.insert(file.path.clone(), file.content.clone());
                }
            }
            if manifest
                .entries
                .iter()
                .any(|entry| entry.source == file.path && entry.source != generated.entrypoint)
            {
                bail!(
                    "{}: generated file {} collides with another manifest source",
                    yaml_path.display(),
                    file.path.display()
                );
            }
        }
        if manifest
            .entries
            .iter()
            .any(|entry| entry.source == generated.entrypoint && entry.output != *yaml_path)
        {
            bail!(
                "{}: generated entrypoint {} is already assigned to another document",
                yaml_path.display(),
                generated.entrypoint.display()
            );
        }
        if manifest
            .entries
            .iter()
            .any(|entry| entry.output == *yaml_path && entry.source != generated.entrypoint)
        {
            bail!(
                "{}: document is already assigned to another manifest source",
                yaml_path.display()
            );
        }
        // The fingerprint above is captured before the overwrite check.

        // Refuse to clobber hand-edited source: only overwrite files that
        // either don't exist yet, or exactly match what Loopt last generated
        // at this destination.
        let existing_entry = manifest.entry_for_source(&generated.entrypoint).cloned();
        if let Some(on_disk_fingerprint) = &original_fingerprint {
            let known_unedited = existing_entry
                .as_ref()
                .is_some_and(|entry| entry.source_fingerprint == *on_disk_fingerprint);
            if !known_unedited && !options.force {
                bail!(
                    "{}: authoring source already exists at {} and was not last generated by Loopt; pass --force to overwrite",
                    yaml_path.display(),
                    generated.entrypoint.display()
                );
            }
        }

        let fingerprint = manifest::fingerprint_files(
            document_specific_files(&generated.files)
                .map(|file| (file.path.as_path(), file.content.as_str())),
        );

        let entry = ManifestEntry {
            language: options.language.clone(),
            source: generated.entrypoint.clone(),
            output: yaml_path.clone(),
            document_kind: input.document_kind,
            last_accepted_yaml: yaml_text,
            source_fingerprint: fingerprint,
            plugin: plugin.clone(),
        };

        planned.push(Planned {
            yaml_path: yaml_path.clone(),
            generated,
            original_fingerprint,
            entry,
        });
    }

    // Recheck all generated destinations before the first write. This closes
    // the ordinary planning-to-commit edit window; an OS-level race can still
    // occur between this check and an individual atomic rename.
    if !options.force {
        for item in &planned {
            let current = read_fingerprint(root, &item.generated)?;
            if current != item.original_fingerprint {
                bail!(
                    "{}: generated source files changed on disk while import was in progress; refusing to overwrite",
                    item.generated.entrypoint.display()
                );
            }
        }
    }

    // Phase 2: commit. Each file write is itself atomic (temp + rename).
    for item in &planned {
        for file in &item.generated.files {
            paths::write_atomic(root, &file.path, file.content.as_bytes())
                .with_context(|| format!("write {}", file.path.display()))?;
        }
        manifest.upsert(item.entry.clone());
    }
    manifest.save(root)?;

    for item in &planned {
        println!(
            "imported {} -> {}",
            item.yaml_path.display(),
            item.generated.entrypoint.display()
        );
    }
    Ok(())
}

fn resolve_language(
    root: &Path,
    options: &ImportOptions,
) -> Result<(Option<Language>, Option<PluginSpec>)> {
    match &options.plugin {
        Some(plugin) => {
            if Language::parse(&options.language).is_some() {
                bail!(
                    "--plugin is only for a custom --language; {:?} is a built-in language",
                    options.language
                );
            }
            let capabilities = plugin
                .describe(root)
                .with_context(|| format!("validate plugin {}", plugin.program))?;
            if capabilities.language != options.language {
                bail!(
                    "plugin {} describes itself as language {:?}, not {:?}",
                    plugin.program,
                    capabilities.language,
                    options.language
                );
            }
            Ok((None, Some(plugin.clone())))
        }
        None => {
            let language = Language::parse(&options.language).with_context(|| {
                format!(
                    "{:?} is not a built-in language (typescript, python, go, rust); pass --plugin for a custom language",
                    options.language
                )
            })?;
            Ok((Some(language), None))
        }
    }
}

/// The files of `generated` specific to one document: every file except a
/// vendored shared-SDK file under `paths::SDK_DIR` (identical across every
/// document of its language, so it is irrelevant to whether *this*
/// document's own source was hand-edited -- fingerprinting it would wrongly
/// treat another document's already-vendored copy as evidence this brand
/// new document was already generated, or mark every sibling document's
/// source "edited" the moment the vendored copy itself changes between
/// `loopt` versions).
fn document_specific_files(files: &[GeneratedFile]) -> impl Iterator<Item = &GeneratedFile> {
    files
        .iter()
        .filter(|file| !file.path.starts_with(paths::SDK_DIR))
}

/// SHA-256 fingerprint of whatever currently exists on disk at `generated`'s
/// document-specific file paths, or `None` if none of them exist yet (a
/// first-time import).
fn read_fingerprint(root: &Path, generated: &GeneratedSource) -> Result<Option<String>> {
    let mut pairs: Vec<(PathBuf, String)> = Vec::new();
    let mut any_exists = false;
    for file in document_specific_files(&generated.files) {
        let dest = root.join(&file.path);
        if dest.exists() && !dest.is_file() {
            bail!(
                "{}: generated destination exists but is not a regular file",
                file.path.display()
            );
        }
        if dest.is_file() {
            any_exists = true;
            let content = std::fs::read_to_string(&dest)
                .with_context(|| format!("read {}", dest.display()))?;
            pairs.push((file.path.clone(), content));
        } else {
            pairs.push((file.path.clone(), String::new()));
        }
    }
    if !any_exists {
        return Ok(None);
    }
    Ok(Some(manifest::fingerprint_files(pairs.iter().map(
        |(path, content)| (path.as_path(), content.as_str()),
    ))))
}

fn discover_yaml_inputs(root: &Path, explicit: &[PathBuf]) -> Result<Vec<YamlInput>> {
    if !explicit.is_empty() {
        let mut found = Vec::new();
        for path in explicit {
            let relative = paths::relativize(root, path)?;
            paths::resolve_within(root, &relative)
                .with_context(|| format!("{}: unsafe path", relative.display()))?;
            if !root.join(&relative).is_file() {
                bail!("{}: no such file", relative.display());
            }
            // Files inside `.github/workflows/` are workflows even when they
            // happen to be named action.yml; GitHub applies the same rule.
            let document_kind = if relative.starts_with(WORKFLOWS_DIR) {
                DocumentKind::Workflow
            } else if paths::is_action_path(&relative) {
                DocumentKind::Action
            } else {
                DocumentKind::Workflow
            };
            found.push(YamlInput {
                path: relative,
                document_kind,
            });
        }
        found.sort_by(|left, right| left.path.cmp(&right.path));
        found.dedup_by(|left, right| left.path == right.path);
        return Ok(found);
    }

    let dir = root.join(WORKFLOWS_DIR);
    let mut found = Vec::new();
    if dir.is_dir() {
        for entry in std::fs::read_dir(&dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry.with_context(|| format!("read {}", dir.display()))?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            if matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml") | Some("yaml")
            ) {
                found.push(YamlInput {
                    path: PathBuf::from(WORKFLOWS_DIR).join(entry.file_name()),
                    document_kind: DocumentKind::Workflow,
                });
            }
        }
    }
    found.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(found)
}
