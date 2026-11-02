# SDK API reference

All four authoring SDKs expose the same mental model. You build a workflow
document from three layers, then print it:

1. **`expr`** — `${{ }}` expressions: literals, operators, functions, and
   navigable contexts (`github`, `secrets`, `env`, …). Expressions render as
   `${{ ... }}` strings and can be dropped anywhere a value is expected.
2. **`core`** — step / job / workflow constructors, plus `JobRef`/`StepRef`
   for typed references to `needs.<id>.outputs.<name>` and
   `steps.<id>.outputs.<name>`.
3. **`emit`** — print the assembled document as the single JSON line
   `loopt gen` reads back.

Per-SDK examples: [TypeScript](../sdk/typescript/README.md) ·
[Go](../sdk/go/README.md) · [Python](../sdk/python/README.md) ·
[Rust](../sdk/rust) (see `.loopt/workflows/*/main.rs` output for a working
example).

## Concept → call

| Concept | TypeScript | Go | Python | Rust |
|---|---|---|---|---|
| Import | `import { … } from "@preloopdev/sdk"` (or vendored `./sdk/typescript/index.ts`) | `import "github.com/preloopdev/loopt/sdk/go/{core,expr,om,emit}"` | `from loopt_sdk import …` | `use loopt_sdk::{core, expr, emit};` |
| Shell step | `run("cargo test")` / tagged `` run`…` `` | `core.Run("cargo test")` | `run("cargo test")` | `core::run("cargo test")` |
| Action step | `uses("actions/checkout@v6", { with: {…} })` | `core.Uses("actions/checkout@<sha>").With(key, value)` | `uses("actions/checkout@v6").with_(key, value)` | `core::uses("actions/checkout@v6").with(key, value)` |
| Docker step | `docker("image:tag")` | — | — | — |
| Local action | `local("./path")` | — | — | — |
| Job | `job({ "runs-on": "ubuntu-latest", steps: […] })` | `core.NewJob().RunsOn("ubuntu-latest").Step(…).Build()` | `job().runs_on("ubuntu-latest").step(…).build()` | `core::job().runs_on("ubuntu-latest").step(…)` |
| Reusable-workflow job | `callWorkflow("o/r/.github/wf.yml@ref", def)` | `core.NewCallJob(uses)` | `call_workflow(uses)` | `core::call_workflow(uses)` |
| Workflow | `workflow("CI", { on: {…}, jobs: {…} })` | `core.NewWorkflow().Name("CI").On(om.New(…))` + `AddJob(id, job)` | `workflow(name="CI")` builder | `core::workflow().name("CI")` builder |
| Emit | `emit(workflow)` / `printWorkflow` | `emit.Print(wf)` | `print_workflow(wf)` | `emit::print(&wf)` |
| Ordered object (Go only) | — | `om.New(om.F("k", v), …)` | dicts are ordered | `serde_json` preserve_order |
| Expression contexts | `github.event.pull_request.title`, `secrets.TOKEN`, `env.X` | `expr.Github().Field("event").Field("pull_request")…`, `expr.Secrets().Field("TOKEN")` | `github.event.pull_request.title`, `secrets.TOKEN` | `expr::github().field("event")…`, `expr::secrets().field("TOKEN")` |
| Literal | `lit("x")`, `operand(v)` | `expr.Lit(v)`, `expr.Operand(v)` | `lit(v)`, `operand(v)` | `expr::lit(v)`, `expr::operand(v)` |
| Operators | `eq(a,b) ne lt lte gt gte and or not at star` | `expr.Eq Ne Lt Le Gt Ge And Or Not At Star` | `eq ne lt le gt ge and_ or_ not_ at star` | `expr::eq ne lt le gt ge and or not at star` |
| Functions | `contains startsWith endsWith format join toJSON fromJSON hashFiles caseOf` | `expr.Contains StartsWith EndsWith Format Join ToJSON FromJSON HashFiles Case` | `contains starts_with ends_with format join to_json from_json hash_files case_of` | `expr::contains starts_with …` (snake_case) |
| Status | `success() failure() always() cancelled()` | `expr.Success() Failure() Always() Cancelled()` | `success() failure() always() cancelled()` | `expr::success() …` |
| Step outputs | `step.outputs.<name>` (`StepRef`) | `stepRef.Outputs.<name>` | `step.outputs.<name>` | `StepRef::output(name)` |
| Job outputs | `job.outputs.<name>` (`JobRef`) | `jobRef.Outputs.<name>` | `job.outputs.<name>` | `JobRef::output(name)` |
| Step/job `if:` | `if:` field / `.if(expr)` | `.If(expr)` | `.if_cond(expr)` (`if` is a keyword) | `.if_cond(expr)` |
| Raw `${{ }}` escape | `expr.raw("…")` / operand passthrough | `expr.Raw("…")` | raw operand passthrough | `expr::operand` passthrough |

## Common patterns

**Condition a step on the branch:**

```ts
run`./deploy.sh`.if(eq(github.refName, "main"))
```

**Depend on another job's output:**

```ts
const build = job({ "runs-on": "ubuntu-latest", outputs: { bin: "${{ steps.compile.outputs.path }}" }, steps: […] });
const ship  = job({ "runs-on": "ubuntu-latest", needs: build, steps: [ run`cp ${{ needs.build.outputs.bin }} .` ] });
```

`needs:` accepts the `JobRef`; `needs.build.outputs.bin` type-checks because
`build` declared `outputs.bin`.

**Matrix:**

```ts
job({ "runs-on": "${{ matrix.os }}", strategy: { matrix: { os: ["ubuntu-latest", "macos-latest"], node: [20, 22] } }, steps: […] })
```

**Safe `run` interpolation (TypeScript only):** see
[safe run templates](authoring-workflows.md#safe-run-templates-typescript).

## Generated bindings and secrets

`loopt actions add` writes typed modules under `.loopt/actions/` —
`import { checkout } from "../actions/checkout.ts"` gives you
`checkout({…}).outputs.headSha`. `loopt secrets sync` writes
`.loopt/secrets.ts` with typed `secrets` accessors. Both import the vendored
SDK by default; see [action bindings](action-bindings.md) and
[secrets](secrets.md).
