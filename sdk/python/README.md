# loopt-sdk (Python)

Python authoring SDK for [loopt](https://github.com/preloopdev/loopt). It builds
GitHub Actions workflow documents as native Python values and prints the one
JSON line `loopt gen` reads back.

`loopt import --language python` vendors this package into your repository
under `.loopt/sdk/python`, so generated sources run offline with no install:

```python
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "../sdk/python"))

from loopt_sdk import eq, github, job, print_workflow, run, secrets, workflow
```

## Usage

```python
from loopt_sdk import (
    always,
    eq,
    github,
    job,
    print_workflow,
    run,
    uses,
    workflow,
)

build_step = run("cargo build").id("build").build()

build = (
    job()
    .name("build")
    .runs_on("ubuntu-latest")
    .step(uses("actions/checkout@v4").build())
    .step(build_step)
    .build()
)

ci = workflow().name("CI").on_("push")
build_ref = ci.job("build", build)

print_workflow(ci)
```

Expressions render with `${{ ... }}` when stringified:

```python
github.event.pull_request.title     # ${{ github.event.pull_request.title }}
secrets.GITHUB_TOKEN                # ${{ secrets.GITHUB_TOKEN }}
eq(github.ref, "refs/heads/main")   # ${{ github.ref == 'refs/heads/main' }}
```

`JobRef.output(...)` and `StepRef.output(...)` produce typed references such
as `${{ needs.build.outputs.artifact }}` and
`${{ steps.build.outputs.artifact }}`; regular jobs reject undeclared output
names.

## Package layout

- `loopt_sdk.expr` — literals, operators, built-in functions, and contexts.
- `loopt_sdk.core` — `run`/`uses`, `job`, `workflow`, `JobRef`/`StepRef`.
- `loopt_sdk.emit` — `print_workflow`, one ordered JSON line on stdout.

## Development

Tests use only the standard library:

```sh
cd sdk/python
python3 -m unittest discover
```

Requires Python 3.10 or newer.
