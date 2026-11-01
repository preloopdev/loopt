//! Command-line entry point for the TypeScript SDK generator.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use loopt_sdkgen::{generate_typescript, stale_files, write_all};

/// The default output directory, relative to this crate's manifest directory.
const DEFAULT_OUT: &str = "../../sdk/typescript/src/generated";

/// Usage text shown for `--help` and argument errors.
const USAGE: &str = "usage: loopt-sdkgen typescript [--out <dir>] [--check]";

/// The hint printed when `--check` finds stale files.
const REGENERATE_HINT: &str = "run: cargo run -p loopt-sdkgen -- typescript";

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("loopt-sdkgen: {error:#}");
            ExitCode::from(2)
        }
    }
}

/// Parses arguments, generates the files, and either writes or checks them.
fn run(args: Vec<String>) -> Result<ExitCode> {
    let mut args = args.into_iter();
    let command = match args.next() {
        Some(command) => command,
        None => bail!("{USAGE}"),
    };

    match command.as_str() {
        "--help" | "-h" => {
            println!("{USAGE}");
            return Ok(ExitCode::SUCCESS);
        }
        "typescript" => {}
        other => bail!("unknown command `{other}`\n{USAGE}"),
    }

    let mut out_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_OUT);
    let mut check = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => {
                let value = args
                    .next()
                    .with_context(|| format!("`--out` requires a directory\n{USAGE}"))?;
                out_dir = PathBuf::from(value);
            }
            "--check" => check = true,
            "--help" | "-h" => {
                println!("{USAGE}");
                return Ok(ExitCode::SUCCESS);
            }
            other => {
                if let Some(value) = other.strip_prefix("--out=") {
                    if value.is_empty() {
                        bail!("`--out` requires a directory\n{USAGE}");
                    }
                    out_dir = PathBuf::from(value);
                } else {
                    bail!("unknown argument `{other}`\n{USAGE}");
                }
            }
        }
    }

    let files = generate_typescript().context("generating TypeScript SDK sources")?;

    if check {
        let stale =
            stale_files(&out_dir, &files).context("checking generated TypeScript sources")?;
        if stale.is_empty() {
            return Ok(ExitCode::SUCCESS);
        }
        for path in &stale {
            println!("{path}");
        }
        eprintln!("{REGENERATE_HINT}");
        return Ok(ExitCode::from(1));
    }

    write_all(&out_dir, &files).context("writing generated TypeScript sources")?;
    println!(
        "wrote {} generated TypeScript files to {}",
        files.len(),
        out_dir.display()
    );
    Ok(ExitCode::SUCCESS)
}
