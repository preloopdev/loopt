//! Shared `gh api` plumbing for modules that read GitHub metadata.
//!
//! Everything here goes through the authenticated `gh` executable directly
//! (never a shell) so `gh auth login`, `GH_HOST`, enterprise host settings,
//! and the `LOOPT_GH` test override all behave the same way for action
//! bindings, the secrets inventory, and the workflow lockfile.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value as JsonValue;

const DEFAULT_GH_BINARY: &str = "gh";

/// Run `gh api <endpoint>` and return the response body.
///
/// `raw` requests `application/vnd.github.raw+json`, used for contents-API
/// file payloads; otherwise the caller gets the JSON envelope.
pub(crate) fn api(root: &Path, endpoint: &str, raw: bool) -> Result<Vec<u8>> {
    let gh = std::env::var_os("LOOPT_GH").unwrap_or_else(|| OsString::from(DEFAULT_GH_BINARY));
    let mut command = Command::new(gh);
    command.current_dir(root).arg("api").arg(endpoint);
    if raw {
        command.args(["--header", "Accept: application/vnd.github.raw+json"]);
    }
    let output = command.output().with_context(|| {
        "run `gh api`; install GitHub CLI and authenticate with `gh auth login`"
    })?;
    if !output.status.success() {
        bail!(
            "gh api request failed with {}; authenticate with `gh auth login` and check repository/ref",
            output.status
        );
    }
    Ok(output.stdout)
}

/// Resolve `owner/repo@requested_ref` to a full lowercase 40-char commit SHA.
pub(crate) fn resolve_commit(
    root: &Path,
    owner: &str,
    repo: &str,
    requested_ref: &str,
) -> Result<String> {
    let commit_endpoint = format!("repos/{owner}/{repo}/commits/{}", encode_ref(requested_ref));
    let bytes = api(root, &commit_endpoint, false)
        .with_context(|| format!("resolve {owner}/{repo}@{requested_ref} to a commit"))?;
    let text = String::from_utf8(bytes).context("gh returned non-UTF-8 commit metadata")?;
    let trimmed = text.trim();
    let candidate = if let Ok(value) = serde_json::from_str::<JsonValue>(trimmed) {
        match &value {
            JsonValue::Object(object) => object
                .get("sha")
                .and_then(JsonValue::as_str)
                .context("commit response did not contain a sha")?
                .to_string(),
            JsonValue::String(value) => value.clone(),
            _ => bail!("commit response did not contain a sha"),
        }
    } else {
        trimmed.to_string()
    };
    let sha = candidate.trim().to_ascii_lowercase();
    validate_sha(&sha).context("gh did not resolve the ref to a full commit SHA")?;
    Ok(sha)
}

/// Fetch a file from `owner/repo/path` at `sha` through the contents API.
///
/// Accepts either the JSON envelope (base64 `content`) or the raw media-type
/// body; both are produced by `Accept` negotiation differences across gh
/// versions and by test fixtures.
pub(crate) fn fetch_file(
    root: &Path,
    owner: &str,
    repo: &str,
    path: &str,
    sha: &str,
) -> Result<Vec<u8>> {
    let mut endpoint = format!("repos/{owner}/{repo}/contents");
    for component in path.split('/') {
        endpoint.push('/');
        endpoint.push_str(&encode_path_component(component));
    }
    endpoint.push_str("?ref=");
    endpoint.push_str(sha);
    let bytes =
        api(root, &endpoint, true).with_context(|| format!("read {owner}/{repo}/{path}@{sha}"))?;
    decode_contents_response(&bytes)
}

/// Fetch `(owner_id, repo_id)` for `owner/repo`, as recorded in the GitHub
/// workflow lockfile.
pub(crate) fn repo_ids(root: &Path, owner: &str, repo: &str) -> Result<(u64, u64)> {
    let bytes = api(root, &format!("repos/{owner}/{repo}"), false)
        .with_context(|| format!("read repository ids for {owner}/{repo}"))?;
    let value: JsonValue =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {owner}/{repo} metadata"))?;
    let object = value
        .as_object()
        .with_context(|| format!("{owner}/{repo}: expected object metadata"))?;
    let repo_id = object
        .get("id")
        .and_then(JsonValue::as_u64)
        .with_context(|| format!("{owner}/{repo}: missing numeric id"))?;
    let owner_id = object
        .get("owner")
        .and_then(JsonValue::as_object)
        .and_then(|owner| owner.get("id"))
        .and_then(JsonValue::as_u64)
        .with_context(|| format!("{owner}/{repo}: missing owner id"))?;
    Ok((owner_id, repo_id))
}

/// Reject refs that could alter the API path or carry injection payloads.
pub(crate) fn validate_ref(value: &str) -> Result<()> {
    if value.is_empty()
        || value.trim() != value
        || value.chars().any(|character| {
            character.is_ascii_control()
                || character.is_whitespace()
                || matches!(character, '?' | '#' | '%' | '\\')
        })
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains("//")
    {
        bail!("invalid GitHub ref {value:?}");
    }
    Ok(())
}

/// Require a full lowercase 40-character commit SHA.
pub(crate) fn validate_sha(value: &str) -> Result<()> {
    if value.len() != 40
        || !value.chars().all(|character| character.is_ascii_hexdigit())
        || value != value.to_ascii_lowercase()
    {
        bail!("expected a full lowercase 40-character commit SHA, got {value:?}");
    }
    Ok(())
}

/// Percent-encode a ref for the commits endpoint, keeping `/` for branch names.
pub(crate) fn encode_ref(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | '~' | '/')
            {
                character.to_string()
            } else {
                format!("%{:02X}", character as u32)
            }
        })
        .collect()
}

/// Percent-encode one path component for the contents endpoint.
pub(crate) fn encode_path_component(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

fn decode_contents_response(bytes: &[u8]) -> Result<Vec<u8>> {
    let Ok(value) = serde_json::from_slice::<JsonValue>(bytes) else {
        return Ok(bytes.to_vec());
    };
    let Some(object) = value.as_object() else {
        return Ok(bytes.to_vec());
    };
    if let Some(message) = object.get("message").and_then(JsonValue::as_str) {
        bail!("GitHub API: {message}");
    }
    if let Some(content) = object.get("content").and_then(JsonValue::as_str) {
        return decode_base64(content);
    }
    // A JSON object can itself be a valid file payload (a fixture or a raw
    // JSON document); preserve it rather than requiring the envelope.
    Ok(bytes.to_vec())
}

fn decode_base64(value: &str) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut quartet = [0u8; 4];
    let mut count = 0usize;
    for byte in value.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        quartet[count] = byte;
        count += 1;
        if count == 4 {
            let a = base64_value(quartet[0])?;
            let b = base64_value(quartet[1])?;
            let c = if quartet[2] == b'=' {
                0
            } else {
                base64_value(quartet[2])?
            };
            let d = if quartet[3] == b'=' {
                0
            } else {
                base64_value(quartet[3])?
            };
            output.push((a << 2) | (b >> 4));
            if quartet[2] != b'=' {
                output.push((b << 4) | (c >> 2));
            }
            if quartet[3] != b'=' {
                output.push((c << 6) | d);
            }
            count = 0;
        }
    }
    if count != 0 {
        bail!("GitHub contents response contained invalid base64");
    }
    Ok(output)
}

fn base64_value(byte: u8) -> Result<u8> {
    match byte {
        b'A'..=b'Z' => Ok(byte - b'A'),
        b'a'..=b'z' => Ok(byte - b'a' + 26),
        b'0'..=b'9' => Ok(byte - b'0' + 52),
        b'+' => Ok(62),
        b'/' => Ok(63),
        _ => bail!("invalid base64 in GitHub contents response"),
    }
}
