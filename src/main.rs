//! `loopt` CLI: argument parsing and dispatch only. The actual `import` and
//! `gen` behavior lives in the `loopt` library crate (`src/lib.rs`).

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use loopt::plugin::PluginSpec;
use loopt::{gen, import, plugin};

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
        Command::Plugin { language } => {
            let root = resolve_root(cli.root)?;
            plugin::serve_builtin_once(&language, &root)
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
        Command::Gen { sources, force } => {
            let root = resolve_root(cli.root)?;
            gen::run(&root, gen::GenOptions { sources, force })
        }
    }
}

fn resolve_root(root: Option<PathBuf>) -> Result<PathBuf> {
    let root = root.unwrap_or_else(|| PathBuf::from("."));
    root.canonicalize()
        .with_context(|| format!("resolve repository root {}", root.display()))
}
