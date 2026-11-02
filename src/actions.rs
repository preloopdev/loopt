//! Typed, commit-pinned bindings for GitHub Actions.
//!
//! The public entry point intentionally accepts the trailing arguments after
//! `loopt actions`; the top-level CLI owns argument parsing and dispatch.  The
//! module keeps the network boundary small (`gh api` only), stores all binding
//! metadata in a deterministic lock file, and emits ordinary TypeScript that
//! calls the existing SDK `uses` helper.

use crate::gh;
use crate::paths;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use preloop_gha_authoring::WorkflowDocument;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

const LOCK_PATH: &str = ".loopt/actions.lock";
const ACTIONS_DIR: &str = ".loopt/actions";
/// Default import for generated bindings under `.loopt/actions/` — the
/// vendored SDK at `.loopt/sdk/typescript`, one level up.
const DEFAULT_SDK_IMPORT: &str = "../sdk/typescript/index.ts";
const LOCK_VERSION: u32 = 1;

/// Run one `loopt actions` command.
///
/// The caller passes only the arguments following the `actions` command.  The
/// supported subcommands are `add`, `update`, `list`, `remove`, and `verify`.
/// Network operations use the authenticated `gh` executable directly (never a
/// shell), while `list`, `remove`, and `verify` are entirely local.
pub fn run(root: &Path, args: &[String]) -> Result<()> {
    if !root.is_dir() {
        bail!("{}: repository root is not a directory", root.display());
    }
    let Some(command) = args.first().map(String::as_str) else {
        print_usage();
        bail!("missing actions subcommand; expected add, update, list, remove, or verify");
    };
    if command == "--help" || command == "-h" {
        print_usage();
        return Ok(());
    }

    match command {
        "add" => add(root, &args[1..]),
        "update" => update(root, &args[1..]),
        "list" => list(root, &args[1..]),
        "remove" => remove(root, &args[1..]),
        "verify" => verify(root, &args[1..]),
        other => {
            print_usage();
            bail!("unknown actions subcommand {other:?}");
        }
    }
}

fn print_usage() {
    println!(
        "Usage: loopt actions <add|update|list|remove|verify> [options]\n\n\
         add SPEC [--name NAME] [--sdk-import MODULE] [--input-type KEY=TYPE] [--output NAME]\n\
         update NAME|SPEC [the same binding options]\n\
         list [--json]\n\
         remove NAME [--force]\n\
         verify [NAME]"
    );
}

#[derive(Debug, Clone, Default)]
struct ActionOptions {
    target: Option<String>,
    name: Option<String>,
    sdk_import: Option<String>,
    input_types: BTreeMap<String, String>,
    additional_outputs: BTreeSet<String>,
    patch_files: Vec<String>,
    force: bool,
    json: bool,
}

fn add(root: &Path, args: &[String]) -> Result<()> {
    let options = parse_options(root, args, true, false)?;
    let spec = options
        .target
        .as_deref()
        .context("actions add requires an action SPEC")?;
    let (source, requested_ref) = parse_action_spec(spec)?;
    let name = options
        .name
        .clone()
        .unwrap_or_else(|| derive_binding_name(&source));
    validate_binding_name(&name)?;

    let mut lock = load_lock(root)?;
    if lock.entries.iter().any(|entry| entry.name == name) {
        bail!("action binding {name:?} already exists; use `actions update {name}`");
    }
    let patches = options_to_patches(&options);
    let resolved = fetch_action(root, &source, &requested_ref)?;
    let sdk_import = options
        .sdk_import
        .clone()
        .unwrap_or_else(|| DEFAULT_SDK_IMPORT.to_string());
    if options.sdk_import.is_none() {
        crate::sdk_ts::vendor(root)?;
    }
    let entry = build_entry(name, source, requested_ref, resolved, patches, sdk_import)?;
    write_new_entry(root, &mut lock, entry, options.force)
}

fn update(root: &Path, args: &[String]) -> Result<()> {
    let options = parse_options(root, args, true, false)?;
    let target = options
        .target
        .as_deref()
        .context("actions update requires an existing binding name or action SPEC")?;
    let mut lock = load_lock(root)?;

    let (index, source, requested_ref, name) = if looks_like_action_spec(target) {
        let (source, requested_ref) = parse_action_spec(target)?;
        let name = options
            .name
            .clone()
            .unwrap_or_else(|| derive_binding_name(&source));
        let index = lock
            .entries
            .iter()
            .position(|entry| entry.name == name)
            .with_context(|| {
                format!("no existing action binding named {name:?}; use `actions add` first")
            })?;
        (index, source, requested_ref, name)
    } else {
        validate_binding_name(target)?;
        let index = lock
            .entries
            .iter()
            .position(|entry| entry.name == target)
            .with_context(|| format!("no action binding named {target:?}"))?;
        let entry = &lock.entries[index];
        if let Some(name) = options.name.as_deref() {
            if name != entry.name {
                bail!("actions update cannot rename {target:?} to {name:?}; use remove and add");
            }
        }
        (
            index,
            parse_source(&entry.source)?,
            entry.requested_ref.clone(),
            entry.name.clone(),
        )
    };

    let old = lock.entries[index].clone();
    ensure_generated_replace_allowed(
        root,
        &old.generated,
        Some(&old.generated_sha256),
        options.force,
    )?;
    let patches = merge_patches(&old.patches, options_to_patches(&options));
    let resolved = fetch_action(root, &source, &requested_ref)?;
    let sdk_import = options
        .sdk_import
        .clone()
        .unwrap_or_else(|| old.sdk_import.clone());
    if sdk_import == DEFAULT_SDK_IMPORT {
        crate::sdk_ts::vendor(root)?;
    }
    let entry = build_entry(name, source, requested_ref, resolved, patches, sdk_import)?;

    let old_path = old.generated.clone();
    let new_path = entry.generated.clone();
    if old_path != new_path {
        bail!("action binding update would change its generated path; use remove and add");
    }
    write_generated(root, &entry, Some(&old.generated_sha256), options.force)?;
    lock.entries[index] = entry;
    normalize_lock(&mut lock);
    write_lock(root, &lock)
}

fn list(root: &Path, args: &[String]) -> Result<()> {
    let options = parse_options(root, args, false, true)?;
    let lock = load_lock(root)?;
    if options.json {
        println!("{}", serde_json::to_string_pretty(&lock)?);
        return Ok(());
    }
    if lock.entries.is_empty() {
        println!("no action bindings");
        return Ok(());
    }
    for entry in &lock.entries {
        println!(
            "{}\t{}@{}\t{}",
            entry.name, entry.source, entry.sha, entry.generated
        );
    }
    Ok(())
}

fn remove(root: &Path, args: &[String]) -> Result<()> {
    let options = parse_options(root, args, true, false)?;
    let target = options
        .target
        .as_deref()
        .context("actions remove requires a binding name")?;
    let mut lock = load_lock(root)?;
    let index = lock
        .entries
        .iter()
        .position(|entry| entry.name == target)
        .with_context(|| format!("no action binding named {target:?}"))?;
    let entry = lock.entries[index].clone();
    ensure_generated_replace_allowed(
        root,
        &entry.generated,
        Some(&entry.generated_sha256),
        options.force,
    )?;

    let generated = resolve_generated(root, &entry.generated)?;
    match fs::symlink_metadata(&generated) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("{}: refusing to remove symlink", entry.generated)
        }
        Ok(metadata) if metadata.is_dir() => {
            bail!("{}: generated binding path is a directory", entry.generated)
        }
        Ok(_) => {
            fs::remove_file(&generated)
                .with_context(|| format!("remove generated binding {}", entry.generated))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("inspect {}", entry.generated));
        }
    }

    lock.entries.remove(index);
    normalize_lock(&mut lock);
    write_lock(root, &lock)
}

fn verify(root: &Path, args: &[String]) -> Result<()> {
    let options = parse_options(root, args, false, false)?;
    let lock_bytes = read_lock_bytes(root)?;
    let lock = parse_lock(&lock_bytes)?;
    let canonical = serialize_lock(&lock)?;
    if lock_bytes != canonical.as_bytes() {
        bail!("{LOCK_PATH}: is not in deterministic format or was modified");
    }

    let selected = options.target.as_deref();
    let mut checked = 0usize;
    let mut failures = Vec::new();
    for entry in &lock.entries {
        if selected.is_some_and(|name| name != entry.name) {
            continue;
        }
        checked += 1;
        if let Err(error) = verify_entry(root, entry) {
            failures.push(format!("{}: {error}", entry.name));
        }
    }
    if let Some(name) = selected {
        if checked == 0 {
            bail!("no action binding named {name:?}");
        }
    }
    if !failures.is_empty() {
        bail!("actions verify failed:\n- {}", failures.join("\n- "));
    }
    println!("verified {checked} action binding(s) (offline)");
    Ok(())
}

fn parse_options(
    root: &Path,
    args: &[String],
    require_target: bool,
    allow_json: bool,
) -> Result<ActionOptions> {
    let mut options = ActionOptions::default();
    let mut index = 0usize;
    while index < args.len() {
        let raw = &args[index];
        if raw == "--" {
            index += 1;
            if index < args.len() {
                if options.target.is_some() {
                    bail!("unexpected extra positional argument {:?}", args[index]);
                }
                options.target = Some(args[index].clone());
                index += 1;
            }
            if index != args.len() {
                bail!("unexpected extra positional argument {:?}", args[index]);
            }
            break;
        }
        if raw == "-h" || raw == "--help" {
            print_usage();
            return Ok(options);
        }
        if let Some(flag) = raw.strip_prefix("--") {
            let (key, inline) = flag
                .split_once('=')
                .map_or((flag, None), |(key, value)| (key, Some(value.to_string())));
            match key {
                "force" => {
                    if inline.is_some() {
                        bail!("--force does not take a value");
                    }
                    options.force = true;
                }
                "json" if allow_json => {
                    if inline.is_some() {
                        bail!("--json does not take a value");
                    }
                    options.json = true;
                }
                "name" => {
                    let value = option_value(args, &mut index, inline, "--name")?;
                    validate_binding_name(&value)?;
                    options.name = Some(value);
                }
                "sdk-import" => {
                    let value = option_value(args, &mut index, inline, "--sdk-import")?;
                    validate_sdk_import(&value)?;
                    options.sdk_import = Some(value);
                }
                "input-type" | "input" | "type" => {
                    let value = option_value(args, &mut index, inline, "--input-type")?;
                    let (name, type_name) = parse_input_type_patch(&value)?;
                    insert_input_patch(&mut options.input_types, name, type_name)?;
                }
                "output" | "additional-output" | "extra-output" => {
                    let value = option_value(args, &mut index, inline, "--output")?;
                    options
                        .additional_outputs
                        .insert(parse_output_name(&value)?);
                }
                "patch" => {
                    let value = option_value(args, &mut index, inline, "--patch")?;
                    parse_patch_argument(root, &value, &mut options)?;
                }
                "patch-file" => {
                    let value = option_value(args, &mut index, inline, "--patch-file")?;
                    options.patch_files.push(value.clone());
                    let patch = read_patch_file(root, &value)?;
                    merge_cli_patch(&mut options, patch)?;
                }
                _ => bail!("unknown actions option --{key}"),
            }
        } else {
            if options.target.replace(raw.clone()).is_some() {
                bail!("unexpected extra positional argument {raw:?}");
            }
        }
        index += 1;
    }
    if require_target && options.target.is_none() {
        bail!("missing action binding target");
    }
    Ok(options)
}

fn option_value(
    args: &[String],
    index: &mut usize,
    inline: Option<String>,
    flag: &str,
) -> Result<String> {
    if let Some(value) = inline {
        if value.is_empty() {
            bail!("{flag} requires a non-empty value");
        }
        return Ok(value);
    }
    let next = index
        .checked_add(1)
        .and_then(|position| args.get(position))
        .with_context(|| format!("{flag} requires a value"))?;
    *index += 1;
    if next.is_empty() || next.starts_with('-') {
        bail!("{flag} requires a non-empty value");
    }
    Ok(next.clone())
}

fn parse_patch_argument(root: &Path, value: &str, options: &mut ActionOptions) -> Result<()> {
    if value.contains('=') || value.contains(':') {
        if let Ok((name, type_name)) = parse_input_type_patch(value) {
            insert_input_patch(&mut options.input_types, name, type_name)?;
            return Ok(());
        }
    }
    if value.trim_start().starts_with('{') {
        let patch =
            parse_patch_text(value.as_bytes()).context("parse inline action binding patch")?;
        return merge_cli_patch(options, patch);
    }
    // A plain --patch value is an output name.  A path is accepted as a
    // convenience and is read only after path safety checks.
    if looks_like_patch_path(root, value) {
        let patch = read_patch_file(root, value)?;
        options.patch_files.push(value.to_string());
        merge_cli_patch(options, patch)
    } else {
        options.additional_outputs.insert(parse_output_name(value)?);
        Ok(())
    }
}

fn looks_like_patch_path(root: &Path, value: &str) -> bool {
    let path = Path::new(value);
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| matches!(extension, "json" | "yaml" | "yml"))
        && paths::resolve_within(root, path).is_ok_and(|resolved| resolved.is_file())
}

fn options_to_patches(options: &ActionOptions) -> BindingPatches {
    BindingPatches {
        input_types: options.input_types.clone(),
        additional_outputs: options.additional_outputs.iter().cloned().collect(),
    }
}

fn merge_patches(base: &BindingPatches, overlay: BindingPatches) -> BindingPatches {
    let mut input_types = base.input_types.clone();
    input_types.extend(overlay.input_types);
    let mut outputs = base
        .additional_outputs
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    outputs.extend(overlay.additional_outputs);
    BindingPatches {
        input_types,
        additional_outputs: outputs.into_iter().collect(),
    }
}

fn merge_cli_patch(options: &mut ActionOptions, patch: BindingPatches) -> Result<()> {
    for (name, type_name) in patch.input_types {
        insert_input_patch(&mut options.input_types, name, type_name)?;
    }
    options.additional_outputs.extend(patch.additional_outputs);
    Ok(())
}

fn parse_input_type_patch(value: &str) -> Result<(String, String)> {
    let (name, type_name) = value
        .split_once('=')
        .or_else(|| value.split_once(':'))
        .context("input type patch must be KEY=string|number|boolean")?;
    let name = validate_metadata_key(name.trim(), "input")?;
    let type_name = normalize_input_type(type_name.trim())?;
    Ok((name, type_name))
}

fn insert_input_patch(
    patches: &mut BTreeMap<String, String>,
    name: String,
    type_name: String,
) -> Result<()> {
    if let Some(previous) = patches.get(&name) {
        if previous != &type_name {
            bail!("conflicting type patches for input {name:?}: {previous:?} and {type_name:?}");
        }
        return Ok(());
    }
    patches.insert(name, type_name);
    Ok(())
}

fn parse_output_name(value: &str) -> Result<String> {
    let value = value.split_once('=').map_or(value, |(name, _)| name).trim();
    validate_metadata_key(value, "output")
}

fn normalize_input_type(value: &str) -> Result<String> {
    let normalized = match value.to_ascii_lowercase().as_str() {
        "string" | "str" => "string",
        "number" | "int" | "integer" | "float" => "number",
        "boolean" | "bool" => "boolean",
        other => {
            bail!("unsupported action input type {other:?}; expected string, number, or boolean")
        }
    };
    Ok(normalized.to_string())
}

fn parse_patch_text(bytes: &[u8]) -> Result<BindingPatches> {
    if let Ok(value) = serde_json::from_slice::<JsonValue>(bytes) {
        return parse_json_patch(&value);
    }
    let source =
        String::from_utf8(bytes.to_vec()).context("action binding patch is not valid UTF-8")?;
    let document = WorkflowDocument::parse(&source)
        .map_err(|error| anyhow::anyhow!("parse YAML action binding patch: {error}"))?;
    parse_json_patch(document.value())
}

fn read_patch_file(root: &Path, path: &str) -> Result<BindingPatches> {
    let supplied = Path::new(path);
    let relative = if supplied.is_absolute() {
        paths::relativize(root, supplied)?
    } else {
        supplied.to_path_buf()
    };
    let resolved = paths::resolve_within(root, &relative)?;
    let metadata = fs::symlink_metadata(&resolved)
        .with_context(|| format!("inspect action binding patch {}", relative.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "{}: patch must be a regular file inside the repository",
            path
        );
    }
    let bytes = fs::read(&resolved).with_context(|| format!("read action binding patch {path}"))?;
    parse_patch_text(&bytes).with_context(|| format!("parse action binding patch {path}"))
}

fn parse_json_patch(value: &JsonValue) -> Result<BindingPatches> {
    let object = value
        .as_object()
        .context("action binding patch must be a JSON object")?;
    let mut patch = BindingPatches::default();
    for key in ["inputs", "inputTypes", "input_types"] {
        if let Some(inputs) = object.get(key) {
            let map = inputs
                .as_object()
                .with_context(|| format!("patch field {key:?} must be an object"))?;
            for (name, type_value) in map {
                let type_name = type_value
                    .as_str()
                    .with_context(|| format!("type patch for input {name:?} must be a string"))?;
                insert_input_patch(
                    &mut patch.input_types,
                    validate_metadata_key(name, "input")?,
                    normalize_input_type(type_name)?,
                )?;
            }
        }
    }
    for key in ["outputs", "additionalOutputs", "additional_outputs"] {
        if let Some(outputs) = object.get(key) {
            if let Some(sequence) = outputs.as_array() {
                for output in sequence {
                    patch.additional_outputs.push(parse_output_name(
                        output.as_str().context("patch output must be a string")?,
                    )?);
                }
            } else if let Some(map) = outputs.as_object() {
                for name in map.keys() {
                    patch
                        .additional_outputs
                        .push(validate_metadata_key(name, "output")?);
                }
            } else {
                bail!("patch field {key:?} must be an array or object");
            }
        }
    }
    Ok(patch)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct BindingPatches {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    input_types: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    additional_outputs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockFile {
    version: u32,
    #[serde(default)]
    entries: Vec<LockEntry>,
}

impl Default for LockFile {
    fn default() -> Self {
        Self {
            version: LOCK_VERSION,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockEntry {
    name: String,
    source: String,
    #[serde(rename = "ref")]
    requested_ref: String,
    sha: String,
    metadata: String,
    generated: String,
    #[serde(default = "default_sdk_import")]
    sdk_import: String,
    inputs: Vec<LockInput>,
    outputs: Vec<String>,
    #[serde(default)]
    patches: BindingPatches,
    generated_sha256: String,
}

fn default_sdk_import() -> String {
    DEFAULT_SDK_IMPORT.to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LockInput {
    name: String,
    required: bool,
    #[serde(rename = "type")]
    type_name: String,
}

#[derive(Debug, Clone)]
struct ActionSource {
    owner: String,
    repo: String,
    subpath: Vec<String>,
}

impl ActionSource {
    fn canonical(&self) -> String {
        let mut value = format!("{}/{}", self.owner, self.repo);
        for component in &self.subpath {
            value.push('/');
            value.push_str(component);
        }
        value
    }
}

#[derive(Debug, Clone)]
struct ActionMetadata {
    inputs: Vec<MetadataInput>,
    outputs: Vec<String>,
    metadata_file: String,
}

#[derive(Debug, Clone)]
struct MetadataInput {
    name: String,
    required: bool,
}

#[derive(Debug, Clone)]
struct ResolvedAction {
    sha: String,
    metadata: ActionMetadata,
}

fn build_entry(
    name: String,
    source: ActionSource,
    requested_ref: String,
    resolved: ResolvedAction,
    patches: BindingPatches,
    sdk_import: String,
) -> Result<LockEntry> {
    let mut inputs = resolved.metadata.inputs;
    for input in &mut inputs {
        if let Some(type_name) = patches.input_types.get(&input.name) {
            normalize_input_type(type_name)?;
        }
    }
    for patch_name in patches.input_types.keys() {
        if !inputs.iter().any(|input| input.name == *patch_name) {
            bail!("input type patch names undeclared action input {patch_name:?}");
        }
    }
    inputs.sort_by(|left, right| left.name.cmp(&right.name));
    let mut outputs = resolved
        .metadata
        .outputs
        .into_iter()
        .collect::<BTreeSet<_>>();
    outputs.extend(patches.additional_outputs.iter().cloned());
    let mut patches = patches;
    patches.additional_outputs = patches
        .additional_outputs
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let generated = generated_path(&name);
    let mut entry = LockEntry {
        name,
        source: source.canonical(),
        requested_ref,
        sha: resolved.sha,
        metadata: resolved.metadata.metadata_file,
        generated,
        sdk_import,
        inputs: inputs
            .into_iter()
            .map(|input| {
                let type_name = patches
                    .input_types
                    .get(&input.name)
                    .cloned()
                    .unwrap_or_else(|| "string".to_string());
                LockInput {
                    name: input.name,
                    required: input.required,
                    type_name,
                }
            })
            .collect(),
        outputs: outputs.into_iter().collect(),
        patches,
        generated_sha256: String::new(),
    };
    let content = render_binding(&entry);
    entry.generated_sha256 = sha256_hex(content.as_bytes());
    Ok(entry)
}

fn write_new_entry(root: &Path, lock: &mut LockFile, entry: LockEntry, force: bool) -> Result<()> {
    if lock
        .entries
        .iter()
        .any(|existing| existing.generated == entry.generated)
    {
        bail!(
            "generated binding path {} is already used by another action",
            entry.generated
        );
    }
    ensure_generated_replace_allowed(root, &entry.generated, None, force)?;
    write_generated(root, &entry, None, force)?;
    lock.entries.push(entry);
    normalize_lock(lock);
    write_lock(root, lock)
}

fn write_generated(
    root: &Path,
    entry: &LockEntry,
    prior_sha256: Option<&str>,
    force: bool,
) -> Result<()> {
    let relative = generated_relative(&entry.name)?;
    let content = render_binding(entry);
    let hash = sha256_hex(content.as_bytes());
    if hash != entry.generated_sha256 {
        bail!("internal error: generated binding hash is inconsistent");
    }
    ensure_generated_replace_allowed(root, &entry.generated, prior_sha256, force)?;
    paths::write_atomic(root, &relative, content.as_bytes())
        .with_context(|| format!("write generated action binding {}", relative.display()))
}

fn ensure_generated_replace_allowed(
    root: &Path,
    generated: &str,
    prior_sha256: Option<&str>,
    force: bool,
) -> Result<()> {
    let generated = resolve_generated(root, generated)?;
    let relative = generated
        .strip_prefix(root)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| generated.display().to_string());
    match fs::symlink_metadata(&generated) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("{relative}: refusing to replace symlink")
        }
        Ok(metadata) if metadata.is_dir() => {
            bail!("{relative}: generated binding path is a directory")
        }
        Ok(_) if force => Ok(()),
        Ok(_) => {
            let bytes = fs::read(&generated)
                .with_context(|| format!("read generated action binding {relative}"))?;
            let Some(prior) = prior_sha256 else {
                bail!(
                    "{relative}: refusing to overwrite an existing generated file without a recorded fingerprint; use --force",
                );
            };
            if prior.is_empty() {
                bail!(
                    "{relative}: refusing to overwrite an existing generated file without a recorded fingerprint; use --force",
                );
            }
            let actual = sha256_hex(&bytes);
            if actual != prior {
                bail!(
                    "{relative}: generated file was edited outside loopt; use --force to replace it",
                );
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {relative}")),
    }
}

fn verify_entry(root: &Path, entry: &LockEntry) -> Result<()> {
    validate_lock_entry(entry)?;
    let generated = resolve_generated(root, &entry.generated)?;
    let metadata = fs::symlink_metadata(&generated)
        .with_context(|| format!("generated file is missing: {}", entry.generated))?;
    if metadata.file_type().is_symlink() {
        bail!("generated file is a symlink");
    }
    if !metadata.is_file() {
        bail!("generated path is not a regular file");
    }
    let bytes = fs::read(&generated).with_context(|| format!("read {}", entry.generated))?;
    let actual = sha256_hex(&bytes);
    if actual != entry.generated_sha256 {
        bail!("generated file fingerprint differs from actions.lock");
    }
    let expected = render_binding(entry);
    if bytes != expected.as_bytes() {
        bail!("generated file content does not match its lock entry");
    }
    Ok(())
}

fn generated_path(name: &str) -> String {
    format!("{ACTIONS_DIR}/{name}.ts")
}

fn generated_relative(name: &str) -> Result<PathBuf> {
    let path = PathBuf::from(generated_path(name));
    if path
        .components()
        .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        bail!("unsafe generated binding path for {name:?}");
    }
    Ok(path)
}

fn resolve_generated(root: &Path, generated: &str) -> Result<PathBuf> {
    let relative = PathBuf::from(generated);
    paths::require_under(root, ACTIONS_DIR, &relative)?;
    paths::resolve_within(root, &relative)
}

fn load_lock(root: &Path) -> Result<LockFile> {
    match read_lock_bytes(root) {
        Ok(bytes) => parse_lock(&bytes),
        Err(error) if is_not_found(&error) => Ok(LockFile::default()),
        Err(error) => Err(error),
    }
}

fn read_lock_bytes(root: &Path) -> Result<Vec<u8>> {
    let relative = Path::new(LOCK_PATH);
    let resolved = paths::resolve_within(root, relative)?;
    let metadata = match fs::symlink_metadata(&resolved) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(error).with_context(|| format!("read {LOCK_PATH}"));
        }
        Err(error) => return Err(error).with_context(|| format!("inspect {LOCK_PATH}")),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{LOCK_PATH}: must be a regular file");
    }
    fs::read(&resolved).with_context(|| format!("read {LOCK_PATH}"))
}

fn is_not_found(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

fn parse_lock(bytes: &[u8]) -> Result<LockFile> {
    let mut lock: LockFile = serde_json::from_slice(bytes).context("parse .loopt/actions.lock")?;
    if lock.version != LOCK_VERSION {
        bail!(
            ".loopt/actions.lock: unsupported version {}; expected {}",
            lock.version,
            LOCK_VERSION
        );
    }
    normalize_lock(&mut lock);
    validate_lock(&lock)?;
    Ok(lock)
}

fn validate_lock(lock: &LockFile) -> Result<()> {
    let mut previous = None;
    for entry in &lock.entries {
        validate_lock_entry(entry)?;
        if let Some(previous) = previous {
            if previous >= entry.name.as_str() {
                bail!(".loopt/actions.lock: entries must be sorted by name");
            }
        }
        previous = Some(entry.name.as_str());
    }
    Ok(())
}

fn validate_lock_entry(entry: &LockEntry) -> Result<()> {
    validate_binding_name(&entry.name)?;
    let source = parse_source(&entry.source)?;
    let _ = source;
    gh::validate_ref(&entry.requested_ref)?;
    gh::validate_sha(&entry.sha)?;
    validate_sdk_import(&entry.sdk_import)?;
    if entry.metadata != "action.yml" && entry.metadata != "action.yaml" {
        bail!(
            "{}: unsupported metadata filename {:?}",
            entry.name,
            entry.metadata
        );
    }
    let expected_path = generated_path(&entry.name);
    if entry.generated != expected_path {
        bail!(
            "{}: generated path must be {expected_path:?}, got {:?}",
            entry.name,
            entry.generated
        );
    }
    if entry.generated_sha256.len() != 64
        || !entry
            .generated_sha256
            .chars()
            .all(|character| character.is_ascii_hexdigit())
        || entry.generated_sha256 != entry.generated_sha256.to_ascii_lowercase()
    {
        bail!("{}: invalid generated_sha256", entry.name);
    }
    let mut input_names = BTreeSet::new();
    let mut previous_input = None;
    for input in &entry.inputs {
        validate_metadata_key(&input.name, "input")?;
        let normalized_type = normalize_input_type(&input.type_name)?;
        if let Some(patched_type) = entry.patches.input_types.get(&input.name) {
            if normalize_input_type(patched_type)? != normalized_type {
                bail!(
                    "{}: input type patch disagrees with input {:?}",
                    entry.name,
                    input.name
                );
            }
        }
        if !input_names.insert(&input.name) {
            bail!("{}: duplicate input {:?}", entry.name, input.name);
        }
        if let Some(previous) = previous_input {
            if previous >= input.name.as_str() {
                bail!("{}: inputs must be sorted", entry.name);
            }
        }
        previous_input = Some(input.name.as_str());
    }
    let mut outputs = BTreeSet::new();
    let mut previous_output = None;
    for output in &entry.outputs {
        validate_metadata_key(output, "output")?;
        if !outputs.insert(output) {
            bail!("{}: duplicate output {:?}", entry.name, output);
        }
        if let Some(previous) = previous_output {
            if previous >= output.as_str() {
                bail!("{}: outputs must be sorted", entry.name);
            }
        }
        previous_output = Some(output.as_str());
    }
    for (name, type_name) in &entry.patches.input_types {
        if !entry.inputs.iter().any(|input| input.name == *name) {
            bail!("{}: patch names undeclared input {:?}", entry.name, name);
        }
        normalize_input_type(type_name)?;
    }
    if entry
        .patches
        .additional_outputs
        .windows(2)
        .any(|window| window[0] >= window[1])
    {
        bail!(
            "{}: additional outputs must be sorted and unique",
            entry.name
        );
    }
    for output in &entry.patches.additional_outputs {
        validate_metadata_key(output, "output")?;
        if !entry.outputs.contains(output) {
            bail!(
                "{}: additional output {:?} is not in outputs",
                entry.name,
                output
            );
        }
    }
    Ok(())
}

fn normalize_lock(lock: &mut LockFile) {
    lock.version = LOCK_VERSION;
    for entry in &mut lock.entries {
        entry
            .inputs
            .sort_by(|left, right| left.name.cmp(&right.name));
        entry.outputs.sort();
        entry.patches.additional_outputs.sort();
    }
    lock.entries
        .sort_by(|left, right| left.name.cmp(&right.name));
}

fn serialize_lock(lock: &LockFile) -> Result<String> {
    let mut normalized = lock.clone();
    normalize_lock(&mut normalized);
    validate_lock(&normalized)?;
    let mut output = serde_json::to_string_pretty(&normalized)?;
    output.push('\n');
    Ok(output)
}

fn write_lock(root: &Path, lock: &LockFile) -> Result<()> {
    let content = serialize_lock(lock)?;
    paths::write_atomic(root, Path::new(LOCK_PATH), content.as_bytes())
        .with_context(|| format!("write {LOCK_PATH}"))
}

fn parse_action_spec(spec: &str) -> Result<(ActionSource, String)> {
    let (source, requested_ref) = spec
        .rsplit_once('@')
        .context("action spec must be OWNER/REPO[/SUBPATH]@REF")?;
    let source = parse_source(source)?;
    gh::validate_ref(requested_ref)?;

    Ok((source, requested_ref.to_string()))
}

fn parse_source(source: &str) -> Result<ActionSource> {
    if source.is_empty() || source.contains('@') {
        bail!("action source must be OWNER/REPO[/SUBPATH]");
    }
    let components = source.split('/').collect::<Vec<_>>();
    if components.len() < 2 {
        bail!("action source must include OWNER/REPO");
    }
    validate_repo_component(components[0], "owner")?;
    validate_repo_component(components[1], "repository")?;
    let mut subpath = Vec::new();
    for component in &components[2..] {
        if component.is_empty() || *component == "." || *component == ".." {
            bail!("action source contains an unsafe subpath component");
        }
        validate_metadata_key(component, "action subpath")?;
        subpath.push((*component).to_string());
    }
    Ok(ActionSource {
        owner: components[0].to_string(),
        repo: components[1].to_string(),
        subpath,
    })
}

fn validate_repo_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        bail!("invalid GitHub {label} component {value:?}");
    }
    Ok(())
}

fn validate_metadata_key(value: &str, kind: &str) -> Result<String> {
    if value.is_empty()
        || value.chars().any(|character| character.is_ascii_control())
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
    {
        bail!("invalid {kind} name {value:?}");
    }
    Ok(value.to_string())
}

fn validate_binding_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.starts_with('.')
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || !value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        bail!("invalid action binding name {value:?}");
    }
    Ok(())
}

pub(crate) fn validate_sdk_import(value: &str) -> Result<()> {
    if value.is_empty()
        || value.chars().any(|character| character.is_ascii_control())
        || value.contains('"')
        || value.contains('\\')
        || value.contains('\n')
        || value.contains('\r')
    {
        bail!("invalid TypeScript SDK import specifier");
    }
    Ok(())
}

fn derive_binding_name(source: &ActionSource) -> String {
    source
        .subpath
        .last()
        .cloned()
        .unwrap_or_else(|| source.repo.clone())
}

fn looks_like_action_spec(value: &str) -> bool {
    value.contains('@') && value.contains('/')
}

fn fetch_action(root: &Path, source: &ActionSource, requested_ref: &str) -> Result<ResolvedAction> {
    let sha = gh::resolve_commit(root, &source.owner, &source.repo, requested_ref).with_context(
        || {
            format!(
                "resolve {}@{} to a full commit SHA",
                source.canonical(),
                requested_ref
            )
        },
    )?;
    let metadata_path = source
        .subpath
        .iter()
        .fold(String::new(), |mut path, component| {
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(component);
            path
        });

    let mut failures = Vec::new();
    for metadata_file in ["action.yml", "action.yaml"] {
        let path = if metadata_path.is_empty() {
            metadata_file.to_string()
        } else {
            format!("{metadata_path}/{metadata_file}")
        };
        match gh::fetch_file(root, &source.owner, &source.repo, &path, &sha)
            .and_then(|bytes| parse_action_metadata(&bytes, metadata_file))
        {
            Ok(metadata) => {
                return Ok(ResolvedAction { sha, metadata });
            }
            Err(error) => failures.push(format!("{metadata_file}: {error}")),
        }
    }
    bail!(
        "could not retrieve a valid action metadata file for {}@{}:\n- {}",
        source.canonical(),
        sha,
        failures.join("\n- ")
    )
}

fn parse_action_metadata(bytes: &[u8], metadata_file: &str) -> Result<ActionMetadata> {
    let source = String::from_utf8(bytes.to_vec())
        .with_context(|| format!("{metadata_file} is not valid UTF-8"))?;
    let document = WorkflowDocument::parse(&source)
        .map_err(|error| anyhow::anyhow!("parse {metadata_file} as YAML: {error}"))?;
    let root = document.value();
    if !root.is_object() {
        bail!("action metadata root must be a YAML mapping");
    }
    let name = root
        .get("name")
        .and_then(JsonValue::as_str)
        .context("action metadata requires a string `name`")?;
    if name.trim().is_empty() {
        bail!("action metadata `name` must not be empty");
    }
    let description = root
        .get("description")
        .and_then(JsonValue::as_str)
        .context("action metadata requires a string `description`")?;
    if description.trim().is_empty() {
        bail!("action metadata `description` must not be empty");
    }
    let runs = root
        .get("runs")
        .and_then(JsonValue::as_object)
        .context("action metadata requires a `runs` mapping")?;
    let using = runs
        .get("using")
        .and_then(JsonValue::as_str)
        .context("action metadata `runs.using` must be a string")?;
    match using {
        "node12" | "node16" | "node20" | "node24" => {
            if runs.get("main").and_then(JsonValue::as_str).is_none() {
                bail!("node action metadata requires runs.main");
            }
        }
        "docker" => {
            if runs.get("image").and_then(JsonValue::as_str).is_none() {
                bail!("docker action metadata requires runs.image");
            }
        }
        "composite" => {
            if !matches!(runs.get("steps"), Some(JsonValue::Array(_))) {
                bail!("composite action metadata requires runs.steps");
            }
        }
        other => bail!("unsupported action runtime {other:?} in runs.using"),
    }

    let mut inputs = Vec::new();
    if let Some(value) = root.get("inputs") {
        let map = value
            .as_object()
            .context("action metadata `inputs` must be a mapping")?;
        for (key, value) in map {
            let key = validate_metadata_key(key, "input")?;
            let definition = value
                .as_object()
                .with_context(|| format!("action input {key:?} must be a mapping"))?;
            let required = match definition.get("required") {
                None => false,
                Some(JsonValue::Bool(value)) => *value,
                Some(JsonValue::String(value)) if value.eq_ignore_ascii_case("true") => true,
                Some(JsonValue::String(value)) if value.eq_ignore_ascii_case("false") => false,
                Some(_) => bail!("action input {key:?} has non-boolean `required`"),
            };
            inputs.push(MetadataInput {
                name: key,
                required,
            });
        }
    }

    let mut outputs = Vec::new();
    if let Some(value) = root.get("outputs") {
        let map = value
            .as_object()
            .context("action metadata `outputs` must be a mapping")?;
        for (key, value) in map {
            let key = validate_metadata_key(key, "output")?;
            if !value.is_object() {
                bail!("action output {key:?} must be a mapping");
            }
            outputs.push(key);
        }
    }
    inputs.sort_by(|left, right| left.name.cmp(&right.name));
    outputs.sort();
    Ok(ActionMetadata {
        inputs,
        outputs,
        metadata_file: metadata_file.to_string(),
    })
}

fn render_binding(entry: &LockEntry) -> String {
    let identifier = ts_identifier(&entry.name);
    let interface_name = format!("{identifier}Inputs");
    let outputs_name = format!("{identifier}Outputs");
    let action_name = format!("{identifier}Action");
    let sdk = &entry.sdk_import;
    let patches = serde_json::to_string(&entry.patches).expect("serializing patches cannot fail");
    let mut output = String::new();
    writeln!(
        output,
        "/**\n * Generated by `loopt actions`; do not edit this file by hand.\n *\n * Import this binding from `.loopt/workflows/*.ts` with:\n *   import {{ {identifier} }} from \"../actions/{}.ts\";\n *\n * The action is pinned to the immutable commit {} (requested ref {}).\n * Metadata file: {}; SDK import: {}.\n * Binding patches: {}.\n */",
        entry.name,
        entry.sha,
        comment_safe(&entry.requested_ref),
        comment_safe(&entry.metadata),
        comment_safe(sdk),
        comment_safe(&patches)
    )
    .expect("writing to String cannot fail");
    writeln!(output, "import {{ uses }} from \"{sdk}\";").expect("writing to String cannot fail");
    writeln!(
        output,
        "import type {{ Expr, StepOpts, StepRef }} from \"{sdk}\";\n"
    )
    .expect("writing to String cannot fail");
    writeln!(
        output,
        "export const {action_name} = {};\n",
        ts_string_literal(&format!("{}@{}", entry.source, entry.sha))
    )
    .expect("writing to String cannot fail");
    writeln!(output, "export interface {interface_name} {{")
        .expect("writing to String cannot fail");
    if entry.inputs.is_empty() {
        output.push_str("  // This action declares no inputs.\n");
    } else {
        for input in &entry.inputs {
            let property = ts_property_name(&input.name);
            let optional = if input.required { "" } else { "?" };
            let type_name = &input.type_name;
            writeln!(
                output,
                "  readonly {property}{optional}: {type_name} | Expr<{type_name}>;"
            )
            .expect("writing to String cannot fail");
        }
    }
    output.push_str("}\n\n");
    write!(output, "export type {outputs_name} = ").expect("writing to String cannot fail");
    if entry.outputs.is_empty() {
        output.push_str("never\n\n");
    } else {
        output.push_str(
            &entry
                .outputs
                .iter()
                .map(|output| ts_string_literal(output))
                .collect::<Vec<_>>()
                .join(" | "),
        );
        output.push_str(";\n\n");
    }

    let input_optional = if entry.inputs.iter().any(|input| input.required) {
        ""
    } else {
        "?"
    };
    writeln!(
        output,
        "export function {identifier}(inputs{input_optional}: {interface_name}, opts?: StepOpts<{outputs_name}>): StepRef<{outputs_name}> {{"
    )
    .expect("writing to String cannot fail");
    output.push_str(
        "  const values = inputs === undefined\n    ? undefined\n    : (inputs as unknown as Record<string, string | number | boolean | Expr>);\n",
    );
    writeln!(
        output,
        "  return uses<{outputs_name}>({action_name}, values, opts);"
    )
    .expect("writing to String cannot fail");
    output.push_str("}\n");
    output
}

fn ts_identifier(value: &str) -> String {
    let mut identifier = String::new();
    let mut upper_next = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            if identifier.is_empty() && character.is_ascii_digit() {
                identifier.push_str("action");
            }
            if upper_next {
                identifier.push(character.to_ascii_uppercase());
                upper_next = false;
            } else {
                identifier.push(character);
            }
        } else {
            upper_next = true;
        }
    }
    if identifier.is_empty() {
        identifier.push_str("action");
    }
    if is_ts_reserved(&identifier) {
        identifier.insert_str(0, "action");
    }
    identifier
}

fn is_ts_reserved(value: &str) -> bool {
    matches!(
        value,
        "as" | "break"
            | "case"
            | "catch"
            | "class"
            | "const"
            | "continue"
            | "debugger"
            | "default"
            | "delete"
            | "do"
            | "else"
            | "enum"
            | "export"
            | "extends"
            | "false"
            | "finally"
            | "for"
            | "function"
            | "if"
            | "import"
            | "in"
            | "instanceof"
            | "let"
            | "new"
            | "null"
            | "return"
            | "super"
            | "switch"
            | "this"
            | "throw"
            | "true"
            | "try"
            | "typeof"
            | "var"
            | "void"
            | "while"
            | "with"
            | "yield"
            | "interface"
            | "private"
            | "protected"
            | "public"
            | "static"
            | "implements"
            | "package"
            | "await"
            | "namespace"
            | "type"
    )
}

fn ts_property_name(value: &str) -> String {
    if value.chars().enumerate().all(|(index, character)| {
        (character.is_ascii_alphabetic() || character == '_' || character == '$')
            || (index > 0 && character.is_ascii_digit())
    }) {
        value.to_string()
    } else {
        ts_string_literal(value)
    }
}

fn ts_string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("serializing a string cannot fail")
}

fn comment_safe(value: &str) -> String {
    value.replace("*/", "*\\/").replace(['\n', '\r'], " ")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(output, "{byte:02x}").expect("writing to String cannot fail");
    }
    output
}
