//! Deterministic integration coverage for typed action bindings.
//!
//! The fake `gh` executable is deliberately tiny: it records every API call,
//! resolves a ref to a configurable full SHA, and serves one local action
//! manifest.  No test reaches the network or handles a secret value.

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use loopt::actions;

const ACTION_YAML: &str = r#"name: Fixture action
description: A deterministic fixture action.
inputs:
  token:
    description: Authentication token.
    required: true
  dry-run:
    description: Whether to avoid changes.
    required: false
outputs:
  artifact-name:
    description: Name of the uploaded artifact.
runs:
  using: node20
  main: index.js
"#;

const GH_SCRIPT: &str = r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$GH_LOG"
case "$2" in
  */commits/*)
    printf '{"sha":"%s"}\n' "$GH_SHA"
    ;;
  */contents/action.yml?ref=*)
    cat "$GH_ACTION"
    ;;
  */contents/action.yaml?ref=*)
    exit 1
    ;;
  *)
    printf 'unexpected gh api endpoint: %s\n' "$2" >&2
    exit 1
    ;;
esac
"#;

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct TempRepo {
    path: PathBuf,
}

impl TempRepo {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "loopt-actions-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary repository");
        // The CLI canonicalizes the repository root before dispatch; the
        // library path guards reject symlinked components, so tests must
        // present the same canonicalized root (macOS temp dirs live under
        // a /var -> /private/var symlink).
        let path = path
            .canonicalize()
            .expect("canonicalize temporary repository");
        Self { path }
    }

    fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.path.join(relative);
        fs::create_dir_all(path.parent().expect("file has a parent")).expect("create parent");
        fs::write(&path, content).expect("write fixture file");
        path
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path.join(relative)).expect("read fixture file")
    }

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct GhEnv {
    old_path: Option<std::ffi::OsString>,
    old_loopt_gh: Option<std::ffi::OsString>,
    old_log: Option<std::ffi::OsString>,
    old_action: Option<std::ffi::OsString>,
    old_sha: Option<std::ffi::OsString>,
}

impl GhEnv {
    fn install(repo: &TempRepo, sha: &str) -> Self {
        let bin = repo.path.join("fake-bin");
        fs::create_dir_all(&bin).expect("create fake gh bin");
        let gh = repo.write("fake-bin/gh", GH_SCRIPT);
        let mut permissions = fs::metadata(&gh).expect("stat fake gh").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&gh, permissions).expect("make fake gh executable");
        let old_path = env::var_os("PATH");
        let old_loopt_gh = env::var_os("LOOPT_GH");
        let old_log = env::var_os("GH_LOG");
        let old_action = env::var_os("GH_ACTION");
        let old_sha = env::var_os("GH_SHA");
        let mut path = bin.into_os_string();
        if let Some(old) = &old_path {
            path.push(":");
            path.push(old);
        }
        let log = repo.path.join("gh.log");
        let action = repo.write("fixture-action.yml", ACTION_YAML);
        env::set_var("PATH", path);
        // `gh_api` honors LOOPT_GH first; pinning it to the fixture keeps an
        // ambient developer/CI override from routing calls at the real `gh`.
        env::set_var("LOOPT_GH", &gh);
        env::set_var("GH_LOG", &log);
        env::set_var("GH_ACTION", &action);
        env::set_var("GH_SHA", sha);
        Self {
            old_path,
            old_loopt_gh,
            old_log,
            old_action,
            old_sha,
        }
    }

    fn calls(&self, repo: &TempRepo) -> usize {
        repo.read("gh.log").lines().count()
    }
}

impl Drop for GhEnv {
    fn drop(&mut self) {
        if let Some(value) = self.old_path.take() {
            env::set_var("PATH", value);
        } else {
            env::remove_var("PATH");
        }
        if let Some(value) = self.old_loopt_gh.take() {
            env::set_var("LOOPT_GH", value);
        } else {
            env::remove_var("LOOPT_GH");
        }
        if let Some(value) = self.old_log.take() {
            env::set_var("GH_LOG", value);
        } else {
            env::remove_var("GH_LOG");
        }
        if let Some(value) = self.old_action.take() {
            env::set_var("GH_ACTION", value);
        } else {
            env::remove_var("GH_ACTION");
        }
        if let Some(value) = self.old_sha.take() {
            env::set_var("GH_SHA", value);
        } else {
            env::remove_var("GH_SHA");
        }
    }
}

fn run(repo: &TempRepo, values: &[&str]) -> anyhow::Result<()> {
    actions::run(&repo.path, &TempRepo::args(values))
}

#[test]
fn add_pins_commit_generates_typed_binding_and_verify_is_offline() {
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let repo = TempRepo::new("add");
    let first_sha = "0123456789abcdef0123456789abcdef01234567";
    let gh = GhEnv::install(&repo, first_sha);

    run(
        &repo,
        &[
            "add",
            "acme/fixture@v1",
            "--input-type",
            "dry-run=boolean",
            "--output",
            "digest",
            "--sdk-import",
            "./sdk.ts",
        ],
    )
    .expect("add succeeds with fake gh");

    let generated = repo.read(".loopt/actions/fixture.ts");
    assert!(generated.contains("import { uses } from \"./sdk.ts\";"));
    assert!(generated.contains("acme/fixture@0123456789abcdef0123456789abcdef01234567"));
    assert!(generated.contains("readonly token: string | Expr<string>"));
    assert!(generated.contains("readonly \"dry-run\"?: boolean | Expr<boolean>"));
    assert!(generated.contains("\"artifact-name\" | \"digest\""));
    assert!(repo
        .read(".loopt/actions.lock")
        .contains("\"sha\": \"0123456789abcdef0123456789abcdef01234567\""));

    let calls_before_verify = gh.calls(&repo);
    run(&repo, &["verify"]).expect("verify succeeds without a network call");
    assert_eq!(
        gh.calls(&repo),
        calls_before_verify,
        "verify must not invoke gh"
    );

    repo.write(
        ".loopt/actions/fixture.ts",
        &(generated.clone() + "// tampered\n"),
    );
    assert!(
        run(&repo, &["verify"]).is_err(),
        "tampering must be detected"
    );
}

#[test]
fn update_refreshes_pin_and_remove_cleans_generated_binding() {
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let repo = TempRepo::new("update");
    let first_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let second_sha = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let _gh = GhEnv::install(&repo, first_sha);
    run(&repo, &["add", "acme/fixture@stable"]).expect("initial add succeeds");

    env::set_var("GH_SHA", second_sha);
    run(&repo, &["update", "fixture"]).expect("update succeeds");
    assert!(repo.read(".loopt/actions/fixture.ts").contains(second_sha));
    assert!(repo.read(".loopt/actions.lock").contains(second_sha));

    run(&repo, &["remove", "fixture"]).expect("remove succeeds");
    assert!(!repo.path.join(".loopt/actions/fixture.ts").exists());
    assert!(repo.read(".loopt/actions.lock").contains("\"entries\": []"));
}

#[test]
fn unsafe_specs_and_manual_files_are_rejected_without_writes() {
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let repo = TempRepo::new("safety");
    let sha = "cccccccccccccccccccccccccccccccccccccccc";
    let _gh = GhEnv::install(&repo, sha);

    assert!(run(&repo, &["add", "acme/../escape@v1"]).is_err());
    assert!(!repo.path.join(".loopt/actions.lock").exists());

    let manual = repo.write(".loopt/actions/fixture.ts", "// user file\n");
    assert!(run(&repo, &["add", "acme/fixture@v1"]).is_err());
    assert_eq!(fs::read_to_string(manual).unwrap(), "// user file\n");
    assert!(!repo.path.join(".loopt/actions.lock").exists());
}

#[test]
fn malformed_metadata_and_short_commits_fail_informatively() {
    let _guard = ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
    let repo = TempRepo::new("invalid");
    let _gh = GhEnv::install(&repo, "not-a-commit");
    let error = run(&repo, &["add", "acme/fixture@v1"]).expect_err("short SHA must fail");
    assert!(error.to_string().contains("full commit SHA"));
}

#[allow(dead_code)]
fn _assert_path_is_inside(root: &Path, path: &Path) {
    assert!(path.starts_with(root));
}
