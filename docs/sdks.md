# Authoring SDKs

Two languages ship a real SDK — typed expression builders, context
navigation, and workflow/job/step constructors — rather than just literal
emitters.

| SDK | Registry | Version |
|---|---|---|
| `@preloopdev/sdk` (TypeScript) | npm | `sdk/typescript` in this repo |
| `loopt-sdk` (Rust) | crates.io | `sdk/rust` in this repo |

`scripts/publish-sdks.sh` publishes both; `scripts/publish-crates.sh`
publishes `loopt-sdk` together with the shared `preloop-gha-*` crates.

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

## Python, Go, and plugins

Python and Go emit standalone literal programs (no runtime dependency), so
they do not need an SDK package. Out-of-tree plugin languages likewise emit
source that prints the workflow JSON directly.
