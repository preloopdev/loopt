//! Subprocess tests for filesystem safety boundaries around import and gen.
//!
//! These fixtures contain only benign workflow YAML and run in isolated temp
//! repositories. Symlink cases are Unix-only because the test setup requires
//! the platform symlink primitive.

mod support;

use support::TestRepo;

const BENIGN_WORKFLOW: &str = "name: safe\non: push\njobs:\n  build:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo safe\n";

const BENIGN_PYTHON_SOURCE: &str = "import json\n\nprint(json.dumps({\"name\": \"safe\", \"on\": \"push\", \"jobs\": {\"build\": {\"runs-on\": \"ubuntu-latest\", \"steps\": []}}}))\n";

#[cfg(unix)]
#[test]
fn import_ignores_preexisting_temp_symlink_without_touching_valuable_file() {
    let repo = TestRepo::new("temp-symlink-import");
    let outside = TestRepo::new("temp-symlink-import-outside");
    let valuable_rel = "valuable.txt";
    let valuable_before = "do not overwrite this file\n";
    let valuable = outside.write(valuable_rel, valuable_before);

    repo.write(".github/workflows/ci.yml", BENIGN_WORKFLOW);
    repo.symlink(".loopt/workflows/ci.py.loopt-tmp", &valuable);

    let result = repo.run(&["import", ".github/workflows/ci.yml", "--language", "python"]);
    result.assert_success();
    assert_eq!(outside.read(valuable_rel), valuable_before);
    assert!(
        repo.path(".loopt/workflows/ci.py.loopt-tmp")
            .symlink_metadata()
            .is_ok(),
        "a safe import must not remove the preexisting temp symlink"
    );
    assert!(repo.path(".loopt/workflows/ci.py").is_file());
}

#[cfg(unix)]
#[test]
fn import_rejects_loopt_directory_symlink_escape() {
    let repo = TestRepo::new("loopt-symlink-escape");
    let outside = TestRepo::new("loopt-symlink-escape-outside");
    let valuable_before = "outside repository data\n";
    outside.write("valuable.txt", valuable_before);
    let outside_before = outside.list_files(".");

    repo.write(".github/workflows/ci.yml", BENIGN_WORKFLOW);
    repo.symlink(".loopt", outside.root());

    let result = repo.run(&["import", ".github/workflows/ci.yml", "--language", "python"]);
    result.assert_failure();
    assert_eq!(outside.list_files("."), outside_before);
    assert_eq!(outside.read("valuable.txt"), valuable_before);
}

#[cfg(unix)]
#[test]
fn gen_rejects_symlinked_workflow_destination_without_touching_target() {
    let repo = TestRepo::new("workflow-destination-symlink");
    let outside = TestRepo::new("workflow-destination-symlink-outside");
    let valuable_before = "valuable workflow destination\n";
    let valuable = outside.write("captured.yml", valuable_before);

    repo.write(".loopt/workflows/ci.py", BENIGN_PYTHON_SOURCE);
    repo.symlink(".github/workflows/ci.yml", &valuable);

    let result = repo.run(&["gen", ".loopt/workflows/ci.py"]);
    result.assert_failure();
    assert_eq!(outside.read("captured.yml"), valuable_before);
    assert!(
        repo.path(".github/workflows/ci.yml")
            .symlink_metadata()
            .is_ok(),
        "a rejected generation must not replace the symlinked destination"
    );
}

#[test]
fn import_directory_destination_fails_batch_before_any_write() {
    let repo = TestRepo::new("directory-batch-import");
    repo.write(".github/workflows/a.yml", BENIGN_WORKFLOW);
    repo.write(".github/workflows/b.yml", BENIGN_WORKFLOW);
    std::fs::create_dir_all(repo.path(".loopt/workflows/b.py"))
        .expect("create directory at the second generated source destination");

    let result = repo.run(&[
        "import",
        ".github/workflows/a.yml",
        ".github/workflows/b.yml",
        "--language",
        "python",
    ]);
    result.assert_failure();
    assert!(
        !repo.exists(".loopt/workflows/a.py"),
        "a later directory destination must not allow an earlier source write"
    );
    assert!(
        !repo.exists(".loopt/manifest.json"),
        "a failed import batch must not write a partial manifest"
    );
    assert!(repo.path(".loopt/workflows/b.py").is_dir());
}

#[test]
fn gen_directory_destination_fails_batch_before_any_write() {
    let repo = TestRepo::new("directory-batch-gen");
    repo.write(".loopt/workflows/a.py", BENIGN_PYTHON_SOURCE);
    repo.write(".loopt/workflows/b.py", BENIGN_PYTHON_SOURCE);
    std::fs::create_dir_all(repo.path(".github/workflows/b.yml"))
        .expect("create directory at the second generated workflow destination");

    let result = repo.run(&["gen", ".loopt/workflows/a.py", ".loopt/workflows/b.py"]);
    result.assert_failure();
    assert!(
        !repo.exists(".github/workflows/a.yml"),
        "a later directory destination must not allow an earlier workflow write"
    );
    assert!(
        !repo.exists(".loopt/manifest.json"),
        "a failed generation batch must not write a partial manifest"
    );
    assert!(repo.path(".github/workflows/b.yml").is_dir());
}

#[test]
fn import_safe_workflow_in_isolated_repository() {
    let repo = TestRepo::new("safe-import");
    let workflow_rel = ".github/workflows/safe.yml";
    repo.write(workflow_rel, BENIGN_WORKFLOW);

    let result = repo.run(&["import", workflow_rel, "--language", "python"]);
    result.assert_success();
    assert!(repo.path(".loopt/workflows/safe.py").is_file());
    assert!(repo.path(".loopt/manifest.json").is_file());
    assert_eq!(repo.read(workflow_rel), BENIGN_WORKFLOW);
}
