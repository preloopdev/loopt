//! `loopt secrets`: names-only GitHub Actions secret inventory.
//!
//! `sync` talks to the authenticated GitHub CLI (`gh api`) and writes only
//! secret names.  It never calls a secret-value endpoint.  The generated JSON
//! is directly consumable as a `preloop_gha_lint::LintContext`, while the
//! companion TypeScript module exposes expression accessors for the SDK's
//! `secrets` context.
//!
//! The command is intentionally independent of the top-level CLI parser.  The
//! binary can route its trailing arguments to [`run`], which keeps this module
//! usable by embedders and by deterministic integration tests:
//!
//! ```text
//! loopt secrets sync [--repo OWNER/REPOSITORY]
//!     [--organization ORGANIZATION] [--environment NAME]...
//!     [--all-environments] [--output PATH] [--typescript-output PATH]
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde_json::{Map, Value};

use crate::actions::validate_sdk_import;
use crate::paths;

const INVENTORY_PATH: &str = ".loopt/secrets.json";
const TYPESCRIPT_PATH: &str = ".loopt/secrets.ts";
const DEFAULT_GH_BINARY: &str = "gh";
const DEFAULT_SDK_IMPORT: &str = "@loopt/sdk";

/// Run `loopt secrets` with the already-separated trailing arguments.
///
/// `sync` is the only operation currently exposed.  Unknown options are
/// rejected rather than silently selecting a narrower inventory, because an
/// incomplete inventory can make linting report a false negative.
pub fn run(root: &Path, args: &[String]) -> Result<()> {
    let options = SyncOptions::parse(args)?;
    sync(root, &options)
}

#[derive(Debug, Clone)]
struct SyncOptions {
    repo: Option<String>,
    organization: Option<String>,
    environments: Vec<String>,
    all_environments: bool,
    output: PathBuf,
    typescript_output: PathBuf,
    sdk_import: Option<String>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            repo: None,
            organization: None,
            environments: Vec::new(),
            all_environments: false,
            output: PathBuf::from(INVENTORY_PATH),
            typescript_output: PathBuf::from(TYPESCRIPT_PATH),
            sdk_import: None,
        }
    }
}

impl SyncOptions {
    fn parse(args: &[String]) -> Result<Self> {
        let mut args = args.iter();
        let operation = args.next().map(String::as_str);
        if operation != Some("sync") {
            bail!("usage: loopt secrets sync [OPTIONS]");
        }

        let mut options = Self::default();
        while let Some(argument) = args.next() {
            let (name, inline_value) = match argument.split_once('=') {
                Some((name, value)) if name.starts_with('-') => (name, Some(value)),
                _ => (argument.as_str(), None),
            };

            match name {
                "--repo" | "--repository" => {
                    let value = option_value(name, inline_value, &mut args)?;
                    validate_repo(&value)
                        .with_context(|| format!("invalid repository from {name}"))?;
                    if options.repo.replace(value).is_some() {
                        bail!("{name}: option may only be supplied once");
                    }
                }
                "--organization" | "--org" => {
                    let value = option_value(name, inline_value, &mut args)?;
                    validate_owner_or_repository_component(&value, "organization")
                        .with_context(|| format!("invalid organization from {name}"))?;
                    if options.organization.replace(value).is_some() {
                        bail!("{name}: option may only be supplied once");
                    }
                }
                "--environment" | "--env" => {
                    let value = option_value(name, inline_value, &mut args)?;
                    validate_environment_name(&value)
                        .with_context(|| format!("invalid environment from {name}"))?;
                    if options.environments.contains(&value) {
                        bail!("{name}: duplicate environment {value:?}");
                    }
                    options.environments.push(value);
                }
                "--all-environments" => {
                    if inline_value.is_some() {
                        bail!("--all-environments does not take a value");
                    }
                    options.all_environments = true;
                }
                "--output" | "--inventory" => {
                    let value = option_value(name, inline_value, &mut args)?;
                    options.output = relative_loopt_path(&value, "--output")?;
                }
                "--typescript-output" | "--typescript" => {
                    let value = option_value(name, inline_value, &mut args)?;
                    options.typescript_output = relative_loopt_path(&value, "--typescript-output")?;
                }
                "--sdk-import" => {
                    let value = option_value(name, inline_value, &mut args)?;
                    validate_sdk_import(&value)
                        .with_context(|| format!("invalid SDK import from {name}"))?;
                    if options.sdk_import.replace(value).is_some() {
                        bail!("{name}: option may only be supplied once");
                    }
                }
                _ if name.starts_with('-') => bail!("unknown secrets option {name:?}"),
                _ => bail!("unexpected secrets argument {argument:?}"),
            }
        }

        if options.all_environments && !options.environments.is_empty() {
            bail!("--all-environments cannot be combined with --environment");
        }
        if options.output == options.typescript_output {
            bail!("inventory and TypeScript output paths must differ");
        }
        Ok(options)
    }
}

fn option_value<'a, I>(name: &str, inline: Option<&str>, args: &mut I) -> Result<String>
where
    I: Iterator<Item = &'a String>,
{
    let value = match inline {
        Some(value) => value.to_owned(),
        None => args
            .next()
            .with_context(|| format!("{name} requires a value"))?
            .to_owned(),
    };
    if value.is_empty() {
        bail!("{name} requires a non-empty value");
    }
    Ok(value)
}

fn relative_loopt_path(value: &str, option: &str) -> Result<PathBuf> {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        bail!("{option}: path must be relative to the repository");
    }
    if path.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        bail!("{option}: path contains unsafe components");
    }
    // `resolve_within` below performs the component and symlink checks again
    // immediately before every write.  This lexical check gives callers an
    // early, useful error and keeps custom output inside `.loopt`.
    if !path.starts_with(".loopt") {
        bail!("{option}: path must be under .loopt");
    }
    Ok(path)
}

fn sync(root: &Path, options: &SyncOptions) -> Result<()> {
    let repo = match options.repo.as_deref() {
        Some(repo) => validate_repo(repo)?.to_owned(),
        None => discover_repo(root)?,
    };
    preflight_output(root, &options.output)?;
    preflight_output(root, &options.typescript_output)?;

    let metadata_endpoint = format!("repos/{repo}");
    let metadata = gh_value(root, &metadata_endpoint, false)
        .with_context(|| "read repository metadata from GitHub")?;
    let repository = Repository::from_value(&metadata, &repo)?;

    let repository_names = secret_names(
        &gh_pages(root, &format!("repos/{repo}/actions/secrets"), true)?,
        "secrets",
    )?;

    let organization = options
        .organization
        .clone()
        .or_else(|| repository.organization_name());
    let organization_names = match organization {
        Some(organization) => organization_secret_names(root, &organization, &repository, &repo)?,
        None => BTreeSet::new(),
    };

    let mut available_secrets = repository_names;
    available_secrets.extend(organization_names);

    let environment_pages = gh_pages(root, &format!("repos/{repo}/environments"), true)?;
    let all_environments = environment_names(&environment_pages)?;
    let selected_environments = select_environments(&all_environments, options)?;

    let mut environment_secrets = BTreeMap::new();
    for environment in selected_environments {
        let endpoint = format!(
            "repos/{repo}/environments/{}/secrets",
            encode_path_segment(&environment)
        );
        let names = secret_names(&gh_pages(root, &endpoint, true)?, "secrets")?;
        environment_secrets.insert(environment, names);
    }

    let inventory = Inventory {
        available_secrets: Some(available_secrets),
        environment_secrets,
        known_environments: Some(all_environments),
    };
    let inventory_json = serde_json::to_string_pretty(&inventory)? + "\n";
    let typescript = render_typescript(
        &inventory,
        options.sdk_import.as_deref().unwrap_or(DEFAULT_SDK_IMPORT),
    );

    // No output is touched until every authenticated request above has
    // completed and all response names have been validated.  `write_atomic`
    // additionally rejects symlinked parents and destinations.
    preflight_output(root, &options.output)?;
    preflight_output(root, &options.typescript_output)?;
    paths::resolve_within(root, &options.output)?;
    paths::resolve_within(root, &options.typescript_output)?;
    paths::write_atomic(root, &options.output, inventory_json.as_bytes())
        .context("write .loopt/secrets.json")?;
    if let Err(error) = paths::write_atomic(root, &options.typescript_output, typescript.as_bytes())
    {
        // The JSON write is already durable and valid.  Do not claim a failed
        // TypeScript write is a successful sync; callers can retry safely.
        return Err(error).context("write .loopt/secrets.ts");
    }
    Ok(())
}

fn preflight_output(root: &Path, relative: &Path) -> Result<()> {
    let destination = paths::resolve_within(root, relative)?;
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "{}: refusing to replace symlink destination",
                relative.display()
            );
        }
        Ok(metadata) if !metadata.file_type().is_file() => {
            bail!(
                "{}: output destination is not a regular file",
                relative.display()
            );
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("inspect {}", relative.display())),
    }
}

#[derive(Debug, Clone)]
struct Repository {
    name_with_owner: String,
    owner: String,
    owner_type: String,
    private: bool,
    id: Option<u64>,
}

impl Repository {
    fn from_value(value: &Value, requested: &str) -> Result<Self> {
        let object = value
            .as_object()
            .with_context(|| "GitHub returned invalid repository metadata")?;
        let private = object
            .get("private")
            .and_then(Value::as_bool)
            .with_context(|| "GitHub repository metadata omitted privacy")?;
        let owner_object = object
            .get("owner")
            .and_then(Value::as_object)
            .with_context(|| "GitHub repository metadata omitted owner")?;
        let owner = owner_object
            .get("login")
            .and_then(Value::as_str)
            .with_context(|| "GitHub repository metadata omitted owner login")?;
        let owner_type = owner_object
            .get("type")
            .and_then(Value::as_str)
            .with_context(|| "GitHub repository metadata omitted owner type")?;
        validate_owner_or_repository_component(owner, "repository owner")?;
        if !owner_type.eq_ignore_ascii_case("user")
            && !owner_type.eq_ignore_ascii_case("organization")
        {
            bail!("GitHub returned unsupported repository owner type");
        }
        let requested = validate_repo(requested)?;
        let requested_owner = requested
            .split_once('/')
            .map(|(owner, _)| owner)
            .expect("validate_repo guarantees an owner");
        if !owner.eq_ignore_ascii_case(requested_owner) {
            bail!("GitHub repository metadata did not match the selected owner");
        }
        let name_with_owner = object
            .get("full_name")
            .and_then(Value::as_str)
            .with_context(|| "GitHub repository metadata omitted full name")?
            .to_owned();
        if !name_with_owner.eq_ignore_ascii_case(requested) {
            // A GitHub redirect or alias must not silently make us inventory a
            // different repository than the caller selected.
            bail!("GitHub repository metadata did not match the selected repository");
        }
        let id = object.get("id").and_then(Value::as_u64);
        Ok(Self {
            name_with_owner,
            owner: owner.to_owned(),
            owner_type: owner_type.to_owned(),
            private,
            id,
        })
    }

    fn organization_name(&self) -> Option<String> {
        self.owner_type
            .eq_ignore_ascii_case("organization")
            .then(|| self.owner.clone())
    }
}

fn organization_secret_names(
    root: &Path,
    organization: &str,
    repository: &Repository,
    repo: &str,
) -> Result<BTreeSet<String>> {
    validate_owner_or_repository_component(organization, "organization")?;
    let endpoint = format!("orgs/{organization}/actions/secrets");
    let pages = gh_pages(root, &endpoint, true)
        .with_context(|| "read organization-visible secret inventory from GitHub")?;
    let objects = named_objects(&pages, "secrets")?;
    let mut names = BTreeSet::new();
    for object in objects {
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .with_context(|| "GitHub organization secret omitted its name")?;
        validate_secret_name(name)?;
        let visibility = object
            .get("visibility")
            .and_then(Value::as_str)
            .with_context(|| "GitHub organization secret omitted visibility")?;
        match visibility {
            "all" => {
                names.insert(name.to_owned());
            }
            "private" if repository.private => {
                names.insert(name.to_owned());
            }
            "private" => {}
            "selected" => {
                if organization_secret_is_selected(root, organization, name, repository, repo)? {
                    names.insert(name.to_owned());
                }
            }
            _ => bail!("GitHub returned unsupported organization secret visibility"),
        }
    }
    Ok(names)
}

fn organization_secret_is_selected(
    root: &Path,
    organization: &str,
    secret_name: &str,
    repository: &Repository,
    repo: &str,
) -> Result<bool> {
    let endpoint = format!(
        "orgs/{organization}/actions/secrets/{}/repositories",
        encode_path_segment(secret_name)
    );
    let pages = gh_pages(root, &endpoint, true)
        .with_context(|| "read selected repositories for an organization secret")?;
    for object in named_objects(&pages, "repositories")? {
        if object_matches_repository(object, repository, repo) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn object_matches_repository(
    object: &Map<String, Value>,
    repository: &Repository,
    repo: &str,
) -> bool {
    if let (Some(expected), Some(actual)) =
        (repository.id, object.get("id").and_then(Value::as_u64))
    {
        if expected == actual {
            return true;
        }
    }
    if object
        .get("full_name")
        .and_then(Value::as_str)
        .is_some_and(|full_name| full_name.eq_ignore_ascii_case(&repository.name_with_owner))
    {
        return true;
    }
    let Some(name) = object.get("name").and_then(Value::as_str) else {
        return false;
    };
    let Some((_, expected_name)) = repo.split_once('/') else {
        return false;
    };
    name.eq_ignore_ascii_case(expected_name)
}

fn select_environments(all: &BTreeSet<String>, options: &SyncOptions) -> Result<Vec<String>> {
    if options.environments.is_empty() || options.all_environments {
        return Ok(all.iter().cloned().collect());
    }
    for requested in &options.environments {
        if !all.contains(requested) {
            bail!("environment {requested:?} was not returned by GitHub");
        }
    }
    Ok(options.environments.to_vec())
}

#[derive(Debug, serde::Serialize)]
struct Inventory {
    #[serde(skip_serializing_if = "Option::is_none")]
    available_secrets: Option<BTreeSet<String>>,
    environment_secrets: BTreeMap<String, BTreeSet<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    known_environments: Option<BTreeSet<String>>,
}

fn environment_names(pages: &[Value]) -> Result<BTreeSet<String>> {
    let objects = named_objects(pages, "environments")?;
    let mut names = BTreeSet::new();
    for object in objects {
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .with_context(|| "GitHub environment omitted its name")?;
        validate_environment_name(name)?;
        names.insert(name.to_owned());
    }
    Ok(names)
}

fn secret_names(pages: &[Value], key: &str) -> Result<BTreeSet<String>> {
    let objects = named_objects(pages, key)?;
    let mut names = BTreeSet::new();
    for object in objects {
        let name = object
            .get("name")
            .and_then(Value::as_str)
            .with_context(|| "GitHub secret omitted its name")?;
        validate_secret_name(name)?;
        names.insert(name.to_owned());
    }
    Ok(names)
}

/// Collect only objects in the endpoint's documented collection.  In
/// particular, arbitrary fields (including any accidental `value` field in a
/// fixture or future API response) are never copied into the inventory.
fn named_objects<'a>(pages: &'a [Value], key: &str) -> Result<Vec<&'a Map<String, Value>>> {
    let mut objects = Vec::new();
    let mut saw_collection = false;
    for page in pages {
        collect_named_objects(page, key, &mut objects, &mut saw_collection);
    }
    if !saw_collection && !pages.is_empty() {
        bail!("GitHub returned an unexpected response shape for {key}");
    }
    Ok(objects)
}

fn collect_named_objects<'a>(
    value: &'a Value,
    key: &str,
    objects: &mut Vec<&'a Map<String, Value>>,
    saw_collection: &mut bool,
) {
    match value {
        Value::Array(values) => {
            *saw_collection = true;
            for value in values {
                collect_named_objects(value, key, objects, saw_collection);
            }
        }
        Value::Object(map) => {
            if let Some(Value::Array(values)) = map.get(key) {
                *saw_collection = true;
                for value in values {
                    if let Value::Object(object) = value {
                        objects.push(object);
                    }
                }
            } else if map.contains_key("name") {
                // This permits a fixture (or a future gh wrapper) to return a
                // single resource object while still requiring a name field.
                *saw_collection = true;
                objects.push(map);
            }
        }
        _ => {}
    }
}

fn gh_pages(root: &Path, endpoint: &str, paginate: bool) -> Result<Vec<Value>> {
    let mut command = Command::new(gh_binary());
    command.current_dir(root).arg("api").arg(endpoint);
    if paginate {
        command.arg("--paginate").arg("--slurp");
    }
    let output = command
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .with_context(|| "run authenticated gh api")?;
    if !output.status.success() {
        // Do not include stderr: gh extensions and API proxies are not trusted
        // to keep diagnostics free of values accidentally echoed by a server.
        bail!("gh api request failed for {endpoint}");
    }
    if output.stdout.is_empty() {
        bail!("gh api returned an empty response for {endpoint}");
    }
    let mut documents = Vec::new();
    for document in serde_json::Deserializer::from_slice(&output.stdout).into_iter::<Value>() {
        documents.push(document.with_context(|| "parse gh api JSON response")?);
    }
    if documents.is_empty() {
        bail!("gh api returned no JSON response for {endpoint}");
    }
    if paginate && documents.len() == 1 && matches!(documents[0], Value::Array(_)) {
        if let Value::Array(pages) = documents.remove(0) {
            return Ok(pages);
        }
    }
    Ok(documents)
}

fn gh_value(root: &Path, endpoint: &str, paginate: bool) -> Result<Value> {
    let pages = gh_pages(root, endpoint, paginate)?;
    if pages.len() != 1 {
        bail!("gh api returned multiple repository metadata responses");
    }
    Ok(pages.into_iter().next().expect("length checked above"))
}

fn gh_binary() -> OsString {
    // This narrow override is useful for hermetic callers and integration
    // tests.  It does not alter the default authenticated `gh` behavior.
    std::env::var_os("LOOPT_GH").unwrap_or_else(|| OsString::from(DEFAULT_GH_BINARY))
}

fn discover_repo(root: &Path) -> Result<String> {
    let config_path = root.join(".git/config");
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| "no --repo supplied and .git/config could not be read")?;
    for line in content.lines() {
        let Some(value) = line.trim().strip_prefix("url =") else {
            continue;
        };
        if let Some(repo) = parse_remote_url(value.trim()) {
            return Ok(validate_repo(repo)?.to_owned());
        }
    }
    bail!("no GitHub repository found; pass --repo OWNER/REPOSITORY")
}

fn parse_remote_url(value: &str) -> Option<&str> {
    let value = value.strip_suffix(".git").unwrap_or(value);
    if let Some(rest) = value.strip_prefix("https://github.com/") {
        return Some(rest);
    }
    if let Some(rest) = value.strip_prefix("http://github.com/") {
        return Some(rest);
    }
    if let Some(rest) = value.strip_prefix("git@github.com:") {
        return Some(rest);
    }
    None
}

fn validate_repo(value: &str) -> Result<&str> {
    let mut parts = value.split('/');
    let owner = parts.next().unwrap_or_default();
    let repository = parts.next().unwrap_or_default();
    if parts.next().is_some() || owner.is_empty() || repository.is_empty() {
        bail!("repository must be OWNER/REPOSITORY");
    }
    validate_owner_or_repository_component(owner, "repository owner")?;
    validate_owner_or_repository_component(repository, "repository name")?;
    Ok(value)
}

fn validate_owner_or_repository_component(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        || value == "."
        || value == ".."
    {
        bail!("{label} contains unsafe path characters");
    }
    Ok(())
}

fn validate_secret_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 100
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        bail!("GitHub returned an unsafe secret name");
    }
    Ok(())
}

fn validate_environment_name(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 255
        || value == "."
        || value == ".."
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        bail!("environment name contains unsafe path characters");
    }
    Ok(())
}

fn encode_path_segment(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push(HEX[(byte >> 4) as usize] as char);
            encoded.push(HEX[(byte & 0x0f) as usize] as char);
        }
    }
    encoded
}

fn render_typescript(inventory: &Inventory, sdk_import: &str) -> String {
    let mut all_names = BTreeSet::new();
    if let Some(names) = &inventory.available_secrets {
        all_names.extend(names.iter().cloned());
    }
    for names in inventory.environment_secrets.values() {
        all_names.extend(names.iter().cloned());
    }

    let mut output = String::from(
        "/**\n * Generated by `loopt secrets sync`; names only.\n * Secret values are supplied by GitHub Actions at runtime.\n */\n",
    );
    output.push_str(&format!(
        "import {{ secrets as githubSecrets }} from \"{sdk_import}\";\n\n"
    ));

    output.push_str("export const secretNames = [\n");
    if let Some(names) = &inventory.available_secrets {
        for name in names {
            output.push_str("  ");
            output.push_str(&ts_string_literal(name));
            output.push_str(",\n");
        }
    }
    output.push_str("] as const;\n\n");
    output.push_str("export const knownEnvironmentNames = [\n");
    if let Some(names) = &inventory.known_environments {
        for name in names {
            output.push_str("  ");
            output.push_str(&ts_string_literal(name));
            output.push_str(",\n");
        }
    }
    output.push_str("] as const;\n\n");
    output
        .push_str("export type KnownEnvironmentName = (typeof knownEnvironmentNames)[number];\n\n");

    output.push_str("export const environmentSecretNames = {\n");
    for (environment, names) in &inventory.environment_secrets {
        output.push_str("  ");
        output.push_str(&ts_string_literal(environment));
        output.push_str(": [\n");
        for name in names {
            output.push_str("    ");
            output.push_str(&ts_string_literal(name));
            output.push_str(",\n");
        }
        output.push_str("  ] as const,\n");
    }
    output.push_str("} as const;\n\n");
    output.push_str("export type EnvironmentName = keyof typeof environmentSecretNames;\n\n");

    output.push_str("export const secretAccessors = {\n");
    for name in &all_names {
        output.push_str("  ");
        output.push_str(&ts_property_name(name));
        output.push_str(": ");
        output.push_str(&ts_accessor(name));
        output.push_str(",\n");
    }
    output.push_str("} as const;\n\n");
    output.push_str("export type SecretName = keyof typeof secretAccessors;\n\n");
    output.push_str("export type SecretAccessors = typeof secretAccessors;\n\n");
    output.push_str("export const secrets = secretAccessors;\n\n");
    output.push_str(
        "export function secret(name: SecretName): ReturnType<typeof githubSecrets.at> {\n  return githubSecrets.at(name);\n}\n",
    );

    output
}

fn ts_property_name(value: &str) -> String {
    if is_typescript_identifier(value) && !is_typescript_reserved(value) {
        value.to_owned()
    } else {
        ts_string_literal(value)
    }
}

fn ts_accessor(value: &str) -> String {
    if is_typescript_identifier(value)
        && !is_typescript_reserved(value)
        && !matches!(
            value,
            "at" | "bare" | "contexts" | "precedence" | "source" | "then" | "toString" | "valueOf"
        )
    {
        format!("githubSecrets.{value}")
    } else {
        format!("githubSecrets.at({})", ts_string_literal(value))
    }
}

fn is_typescript_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first == '$' || first.is_ascii_alphabetic())
        && chars.all(|character| {
            character == '_' || character == '$' || character.is_ascii_alphanumeric()
        })
}

fn is_typescript_reserved(value: &str) -> bool {
    matches!(
        value,
        "await"
            | "break"
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
            | "implements"
            | "import"
            | "in"
            | "instanceof"
            | "interface"
            | "let"
            | "new"
            | "null"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "static"
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
    )
}

fn ts_string_literal(value: &str) -> String {
    serde_json::to_string(value).expect("strings are always serializable")
}
