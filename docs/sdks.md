# Authoring SDKs

All four first-party languages ship a real SDK — typed `${{ }}` expression
builders, context navigation, workflow/job/step constructors, and
`JobRef`/`StepRef` typed references to `needs.<id>.outputs.<name>` and
`steps.<id>.outputs.<name>`.

| SDK | Registry | Source |
|---|---|---|
| `@preloopdev/sdk` (TypeScript) | npm | `sdk/typescript` |
| `loopt-sdk` (Rust) | crates.io | `sdk/rust` |
| `loopt-sdk` (Python) | PyPI (not yet published) | `sdk/python` |
| `github.com/preloopdev/loopt/sdk/go` | Go modules via Git tags | `sdk/go` |

Every SDK is also **vendored into generated projects** — the `loopt` binary
embeds the sources and writes them under `.loopt/sdk/<lang>`, so generated
projects resolve the SDK by relative path, build offline, and always match
the binary that produced them. Registry installs are an opt-in alternative,
never a requirement.

**"What do I call?"** — [docs/sdk-api.md](sdk-api.md) maps every concept
(step/job/workflow, contexts, operators, refs) to the exact function in
each language.

`scripts/publish-sdks.sh` publishes the registry SDKs (npm + crates.io);
`scripts/publish-crates.sh` publishes `loopt-sdk` together with the shared
`preloop-gha-*` crates.

## TypeScript — `@preloopdev/sdk`

The reference SDK: `${{ }}` expressions (literals, operators, built-in
functions, dynamically navigable contexts like `github`/`env`/`secrets`),
workflow/job/step constructors, `JobRef`/`StepRef` typed references to
`needs.<id>.outputs.<name>` and `steps.<id>.outputs.<name>`, schema-generated
workflow/action types, and safe tagged `run` templates.

**No npm install needed**: `loopt actions add` and `loopt secrets sync`
vendor the SDK source into `.loopt/sdk/typescript` (embedded in the `loopt`
binary, always matching its version) and generate imports pointing at that
relative path. The vendored copy runs directly under Node 26+.

When `@preloopdev/sdk` is published on npm you can opt into it instead:

```sh
npm install @preloopdev/sdk        # requires Node.js 26+
loopt actions add actions/checkout@v6 --sdk-import @preloopdev/sdk
```

The package ships raw `.ts` sources — Node 26+ type-stripping runs them
directly.

## Rust — `loopt-sdk`

`loopt-sdk` mirrors `@preloopdev/sdk`: typed `${{ }}` expressions plus
`run`/`uses`/`job`/`workflow` constructors that assemble a workflow as an
ordered `serde_json::Value`, with `JobRef`/`StepRef` typed references.

```toml
loopt-sdk = "0.1"
```

`loopt import --language rust` also vendors the crate verbatim into every
generated project's `.loopt/sdk/rust`, so `<project>/Cargo.toml` can depend
on it with a plain relative path (`../../sdk/rust`) instead of the registry —
generated projects build offline and use the exact source the `loopt` binary
was built from.

## Python — `loopt_sdk`

`sdk/python` is the `loopt_sdk` package (dist `loopt-sdk`, Python 3.10+,
stdlib-only): `loopt_sdk.expr` (literal/operand, operators, functions, and
attribute/item context navigation like `github.event.pull_request.title` and
`secrets.GITHUB_TOKEN`), `loopt_sdk.core` (`run`/`uses`/`job`/
`call_workflow`/`workflow`, `JobRef`/`StepRef`), and `loopt_sdk.emit`.

Generated `<stem>.py` projects vendored under `.loopt/sdk/python` and import
it through a small `sys.path` shim computed from `__file__` — `loopt gen`
needs nothing but `python3`.

## Go — `github.com/preloopdev/loopt/sdk/go`

`sdk/go` is a Go module (Go 1.21+) with four packages: `expr` (`Expr`,
`Context`, operators/functions, navigable contexts), `om` (insertion-ordered
JSON object — Go's `encoding/json` would sort map keys and reorder workflow
fields), `core` (`Run`/`Uses`, `NewJob`/`NewCallJob`/`NewWorkflow`,
`JobRef`/`StepRef`), and `emit` (ordered one-line JSON to stdout).

`loopt import --language go` generates a `<stem>/` project (`main.go` +
`go.mod`) whose `go.mod` requires the SDK with a relative `replace`
directive pointing at the vendored `.loopt/sdk/go` — no proxy or network.

## Plugins

Out-of-tree plugin languages emit source that prints the workflow JSON
directly; they do not use an SDK.
