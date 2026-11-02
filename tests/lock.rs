//! Deterministic integration coverage for `loopt lock`.
//!
//! The fake `gh` resolves any ref to `$GH_SHA`, serves a composite
//! `action.yml` for `octo/parent`, plain metadata for `octo/leaf`, and
//! fixed repository ids via `GH_OWNER_ID`/`GH_REPO_ID`. No test reaches the
//! network.

#![cfg(unix)]

use std::env;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use loopt::lock;

const WORKFLOW: &str = r#"name: ci
on: push
jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - uses: octo/parent@v1
      - uses: octo/leaf@v2
      - uses: ./local-action
      - run: echo hi
"#;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const COMPOSITE_ACTION: &str = r#"name: Parent
description: Composite fixture.
runs:
  using: composite
  steps:
    - uses: octo/leaf@v2
    - uses: ./nested-local
    - run: echo nested
      shell: bash
"#;

const LEAF_ACTION: &str = r#"name: Leaf
description: Leaf fixture.
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
  repos/octo/parent/contents/action.yml?ref=*)
    cat "$GH_PARENT"
    ;;
  repos/octo/parent/contents/action.yaml?ref=*)
    exit 1
    ;;
  repos/octo/leaf/contents/action.yml?ref=*)
    cat "$GH_LEAF"
    ;;
  repos/octo/leaf/contents/action.yaml?ref=*)
    exit 1
    ;;
  repos/octo/parent|repos/octo/leaf)
    printf '{"id":%s,"owner":{"id":%s}}\n' "$GH_REPO_ID" "$GH_OWNER_ID"
    ;;
  *)
    printf 'unexpected gh api endpoint: %s\n' "$2" >&2
    exit 1
    ;;
esac
"#;

struct TempRepo {
    path: PathBuf,
}

impl TempRepo {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the Unix epoch")
            .as_nanos();
        let path =
            env::temp_dir().join(format!("loopt-lock-{label}-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).expect("create temporary repository");
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
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct GhEnv {
    saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl GhEnv {
    const KEYS: [&'static str; 8] = [
        "LOOPT_GH",
        "GH_LOG",
        "GH_SHA",
        "GH_PARENT",
        "GH_LEAF",
        "GH_OWNER_ID",
        "GH_REPO_ID",
        "PATH",
    ];

    fn install(repo: &TempRepo, sha: &str) -> Self {
        let saved: Vec<(&'static str, Option<std::ffi::OsString>)> = Self::KEYS
            .iter()
            .map(|key| (*key, env::var_os(key)))
            .collect();

        let gh = repo.write("fake-bin/gh", GH_SCRIPT);
        let mut permissions = fs::metadata(&gh).expect("stat fake gh").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&gh, permissions).expect("make fake gh executable");

        let mut path = repo.path.join("fake-bin").into_os_string();
        if let Some(old) = &saved[7].1 {
            path.push(":");
            path.push(old);
        }
        env::set_var("PATH", path);
        env::set_var("LOOPT_GH", &gh);
        env::set_var("GH_LOG", repo.path.join("gh.log"));
        env::set_var("GH_SHA", sha);
        env::set_var(
            "GH_PARENT",
            repo.write("parent-action.yml", COMPOSITE_ACTION),
        );
        env::set_var("GH_LEAF", repo.write("leaf-action.yml", LEAF_ACTION));
        env::set_var("GH_OWNER_ID", "42");
        env::set_var("GH_REPO_ID", "4242");
        Self { saved }
    }

    fn set_sha(&self, sha: &str) {
        env::set_var("GH_SHA", sha);
    }
}

impl Drop for GhEnv {
    fn drop(&mut self) {
        for (key, value) in self.saved.drain(..) {
            if let Some(value) = value {
                env::set_var(key, value);
            } else {
                env::remove_var(key);
            }
        }
    }
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn lock_writes_commit_pins_and_transitive_closure() {
    let _guard = ENV_LOCK.lock().expect("lock env mutex");
    let repo = TempRepo::new("write");
    let _env = GhEnv::install(&repo, SHA);
    repo.write(".github/workflows/ci.yml", WORKFLOW);

    lock::run(&repo.path, &args(&[])).expect("lock run");

    let text = repo.read(".github/workflows/actions.lock");
    assert!(text.contains("version: 'v0.0.2'"), "{text}");
    // The composite parent contributes its nested uses: list.
    assert!(text.contains("'octo/parent@v1':"), "{text}");
    assert!(text.contains(&format!("commit: 'sha1-{SHA}'")), "{text}");
    assert!(text.contains("owner_id: 42"), "{text}");
    assert!(text.contains("repo_id: 4242"), "{text}");
    assert!(text.contains("      - 'octo/leaf@v2'"), "{text}");
    // The workflow lists its full transitive closure.
    assert!(text.contains("'.github/workflows/ci.yml':"), "{text}");
    // Local ./ actions are never locked.
    assert!(!text.contains("local-action"), "{text}");
    assert!(!text.contains("nested-local"), "{text}");
}

#[test]
fn verify_local_accepts_fresh_lockfile() {
    let _guard = ENV_LOCK.lock().expect("lock env mutex");
    let repo = TempRepo::new("verify-local-ok");
    let _env = GhEnv::install(&repo, SHA);
    repo.write(".github/workflows/ci.yml", WORKFLOW);
    lock::run(&repo.path, &args(&[])).expect("lock run");

    lock::run(&repo.path, &args(&["--verify-local"])).expect("verify-local");
    lock::run(&repo.path, &args(&["--verify"])).expect("verify");
}

#[test]
fn verify_local_flags_missing_and_stale_entries() {
    let _guard = ENV_LOCK.lock().expect("lock env mutex");
    let repo = TempRepo::new("verify-local-drift");
    let _env = GhEnv::install(&repo, SHA);
    repo.write(".github/workflows/ci.yml", WORKFLOW);
    lock::run(&repo.path, &args(&[])).expect("lock run");

    // A workflow uses: the lockfile never saw is a coverage error.
    repo.write(
        ".github/workflows/extra.yml",
        "name: x\non: push\njobs:\n  j:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: octo/new@v9\n",
    );
    let result = lock::run(&repo.path, &args(&["--verify-local"]));
    assert!(result.is_err(), "verify-local must fail on missing entry");

    // Lockfile-only entries are stale.
    fs::remove_file(repo.path.join(".github/workflows/extra.yml")).expect("remove extra");
    let text = repo.read(".github/workflows/actions.lock");
    let mutated = text.replace(
        "workflows:",
        "  'octo/unused@v1':\n    ref: 'v1'\n    commit: 'sha1-0000000000000000000000000000000000000000'\n    owner_id: 1\n    repo_id: 2\nworkflows:",
    );
    repo.write(".github/workflows/actions.lock", &mutated);
    let result = lock::run(&repo.path, &args(&["--verify-local"]));
    assert!(result.is_err(), "verify-local must fail on stale entry");
}

#[test]
fn verify_flags_moved_refs() {
    let _guard = ENV_LOCK.lock().expect("lock env mutex");
    let repo = TempRepo::new("verify-moved");
    let env = GhEnv::install(&repo, SHA);
    repo.write(".github/workflows/ci.yml", WORKFLOW);
    lock::run(&repo.path, &args(&[])).expect("lock run");

    // The ref now resolves somewhere else: online verify must notice.
    env.set_sha("abcdef0123456789abcdef0123456789abcdef01");
    let result = lock::run(&repo.path, &args(&["--verify"]));
    assert!(result.is_err(), "verify must fail when a ref moved");

    // Offline coverage is unaffected by ref drift.
    lock::run(&repo.path, &args(&["--verify-local"])).expect("verify-local ignores ref drift");
}
