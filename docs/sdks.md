# Authoring SDKs

Two languages ship a real SDK — typed expression builders, context
navigation, and workflow/job/step constructors — rather than just literal
emitters.

| SDK | Registry | Version |
|---|---|---|
| `@loopt/sdk` (TypeScript) | npm | `sdk/typescript` in this repo |
| `loopt-sdk` (Rust) | crates.io | `sdk/rust` in this repo |

`scripts/publish-sdks.sh` publishes both; `scripts/publish-crates.sh`
publishes `loopt-sdk` together with the shared `preloop-gha-*` crates.

## TypeScript — `@loopt/sdk`

The reference SDK: `${{ }}` expressions (literals, operators, built-in
functions, dynamically navigable contexts like `github`/`env`/`secrets`),
workflow/job/step constructors, `JobRef`/`StepRef` typed references to
`needs.<id>.outputs.<name>` and `steps.<id>.outputs.<name>`, schema-generated
workflow/action types, and safe tagged `run` templates.

```sh
npm install @loopt/sdk        # requires Node.js 26+
```

The package ships raw `.ts` sources — Node 26+ type-stripping runs them
directly. Generated action bindings (`loopt actions add`) import it as
`@loopt/sdk` by default; `--sdk-import` overrides the specifier for local
checkouts (`./sdk.ts`, a vendored copy, a pnpm workspace path).

## Rust — `loopt-sdk`

`loopt-sdk` mirrors `@loopt/sdk`: typed `${{ }}` expressions plus
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
