//! Full-corpus conformance: for every real Preloop fixture YAML file and
//! every Loopt language integration, run the actual pipeline --
//! `loopt import --language <lang>` to produce a native source file, then
//! `loopt gen` to execute that generated program (via the real language
//! runtime: node, python3, `go run`, or `cargo run`) and turn its JSON
//! output back into workflow YAML -- and require the regenerated document
//! to be semantically identical, in the same nested key order, to the
//! original.
//!
//! These tests never execute anything *from* a fixture's own YAML (its
//! `run:` steps, actions, etc.) -- only the Loopt-generated authoring
//! program is ever executed, and that program only constructs and prints a
//! JSON value. They're expensive (hundreds of fixtures, several involving a
//! cold `cargo run` build) so they're `#[ignore]`d; run explicitly, e.g.:
//!
//!   PRELOOP_FIXTURES=/path/to/fixtures cargo test --test corpus -- --ignored --test-threads=4
//!
//! Classification has no skip list: every fixture is parsed once with the
//! same shared parser used for comparison. A fixture that fails to parse
//! (syntax-invalid or otherwise unrepresentable -- e.g. an action document
//! or a semantically-negative workflow that still happens to be valid,
//! representable YAML) is still exercised: the assertion simply flips to
//! "import fails cleanly and writes nothing", instead of being silently
//! excluded from the run. Action documents and semantic-negative workflows
//! that *do* parse (the shared parser performs no mandatory semantic
//! validation, per contract) flow through the same round-trip assertion as
//! any other fixture -- they are not treated as a separate category.

mod support;

use std::path::{Path, PathBuf};

use support::{
    assert_order_preserving_eq, discover_fixture_yaml_files, fixture_label, fixtures_root,
    generated_workflow_files, parse_document, TestRepo,
};

enum Outcome {
    RoundTripped,
    CleanlyRejected,
    Failure(String),
}

fn check_one(language: &str, fixture: &Path, label: &str) -> Outcome {
    let original_text = match std::fs::read_to_string(fixture) {
        Ok(t) => t,
        Err(e) => return Outcome::Failure(format!("could not read fixture: {e}")),
    };

    // The same shared parser the CLI uses is the oracle for whether this
    // document is representable at all -- not a hand-maintained skip list.
    let oracle = parse_document(fixture);

    let repo = TestRepo::new(&format!("{language}-{label}"));
    let file_name = fixture
        .file_name()
        .expect("fixture path has a file name")
        .to_string_lossy()
        .into_owned();
    let workflow_rel = format!(".github/workflows/{file_name}");
    repo.write(&workflow_rel, &original_text);

    let import = repo.run(&["import", &workflow_rel, "--language", language]);

    match oracle {
        Err(_parse_err) => {
            // Syntax-invalid or otherwise unrepresentable: import must fail
            // cleanly -- no panic, nonzero exit, nothing written -- never
            // silently accepted and never skipped.
            if import.success {
                return Outcome::Failure(format!(
                    "fixture does not parse with the shared parser, but `loopt import` \
                     succeeded anyway (stdout: {:?})",
                    import.stdout
                ));
            }
            let generated = generated_workflow_files(&repo);
            if !generated.is_empty() {
                return Outcome::Failure(format!(
                    "import of an unrepresentable fixture still wrote generated files: \
                     {generated:?}"
                ));
            }
            Outcome::CleanlyRejected
        }
        Ok(oracle_doc) => {
            if !import.success {
                return Outcome::Failure(format!(
                    "shared parser accepts this fixture but `loopt import` failed: \
                     exit {:?}\nstderr: {}",
                    import.status_code, import.stderr
                ));
            }

            let generated = generated_workflow_files(&repo);
            if generated.is_empty() {
                return Outcome::Failure(
                    "import succeeded but wrote no files under .loopt/workflows".to_string(),
                );
            }

            // Execute the generated authoring program (never the fixture's
            // own commands) via `loopt gen`, and turn its JSON output back
            // into YAML.
            let gen = repo.run(&["gen"]);
            if !gen.success {
                return Outcome::Failure(format!(
                    "`loopt gen` failed after importing: exit {:?}\nstderr: {}\n\
                     generated files: {generated:?}",
                    gen.status_code, gen.stderr
                ));
            }

            if !repo.exists(&workflow_rel) {
                return Outcome::Failure(format!("`loopt gen` did not (re)write {workflow_rel}"));
            }

            let regenerated_path = repo.path(&workflow_rel);
            let regenerated = match parse_document(&regenerated_path) {
                Ok(doc) => doc,
                Err(e) => {
                    return Outcome::Failure(format!(
                        "regenerated YAML does not parse with the shared parser: {e}"
                    ));
                }
            };

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_order_preserving_eq(oracle_doc.value(), regenerated.value());
            }));
            match result {
                Ok(()) => Outcome::RoundTripped,
                Err(payload) => {
                    let msg = payload
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                        .unwrap_or_else(|| "round-trip comparison panicked".to_string());
                    Outcome::Failure(msg)
                }
            }
        }
    }
}

fn run_corpus_for_language(language: &str) {
    let fixtures = discover_fixture_yaml_files();
    assert!(
        !fixtures.is_empty(),
        "no fixture YAML files discovered under PRELOOP_FIXTURES"
    );
    let fixtures_root: PathBuf = fixtures_root();

    let mut failures = Vec::new();
    let mut round_tripped = 0usize;
    let mut cleanly_rejected = 0usize;

    for fixture in &fixtures {
        let label = fixture_label(&fixtures_root, fixture);
        match check_one(language, fixture, &label) {
            Outcome::RoundTripped => round_tripped += 1,
            Outcome::CleanlyRejected => cleanly_rejected += 1,
            Outcome::Failure(msg) => failures.push(format!("{label}: {msg}")),
        }
    }

    eprintln!(
        "[{language}] corpus: {} fixtures, {round_tripped} round-tripped, \
         {cleanly_rejected} cleanly rejected as unrepresentable, {} failed",
        fixtures.len(),
        failures.len()
    );

    assert!(
        failures.is_empty(),
        "{} of {} fixtures failed for language `{language}`:\n{}",
        failures.len(),
        fixtures.len(),
        failures.join("\n\n")
    );
}

#[test]
#[ignore = "expensive: runs the full Preloop fixture corpus; set PRELOOP_FIXTURES and pass --ignored"]
fn corpus_round_trip_typescript() {
    run_corpus_for_language("typescript");
}

#[test]
#[ignore = "expensive: runs the full Preloop fixture corpus; set PRELOOP_FIXTURES and pass --ignored"]
fn corpus_round_trip_python() {
    run_corpus_for_language("python");
}

#[test]
#[ignore = "expensive: runs the full Preloop fixture corpus; set PRELOOP_FIXTURES and pass --ignored"]
fn corpus_round_trip_go() {
    run_corpus_for_language("go");
}

#[test]
#[ignore = "expensive: runs the full Preloop fixture corpus (cold `cargo run` build per \
            fixture, slow); set PRELOOP_FIXTURES and pass --ignored"]
fn corpus_round_trip_rust() {
    run_corpus_for_language("rust");
}
