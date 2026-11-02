# Action bindings

Typed bindings turn any GitHub Action into a commit-pinned import with
declared inputs and outputs:

```sh
loopt actions add actions/checkout@v6
```

This resolves the ref to a commit SHA through `gh api`, reads the action's
`action.yml`, and writes `.loopt/actions/checkout.ts` plus an entry in
`.loopt/actions.lock`. The generated module imports `@preloopdev/sdk` (override
for local checkouts with `--sdk-import`); pinned inputs get typed names and
declared outputs become `StepRef` outputs, so `checkout(...).outputs.headSha`
type-checks.

```sh
loopt actions list [--json]
loopt actions update actions/checkout@main     # re-resolve and re-pin
loopt actions update checkout --output new-out # patch metadata (also --input-type, --patch[-file])
loopt actions verify                           # offline: lock + generated files intact
loopt actions remove checkout
```

`update` and `remove` refuse to touch a generated file that was edited
outside Loopt unless `--force` is passed.

For the workflow-level equivalent — pinning every `uses:` in
`.github/workflows/` — see [the workflow lockfile](workflow-lockfile.md).
