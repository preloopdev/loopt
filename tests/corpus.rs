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

use std::path::PathBuf;

use support::{
    assert_order_preserving_eq, discover_fixture_yaml_files, fixture_label, fixtures_root,
    generated_workflow_files, parse_document, TestRepo,
};

struct RoundTrip {
    label: String,
    workflow_rel: String,
    oracle: preloop_gha_authoring::WorkflowDocument,
}

/// One repository per language, so the generated Rust projects share a
/// single cargo target directory instead of cold-building serde_json once
/// per fixture.
fn run_corpus_for_language(language: &str) {
    let fixtures = discover_fixture_yaml_files();
    assert!(
        !fixtures.is_empty(),
        "no fixture YAML files discovered under PRELOOP_FIXTURES"
    );
    let fixtures_root: PathBuf = fixtures_root();
    let repo = TestRepo::new(&format!("corpus-{language}"));

    let mut round_trips: Vec<RoundTrip> = Vec::new();
    let mut failures = Vec::new();
    let mut cleanly_rejected = 0usize;

    // Unrepresentable fixtures are exercised in their own repository so a
    // single expected import failure cannot abort the batched import of
    // every representable workflow.
    for (index, fixture) in fixtures.iter().enumerate() {
        let label = fixture_label(&fixtures_root, fixture);
        let original_text = match std::fs::read_to_string(fixture) {
            Ok(text) => text,
            Err(error) => {
                failures.push(format!("{label}: could not read fixture: {error}"));
                continue;
            }
        };
        let workflow_rel = format!(
            ".github/workflows/{index:03}-{}.yml",
            sanitize_label(&label)
        );
        match parse_document(fixture) {
            Ok(oracle) => {
                repo.write(&workflow_rel, &original_text);
                round_trips.push(RoundTrip {
                    label,
                    workflow_rel,
                    oracle,
                });
            }
            Err(_) => {
                let rejected_repo = TestRepo::new(&format!("reject-{language}-{index}"));
                rejected_repo.write(&workflow_rel, &original_text);
                let import = rejected_repo.run(&["import", &workflow_rel, "--language", language]);
                if import.success {
                    failures.push(format!(
                        "{label}: shared parser rejects this fixture but `loopt import` succeeded \
                         (stdout: {:?})",
                        import.stdout
                    ));
                    continue;
                }
                let generated = generated_workflow_files(&rejected_repo);
                if !generated.is_empty() {
                    failures.push(format!(
                        "{label}: import of an unrepresentable fixture wrote {generated:?}"
                    ));
                    continue;
                }
                cleanly_rejected += 1;
            }
        }
    }

    if !round_trips.is_empty() {
        let mut args: Vec<String> = vec!["import".to_owned()];
        args.extend(round_trips.iter().map(|item| item.workflow_rel.clone()));
        args.push("--language".to_owned());
        args.push(language.to_owned());
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let import = repo.run(&args);
        if !import.success {
            panic!(
                "[{language}] batched import failed: exit {:?}\nstdout: {}\nstderr: {}",
                import.status_code, import.stdout, import.stderr
            );
        }

        let generated = generated_workflow_files(&repo);
        if generated.is_empty() {
            panic!("[{language}] import succeeded but wrote no source under .loopt/workflows");
        }

        let gen = repo.run(&["gen"]);
        if !gen.success {
            panic!(
                "[{language}] `loopt gen` failed: exit {:?}\nstdout: {}\nstderr: {}",
                gen.status_code, gen.stdout, gen.stderr
            );
        }

        for item in &round_trips {
            let regenerated_path = repo.path(&item.workflow_rel);
            let regenerated = match parse_document(&regenerated_path) {
                Ok(document) => document,
                Err(error) => {
                    failures.push(format!(
                        "{}: regenerated YAML does not parse: {error}",
                        item.label
                    ));
                    continue;
                }
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                assert_order_preserving_eq(item.oracle.value(), regenerated.value());
            }));
            if let Err(payload) = result {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "round-trip comparison panicked".to_string());
                failures.push(format!("{}: {message}", item.label));
            }
        }
    }

    eprintln!(
        "[{language}] corpus: {} fixtures, {} round-tripped, {cleanly_rejected} cleanly \
         rejected as unrepresentable, {} failed",
        fixtures.len(),
        round_trips.len().saturating_sub(failures.len()),
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

fn sanitize_label(label: &str) -> String {
    label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect()
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
