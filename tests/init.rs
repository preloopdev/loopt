//! `loopt init`: a new project goes from nothing to a generated workflow
//! that has no drift and no lint findings, and init never clobbers anything.

mod support;

use support::{parse_document, TestRepo};

#[test]
fn init_creates_source_manifest_and_yaml_with_no_drift_and_no_findings() {
    let repo = TestRepo::new("init-basic");
    repo.run(&["init", "--language", "python"]).assert_success();

    assert!(repo.exists(".loopt/workflows/ci.py"));
    assert!(repo.exists(".loopt/manifest.json"));
    let doc = parse_document(&repo.path(".github/workflows/ci.yml")).expect("starter YAML parses");
    assert_eq!(doc.value()["name"], "ci");
    assert!(
        doc.value()["jobs"]["test"]["steps"]
            .as_array()
            .is_some_and(|steps| steps.len() >= 2),
        "the starter needs a checkout plus at least one real step"
    );

    // A fresh project has nothing to regenerate...
    repo.run(&["gen", "--check"]).assert_success();

    // ...and the starter handed to every new user is clean under our own lint.
    let lint = repo.run(&["lint", "--json"]);
    lint.assert_success();
    let reports: serde_json::Value =
        serde_json::from_str(&lint.stdout).expect("`lint --json` prints JSON");
    let findings = reports[0]["findings"].as_array().expect("findings array");
    assert!(findings.is_empty(), "starter must lint clean: {findings:?}");
}

#[test]
fn init_picks_the_starter_from_project_files_and_explicit_template_wins() {
    let repo = TestRepo::new("init-node");
    repo.write("package.json", "{}\n");
    repo.write("package-lock.json", "{}\n");
    repo.run(&["init", "--language", "python"]).assert_success();
    let yaml = repo.read(".github/workflows/ci.yml");
    assert!(
        yaml.contains("npm ci") && yaml.contains("npm test"),
        "{yaml}"
    );

    // `npm ci` fails without a lockfile, so the starter must not emit it then.
    let repo = TestRepo::new("init-node-nolock");
    repo.write("package.json", "{}\n");
    repo.run(&["init", "--language", "python"]).assert_success();
    let yaml = repo.read(".github/workflows/ci.yml");
    assert!(
        yaml.contains("npm install") && !yaml.contains("npm ci"),
        "{yaml}"
    );

    // An explicit --template beats what detection would pick.
    let repo = TestRepo::new("init-override");
    repo.write("package.json", "{}\n");
    repo.run(&["init", "--language", "python", "--template", "go"])
        .assert_success();
    let yaml = repo.read(".github/workflows/ci.yml");
    assert!(yaml.contains("go test ./..."), "{yaml}");
    assert!(!yaml.contains("npm"), "{yaml}");
}

#[test]
fn init_refuses_to_overwrite_an_existing_workflow() {
    let repo = TestRepo::new("init-exists");
    let existing = "name: mine\non: push\njobs: {}\n";
    repo.write(".github/workflows/ci.yml", existing);

    repo.run(&["init", "--language", "python"]).assert_failure();

    assert_eq!(
        repo.read(".github/workflows/ci.yml"),
        existing,
        "a refused init must leave the user's workflow untouched"
    );
    assert!(
        repo.list_files(".loopt").is_empty(),
        "a refused init must not leave source behind"
    );
}

#[test]
fn init_rejects_names_that_could_escape_the_workflows_directory() {
    let repo = TestRepo::new("init-unsafe");
    for name in ["../evil", "a/b", ".hidden", ""] {
        repo.run(&["init", "--language", "python", "--name", name])
            .assert_failure();
    }
    assert!(repo.list_files(".loopt").is_empty());
    assert!(repo.list_files(".github").is_empty());
}

#[test]
fn init_starter_survives_every_built_in_language() {
    // The starter's `${{ }}` expression has to come through each language's
    // string-literal rules intact, not just the one the other tests use.
    for (language, entrypoint) in [
        ("typescript", ".loopt/workflows/ci.ts"),
        ("go", ".loopt/workflows/ci.go"),
        ("rust", ".loopt/workflows/ci/main.rs"),
    ] {
        let repo = TestRepo::new(&format!("init-{language}"));
        repo.run(&["init", "--language", language]).assert_success();
        assert!(repo.exists(entrypoint), "{language}: expected {entrypoint}");

        let doc =
            parse_document(&repo.path(".github/workflows/ci.yml")).expect("starter YAML parses");
        assert_eq!(
            doc.value()["concurrency"]["group"],
            "ci-${{ github.ref }}",
            "{language}"
        );
        repo.run(&["gen", "--check"]).assert_success();
    }
}
