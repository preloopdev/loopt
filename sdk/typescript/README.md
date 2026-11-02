# @preloopdev/sdk

TypeScript authoring SDK for [loopt](https://github.com/preloopdev/loopt).
Builds GitHub Actions workflow documents as real TypeScript values and
prints the single JSON line `loopt gen` reads back.

The reference SDK — the other languages (`sdk/rust`, `sdk/go`,
`sdk/python`) mirror its API. See [docs/sdk-api.md](../../docs/sdk-api.md)
for the cross-language call table.

Requires **Node.js 26+** (native type stripping; the package ships raw
`.ts` sources).

## Layers

| Layer | Contents |
| --- | --- |
| `expr` | `${{ }}` expressions: `lit`/`operand`, operators (`eq`, `and`, `or`, `at`, `star`, …), functions (`contains`, `startsWith`, `toJSON`, `hashFiles`, `caseOf`, `success`, …), and navigable contexts (`github`, `env`, `secrets`, `vars`, `inputs`, `needs`, `steps`, `runner`, `job`, `strategy`). |
| `core` | `run` (plain + safe tagged template), `uses`, `docker`, `local`, `wait`/`waitAll`/`cancel`/`parallel` step refs, `job`, `callWorkflow`, `workflow`, `JobRef`/`StepRef`. |
| `generated` | Schema-generated `WorkflowDef`/`ActionDef` types and context aliases. |
| `kit` | `emit` (print the workflow as one JSON line), `LooptError`, JSON/expr plumbing. |

## Example

```ts
import {
  eq, github, job, run, secrets, uses, workflow, emit,
} from "@preloopdev/sdk";

const checkout = uses("actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1", {
  with: { "persist-credentials": false },
});

const test = job({
  "runs-on": "ubuntu-latest",
  permissions: { contents: "read" },
  steps: [
    checkout,
    run`npm ci`,
    run`npm test`.if(eq(github.refName, "main")),
  ],
});

const ci = workflow("CI", {
  on: { push: { branches: ["main"] }, pull_request: {} },
  jobs: { test },
  permissions: {},
});

emit(ci);
```

Tagged `run` templates keep untrusted values out of shell source: every
`${…}` interpolation becomes a step `env` entry referenced as
`"${__LOOPT_RUN_N}"`. `run.unsafe` interpolates raw source when you mean it.

## Vendoring

You normally never install this package: `loopt actions add` and
`loopt secrets sync` vendor the SDK source into your repository at
`.loopt/sdk/typescript` and generated helpers import it by relative path.
The vendored copy always matches the `loopt` binary that wrote it.

Once published to npm you can opt into the registry copy:

```sh
npm install @preloopdev/sdk
loopt actions add actions/checkout@v6 --sdk-import @preloopdev/sdk
```

## Development

```sh
npm test          # node --test
npm run typecheck # tsc --noEmit
```
