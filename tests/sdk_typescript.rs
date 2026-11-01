//! Integration test for the hand-written TypeScript authoring kit under
//! `sdk/typescript/`.
//!
//! The kit is ordinary TypeScript that Node executes directly through its
//! built-in type stripping, so the behavioural suite is just
//! `node --test "sdk/typescript/test/*.test.ts"` run from the repository
//! root. When `tsc` is on `PATH` the kit sources are additionally
//! type-checked with `tsc -p sdk/typescript --noEmit`; without a TypeScript
//! compiler the check is skipped unless `LOOPT_REQUIRE_TSC=1` is set, which
//! turns a missing compiler into a failure.

use std::path::Path;
use std::process::{Command, Output};

/// Repository root: integration tests for the root package are compiled with
/// the manifest directory of that package, which owns `sdk/typescript/`.
const REPO_ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// Run `program` with `args` from the repository root and capture its output.
fn run_in_repo(program: &str, args: &[&str]) -> std::io::Result<Output> {
    Command::new(program)
        .args(args)
        .current_dir(Path::new(REPO_ROOT))
        .output()
}

/// Human-readable captured output for assertion messages.
fn describe(output: &Output) -> String {
    format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}

/// Whether `tsc` can be spawned at all (a successful version probe is not
/// required here: a present-but-broken compiler fails the real check below
/// with its own output).
fn tsc_is_available() -> bool {
    match Command::new("tsc").arg("--version").output() {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => panic!("failed to probe for `tsc`: {error}"),
    }
}

#[test]
fn typescript_kit_test_suite_passes() {
    let output =
        run_in_repo("node", &["--test", "sdk/typescript/test/*.test.ts"]).unwrap_or_else(|error| {
            panic!(
                "failed to spawn `node`; Node.js 26 or newer is required to run the \
                 TypeScript kit suite: {error}"
            )
        });

    assert!(
        output.status.success(),
        "`node --test \"sdk/typescript/test/*.test.ts\"` failed ({}).\n{}",
        output.status,
        describe(&output),
    );
}

#[test]
fn typescript_kit_type_checks_when_tsc_is_available() {
    if !tsc_is_available() {
        let require_tsc = std::env::var("LOOPT_REQUIRE_TSC")
            .map(|value| value == "1")
            .unwrap_or(false);
        assert!(
            !require_tsc,
            "LOOPT_REQUIRE_TSC=1 is set but `tsc` was not found on PATH"
        );
        eprintln!("skipping TypeScript type check: `tsc` is not on PATH");
        return;
    }

    let output = run_in_repo("tsc", &["-p", "sdk/typescript", "--noEmit"])
        .expect("`tsc` was found but could not be spawned");

    assert!(
        output.status.success(),
        "`tsc -p sdk/typescript --noEmit` failed ({}).\n{}",
        output.status,
        describe(&output),
    );
}
