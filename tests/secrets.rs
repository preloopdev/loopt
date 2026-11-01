//! Integration coverage for the names-only GitHub secret inventory.
//!
//! The fake `gh` executable below is deliberately an HTTP-fixture stand-in:
//! each endpoint returns paginated REST-shaped JSON, and no test ever supplies
//! a secret value to the implementation's output path.

use std::fs;
use std::panic::{catch_unwind, resume_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use loopt::secrets;
use preloop_gha_lint::LintContext;
use serde_json::Value;

static GH_ENV_LOCK: Mutex<()> = Mutex::new(());

struct TempRepo {
    path: PathBuf,
}

impl TempRepo {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!("loopt-secrets-{label}-{nonce}"));
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

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&self, relative: &str, content: &str) {
        let path = self.path.join(relative);
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        fs::write(path, content).expect("write fixture");
    }

    fn read(&self, relative: &str) -> String {
        fs::read_to_string(self.path.join(relative)).expect("read fixture")
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn with_fake_gh<T>(repo: &TempRepo, script: &str, action: impl FnOnce() -> T) -> T {
    let _lock = GH_ENV_LOCK.lock().expect("gh fixture lock");
    let gh = repo.path.join("fake-gh");
    fs::write(&gh, format!("#!/bin/sh\nset -eu\n{script}\n")).expect("write fake gh");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&gh).expect("stat fake gh").permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&gh, permissions).expect("chmod fake gh");
    }

    let old = std::env::var_os("LOOPT_GH");
    std::env::set_var("LOOPT_GH", &gh);
    let result = catch_unwind(AssertUnwindSafe(action));
    match old {
        Some(value) => std::env::set_var("LOOPT_GH", value),
        None => std::env::remove_var("LOOPT_GH"),
    }
    match result {
        Ok(value) => value,
        Err(payload) => resume_unwind(payload),
    }
}

fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

#[test]
fn sync_paginates_and_emits_names_only_lint_context_and_sdk_accessors() {
    let repo = TempRepo::new("pagination");
    let script = r#"
endpoint=""
for arg in "$@"; do
  case "$arg" in repos/*|orgs/*) endpoint="$arg" ;; esac
done
case "$endpoint" in
  repos/acme/widget)
    printf '%s\n' '{"id":7,"full_name":"acme/widget","private":false,"owner":{"login":"acme","type":"Organization"}}'
    ;;
  repos/acme/widget/actions/secrets)
    printf '%s\n' '[{"secrets":[{"name":"ZETA","value":"TOP_SECRET"}]},{"secrets":[{"name":"ALPHA"}]}]'
    ;;
  orgs/acme/actions/secrets)
    printf '%s\n' '[{"secrets":[{"name":"ORG_ALL","visibility":"all"}]},{"secrets":[{"name":"ORG_SELECTED","visibility":"selected"},{"name":"ORG_PRIVATE","visibility":"private"}]}]'
    ;;
  orgs/acme/actions/secrets/ORG_SELECTED/repositories)
    printf '%s\n' '[{"repositories":[{"id":7,"full_name":"acme/widget"}]}]'
    ;;
  repos/acme/widget/environments)
    printf '%s\n' '[{"environments":[{"name":"prod"}]},{"environments":[{"name":"dev"}]}]'
    ;;
  repos/acme/widget/environments/prod/secrets)
    printf '%s\n' '{"secrets":[{"name":"DEPLOY_KEY","value":"ENV_SECRET_VALUE"}]}'
    ;;
  repos/acme/widget/environments/dev/secrets)
    printf '%s\n' '{"secrets":[]}'
    ;;
  *) exit 91 ;;
esac
"#;

    with_fake_gh(&repo, script, || {
        secrets::run(repo.path(), &args(&["sync", "--repo", "acme/widget"]))
            .expect("sync succeeds");
    });

    let json = repo.read(".loopt/secrets.json");
    assert_eq!(
        json,
        concat!(
            "{\n",
            "  \"available_secrets\": [\n",
            "    \"ALPHA\",\n",
            "    \"ORG_ALL\",\n",
            "    \"ORG_SELECTED\",\n",
            "    \"ZETA\"\n",
            "  ],\n",
            "  \"environment_secrets\": {\n",
            "    \"dev\": [],\n",
            "    \"prod\": [\n",
            "      \"DEPLOY_KEY\"\n",
            "    ]\n",
            "  },\n",
            "  \"known_environments\": [\n",
            "    \"dev\",\n",
            "    \"prod\"\n",
            "  ]\n",
            "}\n",
        )
    );
    let context: LintContext = serde_json::from_str(&json).expect("inventory deserializes");
    assert_eq!(
        context
            .available_secrets
            .expect("base inventory known")
            .into_iter()
            .collect::<Vec<_>>(),
        vec!["ALPHA", "ORG_ALL", "ORG_SELECTED", "ZETA"]
    );
    assert!(context.environment_secrets["prod"].contains("DEPLOY_KEY"));
    assert_eq!(
        context
            .known_environments
            .expect("environment registry known")
            .len(),
        2
    );

    let typescript = repo.read(".loopt/secrets.ts");
    assert!(typescript.contains("export type SecretName"));
    assert!(typescript.contains("githubSecrets.ALPHA"));
    assert!(typescript.contains("githubSecrets.DEPLOY_KEY"));
    assert!(!typescript.contains("TOP_SECRET"));
    assert!(!typescript.contains("ENV_SECRET_VALUE"));
    assert!(!json.contains("TOP_SECRET"));
    assert!(!json.contains("ENV_SECRET_VALUE"));
}

#[test]
fn environment_option_scopes_secret_requests_without_losing_known_environment_names() {
    let repo = TempRepo::new("environment-scope");
    let script = r#"
endpoint=""
for arg in "$@"; do
  case "$arg" in repos/*|orgs/*) endpoint="$arg" ;; esac
done
case "$endpoint" in
  repos/acme/widget)
    printf '%s\n' '{"id":7,"full_name":"acme/widget","private":true,"owner":{"login":"acme","type":"User"}}'
    ;;
  repos/acme/widget/actions/secrets)
    printf '%s\n' '{"secrets":[]}'
    ;;
  orgs/acme/actions/secrets)
    printf '%s\n' '{"secrets":[]}'
    ;;
  repos/acme/widget/environments)
    printf '%s\n' '{"environments":[{"name":"dev"},{"name":"prod"}]}'
    ;;
  repos/acme/widget/environments/prod/secrets)
    printf '%s\n' '{"secrets":[{"name":"DEPLOY"}]}'
    ;;
  repos/acme/widget/environments/dev/secrets)
    exit 92
    ;;
  *) exit 93 ;;
esac
"#;

    with_fake_gh(&repo, script, || {
        secrets::run(
            repo.path(),
            &args(&["sync", "--repo", "acme/widget", "--environment", "prod"]),
        )
        .expect("scoped sync succeeds");
    });

    let inventory: Value =
        serde_json::from_str(&repo.read(".loopt/secrets.json")).expect("valid JSON");
    assert_eq!(
        inventory["environment_secrets"].get("prod").unwrap()[0],
        "DEPLOY"
    );
    assert!(inventory["environment_secrets"].get("dev").is_none());
    assert_eq!(inventory["known_environments"].as_array().unwrap().len(), 2);
}

#[test]
fn unauthorized_scope_fails_and_preserves_existing_outputs() {
    let repo = TempRepo::new("preserve");
    let old_json = "{\"available_secrets\":[\"OLD\"]}\n";
    let old_typescript = "// user-maintained fixture\n";
    repo.write(".loopt/secrets.json", old_json);
    repo.write(".loopt/secrets.ts", old_typescript);
    let script = r#"
endpoint=""
for arg in "$@"; do
  case "$arg" in repos/*|orgs/*) endpoint="$arg" ;; esac
done
case "$endpoint" in
  repos/acme/widget)
    printf '%s\n' '{"id":7,"full_name":"acme/widget","private":false,"owner":{"login":"acme","type":"Organization"}}'
    ;;
  repos/acme/widget/actions/secrets)
    printf '%s\n' '{"secrets":[{"name":"NEW"}]}'
    ;;
  orgs/acme/actions/secrets)
    exit 77
    ;;
  *) exit 78 ;;
esac
"#;

    with_fake_gh(&repo, script, || {
        let result = secrets::run(repo.path(), &args(&["sync", "--repo", "acme/widget"]));
        assert!(result.is_err());
    });
    assert_eq!(repo.read(".loopt/secrets.json"), old_json);
    assert_eq!(repo.read(".loopt/secrets.ts"), old_typescript);
}

#[test]
fn unsafe_environment_input_is_rejected_before_any_write() {
    let repo = TempRepo::new("unsafe");
    let result = secrets::run(
        repo.path(),
        &args(&["sync", "--repo", "acme/widget", "--environment=../prod"]),
    );
    assert!(result.is_err());
    assert!(!repo.path().join(".loopt/secrets.json").exists());
    assert!(!repo.path().join(".loopt/secrets.ts").exists());
}
