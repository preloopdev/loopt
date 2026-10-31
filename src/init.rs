//! `loopt init`: start a project from nothing.
//!
//! Writes a small starter workflow as editable source in the chosen language,
//! then runs the normal `gen` so the workflow YAML and the manifest exist
//! straight away. The starter is an ordinary workflow value rendered through
//! the same per-language `emit` that `import` uses, so every built-in
//! language gets it without any language-specific code here.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::gen::{self, GenOptions};
use crate::languages::Language;
use crate::manifest::Manifest;
use crate::paths;

const WORKFLOWS_DIR: &str = ".github/workflows";
const SOURCE_DIR: &str = ".loopt/workflows";

/// `actions/checkout` v7.0.1, pinned to its commit so the starter is clean
/// under `loopt lint` (`unpinned-uses`) from the first run.
const CHECKOUT: &str = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1";

pub struct InitOptions {
    /// A built-in language id: typescript, python, go, or rust.
    pub language: String,
    /// Workflow name; becomes the source file stem and the YAML file name.
    pub name: String,
    /// `auto`, or one of: generic, node, rust, go, python.
    pub template: String,
}

#[derive(Clone, Copy)]
enum Template {
    Generic,
    Node,
    Rust,
    Go,
    Python,
}

impl Template {
    fn parse(value: &str) -> Result<Template> {
        match value {
            "generic" => Ok(Template::Generic),
            "node" => Ok(Template::Node),
            "rust" => Ok(Template::Rust),
            "go" => Ok(Template::Go),
            "python" => Ok(Template::Python),
            other => {
                bail!("unknown template {other:?}; use auto, generic, node, rust, go, or python")
            }
        }
    }

    fn label(self) -> &'static str {
        match self {
            Template::Generic => "generic",
            Template::Node => "node",
            Template::Rust => "rust",
            Template::Go => "go",
            Template::Python => "python",
        }
    }
}

/// The first project file found in `root`, and the starter it implies.
fn detect(root: &Path) -> Option<(Template, &'static str)> {
    [
        ("Cargo.toml", Template::Rust),
        ("package.json", Template::Node),
        ("go.mod", Template::Go),
        ("pyproject.toml", Template::Python),
        ("requirements.txt", Template::Python),
        ("setup.py", Template::Python),
    ]
    .into_iter()
    .find(|(marker, _)| root.join(marker).is_file())
    .map(|(marker, template)| (template, marker))
}

fn run_step(name: &str, command: &str) -> Value {
    json!({ "name": name, "run": command })
}

/// The build-and-test steps for `template`. Only commands every project of
/// that kind already has: the runner image provides the toolchains, so the
/// starter needs no third-party setup action.
fn test_steps(template: Template, root: &Path) -> Vec<Value> {
    let has = |file: &str| root.join(file).is_file();
    match template {
        Template::Node => vec![
            run_step(
                "Install dependencies",
                if has("package-lock.json") {
                    "npm ci"
                } else {
                    "npm install"
                },
            ),
            run_step("Test", "npm test"),
        ],
        Template::Rust => vec![run_step("Test", "cargo test")],
        Template::Go => vec![run_step("Test", "go test ./...")],
        Template::Python => vec![
            run_step(
                "Install dependencies",
                if has("requirements.txt") {
                    "python -m pip install -r requirements.txt"
                } else {
                    "python -m pip install ."
                },
            ),
            run_step("Test", "python -m pytest"),
        ],
        Template::Generic => vec![run_step(
            "Build and test (replace this step)",
            "echo \"Replace this step with your build and test commands\"",
        )],
    }
}

/// A least-privilege CI workflow that is clean under `loopt lint`: pinned
/// checkout without persisted credentials, read-only token, a concurrency
/// group that cancels superseded runs, and a job timeout.
fn starter(name: &str, template: Template, root: &Path) -> Value {
    let mut steps = vec![json!({
        "uses": CHECKOUT,
        "with": { "persist-credentials": false },
    })];
    steps.extend(test_steps(template, root));
    json!({
        "name": name,
        "on": { "push": { "branches": ["main"] }, "pull_request": {} },
        "permissions": { "contents": "read" },
        "concurrency": {
            "group": format!("{name}-${{{{ github.ref }}}}"),
            "cancel-in-progress": true,
        },
        "jobs": {
            "test": {
                "runs-on": "ubuntu-latest",
                "timeout-minutes": 30,
                "steps": steps,
            },
        },
    })
}

/// The name becomes a file name and the workflow's `name:`, so keep it to a
/// single safe path component.
fn validate_name(name: &str) -> Result<()> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        bail!(
            "invalid workflow name {name:?}: start with a letter or digit and use only letters, digits, '-' and '_'"
        );
    }
    Ok(())
}

pub fn run(root: &Path, options: InitOptions) -> Result<()> {
    validate_name(&options.name)?;
    let language = Language::parse(&options.language).with_context(|| {
        format!(
            "{:?} is not a built-in language (typescript, python, go, rust); for a custom language, start from a YAML file and run `loopt import --plugin`",
            options.language
        )
    })?;

    let (template, why) = match options.template.as_str() {
        "auto" => match detect(root) {
            Some((template, marker)) => (template, format!("found {marker}")),
            None => (Template::Generic, "no project files found".to_owned()),
        },
        explicit => (Template::parse(explicit)?, "from --template".to_owned()),
    };

    // Refuse before writing anything: init never overwrites a workflow, a
    // source file, or an output the manifest already manages.
    let output = PathBuf::from(WORKFLOWS_DIR).join(format!("{}.yml", options.name));
    if root.join(&output).exists() {
        bail!(
            "{} already exists; use `loopt import` to turn it into source, or pick another --name",
            output.display()
        );
    }
    if Manifest::load(root)?
        .entries
        .iter()
        .any(|entry| entry.output == output)
    {
        bail!(
            "{} is already managed by .loopt/manifest.json; pick another --name",
            output.display()
        );
    }

    println!("starter: {} ({why})", template.label());

    let stem = PathBuf::from(SOURCE_DIR).join(&options.name);
    let generated = language.emit(&starter(&options.name, template, root), &stem)?;
    for file in &generated.files {
        paths::require_under(root, SOURCE_DIR, &file.path)
            .with_context(|| format!("{}: unsafe generated path", file.path.display()))?;
        paths::resolve_within(root, &file.path)
            .with_context(|| format!("{}: unsafe generated path", file.path.display()))?;
        if root.join(&file.path).exists() {
            bail!(
                "{} already exists; pick another --name",
                file.path.display()
            );
        }
    }
    for file in &generated.files {
        paths::write_atomic(root, &file.path, file.content.as_bytes())
            .with_context(|| format!("write {}", file.path.display()))?;
        println!("created {}", file.path.display());
    }

    gen::run(
        root,
        GenOptions {
            sources: vec![generated.entrypoint.clone()],
            force: false,
            lint: true,
            check: false,
        },
    )
    .with_context(|| {
        format!(
            "starter source written to {}, but generating the workflow failed; fix the error and run `loopt gen`",
            generated.entrypoint.display()
        )
    })?;

    println!(
        "\nNext: edit {}, then run `loopt gen`. In CI, run `loopt gen --check` to catch drift.",
        generated.entrypoint.display()
    );
    Ok(())
}
