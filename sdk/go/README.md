# loopt Go SDK

`github.com/preloopdev/loopt/sdk/go` is the Go authoring SDK for
[loopt](https://github.com/preloopdev/loopt). It lets a normal Go program
construct a GitHub Actions workflow document and print it as the single JSON
object `loopt gen` consumes.

The module has four focused packages, in increasing order of abstraction:

| Package | Purpose |
| --- | --- |
| `om` | An insertion-ordered JSON object (`om.New`/`om.F`, with `Set`). Go's `map[string]any` marshals keys alphabetically, which would reorder workflow keys and matrix axes. |
| `expr` | `${{ ... }}` expression builders: literals, operators, built-in functions, and navigable contexts (`Github()`, `Secrets()`, `Env()`, ...). |
| `core` | Step, job, and workflow constructors, plus typed `StepRef`/`JobRef` references to `steps.<id>.outputs.<name>` and `needs.<id>.outputs.<name>`. |
| `emit` | Marshals the assembled document to stdout as exactly one JSON line. |

## Example

```go
package main

import (
	"fmt"
	"os"

	"github.com/preloopdev/loopt/sdk/go/core"
	"github.com/preloopdev/loopt/sdk/go/emit"
	"github.com/preloopdev/loopt/sdk/go/expr"
	"github.com/preloopdev/loopt/sdk/go/om"
)

func main() {
	condition, err := expr.Eq(expr.Github().Field("refName"), "main")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}

	test := core.Run("go test ./...").
		Name("Test").
		If(condition).
		ID("test").
		Build()

	job := core.NewJob().
		Name("Build").
		RunsOn("ubuntu-latest").
		Env("CGO_ENABLED", "0").
		Step(test).
		Build()

	workflow := core.NewWorkflow().
		Name("CI").
		On(om.New(om.F("push", om.New())))
	workflow.AddJob("build", job)

	if err := emit.Print(workflow.Build()); err != nil {
		fmt.Fprintln(os.Stderr, "loopt:", err)
		os.Exit(1)
	}
}
```

Expressions render through `fmt.Stringer` in their wrapped form
(`${{ ... }}`); use `Expr.Bare()` when the bare source text is needed.
`core` builders accept `expr.Expr`/`expr.Context` anywhere a workflow value is
expected and store the wrapped string form.

## Vendoring

`loopt import --language go` and `loopt init --language go` write the SDK
source into the user's repository at `.loopt/sdk/go` and generate a project
whose `go.mod` points at it with a relative `replace` directive:

```
require github.com/preloopdev/loopt/sdk/go v0.0.0

replace github.com/preloopdev/loopt/sdk/go => ../../sdk/go
```

There is no publish step and no network access is needed at build time. The
vendored copy always matches the `loopt` binary that wrote it, and it is safe
to commit because generated projects import it with a plain relative path.

## Development

```
go test ./...
go vet ./...
```
