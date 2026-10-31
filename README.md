# Loopt

Loopt is an open-source CLI for authoring GitHub Actions workflows in
TypeScript, Python, Go, or Rust and generating standard workflow YAML.

Loopt is deliberately explicit: `loopt import` converts existing workflow YAML
into editable source, and `loopt gen` executes that source to write
`.github/workflows/*.yml`. There are no Git hooks, watchers, or runtime
dependencies on a CI service. The generated YAML is ordinary GitHub Actions
YAML.

## Install

Build from source with Rust 1.97 or newer:

```sh
cargo install --path .
```

The generated source needs the corresponding language toolchain at generation
time: Node.js 26+ (TypeScript type stripping), Python 3, Go, or Cargo/Rust.

## Import existing workflows

```sh
loopt import --language typescript
```

With no paths, Loopt imports the direct `.yml`/`.yaml` files in
`.github/workflows`. You can also name specific files:

```sh
loopt import .github/workflows/ci.yml --language python
```

Imported source is written under `.loopt/workflows/`. The original workflow
YAML is not modified. `.loopt/manifest.json` records the source/output mapping
so later generation is deterministic. Existing hand-edited source is not
overwritten unless you pass `--force`.

## Generate workflow YAML

```sh
loopt gen
```

`loopt gen` runs every recorded source (and newly added source under
`.loopt/workflows`) and writes the corresponding workflow YAML. It refuses to
overwrite a workflow that was edited outside Loopt unless you pass `--force`;
run `loopt import` to reconcile an external edit back into source instead.

A source can also be authored from scratch and generated without a prior
import; the default destination is
`.github/workflows/<source-stem>.yml`.

`loopt gen` also runs the native correctness and security review on the
generated workflows and prints findings to stderr. Use `--no-lint` to skip it.

## Review workflows

```sh
loopt lint
```

`loopt lint` checks the generated workflow outputs (or direct
`.github/workflows` files when nothing has been generated). It combines the
shared correctness lint with the native security audits. Warnings and
informational findings exit 0; an `error` finding exits 1.

Use `--json` for machine-readable output, `--persona pedantic|auditor` for
stricter policy checks, and `--context inventory.json` to supply known runner
labels and secret **names** (never values). Checks that need repository
evidence that is not supplied are reported as unavailable rather than treated
as safe.

## Supported languages

| Language | Source | Runtime used by `gen` |
|---|---|---|
| TypeScript | `.loopt/workflows/<stem>.ts` | `node` |
| Python | `.loopt/workflows/<stem>.py` | `python3` |
| Go | `.loopt/workflows/<stem>.go` | `go run` |
| Rust | `.loopt/workflows/<stem>/main.rs` | `cargo run` |

Generated programs construct real language values and print one JSON workflow
object. Matrix axes and all other mapping order are preserved. Rust projects
are standalone Cargo workspaces with build artifacts cached under
`.loopt/.cache`.

## Custom languages

A non-built-in language can be supplied with a process plugin:

```sh
loopt import --language mylang --plugin ./my-plugin --plugin-arg emit
```

The plugin speaks the versioned `loopt plugin` JSON protocol on stdin/stdout
(`describe`, `emit`, `synth`) and is executed directly, never through a shell.

## Testing

```sh
cargo test
```

The expensive corpus test runs the real import → generated program → YAML
pipeline over an external fixture tree:

```sh
PRELOOP_FIXTURES=/path/to/fixtures cargo test --test corpus -- --ignored
```

## License

MIT; see [LICENSE](LICENSE). The vendored workflow authoring module is
documented in `vendor/preloop-gha-parser/PROVENANCE.md`.
