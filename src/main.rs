//! `loopt` CLI: argument parsing and dispatch only. The actual `import` and
//! `gen` behavior lives in the `loopt` library crate (`src/lib.rs`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use loopt::plugin::PluginSpec;
use loopt::shellcheck::{ShellcheckMode, ShellcheckOptions};
use loopt::{actions, gen, import, init, lint, plugin, secrets};
use preloop_gha_sec::Persona;

#[derive(Parser)]
#[command(
    name = "loopt",
    version,
    about = "Author GitHub Actions workflows in code, and generate standard workflow YAML from them."
)]
struct Cli {
    /// Repository root. Defaults to the current directory.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage pinned action bindings: add, update, list, remove or verify.
    Actions {
        /// Subcommand and its options.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Synchronize names-only secret inventory from GitHub.
    Secrets {
        /// Subcommand and its options.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
    /// Start a new project: write a starter workflow as source, then generate it.
    ///
    /// Picks a starter from the project files it finds (package.json,
    /// Cargo.toml, go.mod, Python project files) unless --template is given.
    /// Never overwrites an existing workflow; use `loopt import` for those.
    Init {
        /// Authoring language: typescript, python, go, or rust.
        #[arg(long, default_value = "typescript")]
        language: String,
        /// Workflow name, used for `.loopt/workflows/<name>` and
        /// `.github/workflows/<name>.yml`.
        #[arg(long, default_value = "ci")]
        name: String,
        /// Starter content: auto, generic, node, rust, go, or python.
        #[arg(long, default_value = "auto")]
        template: String,
    },
    /// Convert existing workflow YAML into editable authoring source.
    ///
    /// Never modifies the original YAML. Defaults to every direct child of
    /// `.github/workflows` when no paths are given.
    Import {
        /// YAML files to import.
        paths: Vec<PathBuf>,
        /// Authoring language: typescript, python, go, rust, or a custom
        /// language id paired with --plugin.
        #[arg(long, default_value = "typescript")]
        language: String,
        /// Overwrite authoring source Loopt did not generate.
        #[arg(long)]
        force: bool,
        /// Executable implementing the `loopt plugin` protocol, for a
        /// custom (non-built-in) --language.
        #[arg(long)]
        plugin: Option<String>,
        /// Argument for --plugin's executable. Repeatable; order preserved.
        #[arg(long = "plugin-arg", requires = "plugin")]
        plugin_arg: Vec<String>,
    },
    /// Run authoring source and write standard workflow YAML.
    ///
    /// Defaults to every `.loopt/manifest.json` entry plus any new source
    /// discovered under `.loopt/workflows` when no sources are given.
    Gen {
        /// Source entrypoints to run.
        sources: Vec<PathBuf>,
        /// Overwrite workflow YAML Loopt did not last write.
        #[arg(long)]
        force: bool,
        /// Skip the native lint/security review after generation.
        #[arg(long)]
        no_lint: bool,
        /// Verify generated YAML is up to date without writing anything.
        /// Exits non-zero if any workflow would change.
        #[arg(long, conflicts_with = "force")]
        check: bool,
    },
    /// Run native correctness and security checks on workflow YAML.
    ///
    /// Defaults to the generated outputs recorded in `.loopt/manifest.json`,
    /// or direct `.github/workflows` files when nothing has been generated.
    Lint {
        /// Workflow YAML files to check.
        paths: Vec<PathBuf>,
        /// Machine-readable JSON output.
        #[arg(long)]
        json: bool,
        /// Reviewer persona: regular, pedantic, auditor.
        #[arg(long, default_value = "regular")]
        persona: String,
        /// JSON inventory of runner labels and secret names (never values).
        #[arg(long)]
        context: Option<PathBuf>,
        /// shellcheck integration for `run:` scripts: auto, on, off.
        #[arg(long, default_value = "auto")]
        shellcheck: String,
        /// Path to the shellcheck executable (default: `shellcheck` on `PATH`).
        #[arg(long)]
        shellcheck_path: Option<String>,
    },
    /// Advanced: serve one describe/emit/synth request for a built-in
    /// language over stdin/stdout, then exit. This is the reference a
    /// custom plugin executable's protocol responses are checked against.
    #[command(hide = true)]
    Plugin {
        /// One of: typescript, python, go, rust.
        language: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Actions { args } => actions::run(&resolve_root(cli.root)?, &args),
        Command::Secrets { args } => secrets::run(&resolve_root(cli.root)?, &args),
        Command::Plugin { language } => {
            let root = resolve_root(cli.root)?;
            plugin::serve_builtin_once(&language, &root)
        }
        Command::Init {
            language,
            name,
            template,
        } => {
            let root = resolve_root(cli.root)?;
            init::run(
                &root,
                init::InitOptions {
                    language,
                    name,
                    template,
                },
            )
        }
        Command::Import {
            paths,
            language,
            force,
            plugin,
            plugin_arg,
        } => {
            let root = resolve_root(cli.root)?;
            let plugin = plugin.map(|program| PluginSpec {
                program,
                args: plugin_arg,
            });
            import::run(
                &root,
                import::ImportOptions {
                    paths,
                    language,
                    force,
                    plugin,
                },
            )
        }
        Command::Gen {
            sources,
            force,
            no_lint,
            check,
        } => {
            let root = resolve_root(cli.root)?;
            gen::run(
                &root,
                gen::GenOptions {
                    sources,
                    force,
                    lint: !no_lint,
                    check,
                },
            )
        }
        Command::Lint {
            paths,
            json,
            persona,
            context,
            shellcheck,
            shellcheck_path,
        } => {
            let root = resolve_root(cli.root)?;
            let persona = parse_persona(&persona)?;
            let shellcheck = ShellcheckMode::parse(&shellcheck)?;
            lint::run(
                &root,
                lint::LintOptions {
                    paths,
                    json,
                    persona,
                    context,
                    shellcheck: ShellcheckOptions {
                        mode: shellcheck,
                        path: shellcheck_path,
                    },
                },
            )
        }
    }
}

fn parse_persona(value: &str) -> Result<Persona> {
    match value {
        "regular" => Ok(Persona::Regular),
        "pedantic" => Ok(Persona::Pedantic),
        "auditor" => Ok(Persona::Auditor),
        other => anyhow::bail!("unknown persona {other:?}; expected regular, pedantic, or auditor"),
    }
}

fn resolve_root(root: Option<PathBuf>) -> Result<PathBuf> {
    let root = root.unwrap_or_else(|| PathBuf::from("."));
    root.canonicalize()
        .with_context(|| format!("resolve repository root {}", root.display()))
}
