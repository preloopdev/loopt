//! The `loopt plugin <language>` process protocol: a single JSON request on
//! stdin produces a single JSON response on stdout, then the process exits.
//! No daemon, no LSP.
//!
//! Loopt's four built-in language engines answer this protocol in-process
//! (`handle_builtin`, wired up to the hidden `loopt plugin <language>`
//! subcommand by `serve_builtin_once`); a custom, non-built-in `--language`
//! is paired with an external executable (`PluginSpec`) that speaks the
//! exact same request/response shapes over its own stdin/stdout. Core
//! treats both identically everywhere above this module.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::exec;
use crate::languages::{GeneratedSource, Language};

/// Protocol version this build speaks. A mismatch is a hard error on
/// either side, never silently coerced.
pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", content = "params", rename_all = "lowercase")]
enum PluginRequest {
    Describe,
    Emit {
        workflow: serde_json::Value,
        stem: PathBuf,
    },
    Synth {
        source: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RequestEnvelope {
    version: u32,
    #[serde(flatten)]
    request: PluginRequest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
enum ResponseEnvelope {
    Ok {
        version: u32,
        result: serde_json::Value,
    },
    Error {
        version: u32,
        message: String,
    },
}

/// What `describe` reports about a language, built-in or custom.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCapabilities {
    /// The language id this plugin answers to; must equal the `--language`
    /// it was configured under.
    pub language: String,
    /// Source file extensions this language's entrypoint may use, without
    /// the leading dot. Empty for a language whose entrypoint is a
    /// directory (e.g. a standalone Rust project).
    pub extensions: Vec<String>,
}

/// A custom, externally executed plugin: one program plus its fixed
/// argument vector. Never invoked through a shell. Persisted verbatim in
/// `.loopt/manifest.json` entries for languages that are not built in.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl PluginSpec {
    fn call(&self, root: &Path, request: PluginRequest) -> Result<serde_json::Value> {
        let envelope = RequestEnvelope {
            version: PROTOCOL_VERSION,
            request,
        };
        let payload = serde_json::to_vec(&envelope).context("serialize plugin request")?;
        let mut child = Command::new(&self.program)
            .args(&self.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("spawn plugin {}", self.program))?;
        let mut stdin = child.stdin.take().expect("stdin was piped");
        let writer = thread::spawn(move || stdin.write_all(&payload));
        let output = child
            .wait_with_output()
            .with_context(|| format!("run plugin {}", self.program))?;
        writer
            .join()
            .map_err(|_| anyhow::anyhow!("plugin stdin writer thread panicked"))?
            .with_context(|| format!("write request to plugin {}", self.program))?;
        if !output.status.success() {
            bail!(
                "plugin {} exited with {}: {}",
                self.program,
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let response: ResponseEnvelope =
            serde_json::from_slice(&output.stdout).with_context(|| {
                format!(
                    "plugin {} returned an invalid response: {}",
                    self.program,
                    String::from_utf8_lossy(&output.stdout)
                )
            })?;
        match response {
            ResponseEnvelope::Ok { version, result } => {
                if version != PROTOCOL_VERSION {
                    bail!(
                        "plugin {} speaks protocol version {version}, expected {PROTOCOL_VERSION}",
                        self.program
                    );
                }
                Ok(result)
            }
            ResponseEnvelope::Error { message, .. } => {
                bail!("plugin {} reported an error: {message}", self.program)
            }
        }
    }

    /// Validate the plugin and learn its declared language id and
    /// capabilities. Callers should check the returned `language` matches
    /// the `--language` the plugin was configured under before trusting it.
    pub fn describe(&self, root: &Path) -> Result<PluginCapabilities> {
        let value = self.call(root, PluginRequest::Describe)?;
        serde_json::from_value(value)
            .with_context(|| format!("plugin {}: invalid describe response", self.program))
    }

    /// Emit authoring source for `workflow` rooted at `stem`.
    pub fn emit(
        &self,
        root: &Path,
        workflow: &serde_json::Value,
        stem: &Path,
    ) -> Result<GeneratedSource> {
        let value = self.call(
            root,
            PluginRequest::Emit {
                workflow: workflow.clone(),
                stem: stem.to_path_buf(),
            },
        )?;
        serde_json::from_value(value)
            .with_context(|| format!("plugin {}: invalid emit response", self.program))
    }

    /// Run `source` and return the workflow value it synthesized. Unlike
    /// built-in languages, the plugin is solely responsible for how it
    /// compiles or runs `source`; core never inspects it.
    pub fn synth(&self, root: &Path, source: &Path) -> Result<serde_json::Value> {
        let value = self.call(
            root,
            PluginRequest::Synth {
                source: source.to_path_buf(),
            },
        )?;
        value
            .as_object()
            .and_then(|obj| obj.get("workflow"))
            .cloned()
            .with_context(|| format!("plugin {}: synth response missing `workflow`", self.program))
    }
}

/// Handle one request against a built-in language engine, in-process. This
/// is both what `loopt plugin <language>` runs for a built-in language id,
/// and the reference the custom-plugin protocol above is meant to match.
fn handle_builtin(
    language: Language,
    request: PluginRequest,
    root: &Path,
) -> Result<serde_json::Value> {
    match request {
        PluginRequest::Describe => Ok(serde_json::to_value(PluginCapabilities {
            language: language.id().to_string(),
            extensions: language
                .source_extensions()
                .iter()
                .map(|ext| ext.to_string())
                .collect(),
        })?),
        PluginRequest::Emit { workflow, stem } => {
            let source = language.emit(&workflow, &stem)?;
            Ok(serde_json::to_value(source)?)
        }
        PluginRequest::Synth { source } => {
            let command = language.command(&source)?;
            let workflow = exec::run_source(&command, root)?;
            Ok(serde_json::json!({ "workflow": workflow }))
        }
    }
}

/// Run the hidden `loopt plugin <language>` subcommand: read exactly one
/// request from stdin, write exactly one response to stdout, then return.
/// `language_id` must name one of the four built-in languages.
pub fn serve_builtin_once(language_id: &str, root: &Path) -> Result<()> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .context("read plugin request from stdin")?;

    let response = respond(language_id, &input, root);
    let is_error = matches!(response, ResponseEnvelope::Error { .. });

    let mut stdout = std::io::stdout();
    serde_json::to_writer(&mut stdout, &response).context("write plugin response")?;
    stdout.write_all(b"\n").context("write plugin response")?;

    if is_error {
        std::process::exit(1);
    }
    Ok(())
}

fn respond(language_id: &str, input: &str, root: &Path) -> ResponseEnvelope {
    let outcome = (|| -> Result<serde_json::Value> {
        let envelope: RequestEnvelope =
            serde_json::from_str(input).context("parse plugin request")?;
        if envelope.version != PROTOCOL_VERSION {
            bail!(
                "unsupported protocol version {} (expected {PROTOCOL_VERSION})",
                envelope.version
            );
        }
        let language = Language::parse(language_id)
            .with_context(|| format!("{language_id}: not a built-in language"))?;
        handle_builtin(language, envelope.request, root)
    })();
    match outcome {
        Ok(result) => ResponseEnvelope::Ok {
            version: PROTOCOL_VERSION,
            result,
        },
        Err(error) => ResponseEnvelope::Error {
            version: PROTOCOL_VERSION,
            message: format!("{error:#}"),
        },
    }
}
