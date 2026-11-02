# Authoring workflows

Everything under `.loopt/workflows/` is editable source; everything under
`.github/workflows/` is generated output. Edit source, never the YAML.

```sh
# Import existing workflows into .loopt/workflows
loopt import --language typescript

# Generate .github/workflows from the source
loopt gen

# Fail if the committed YAML no longer matches the committed source
loopt gen --check
```

`loopt import` leaves the original YAML untouched. `loopt gen` refuses to
overwrite a workflow edited outside Loopt unless you pass `--force`.

You can also write source from scratch under `.loopt/workflows/`; `loopt gen`
discovers it and writes the matching workflow.

## Composite actions

`loopt import` also converts composite actions. Any file named `action.yml`
or `action.yaml` outside `.github/workflows/` is imported as action metadata
(source under `.loopt/workflows/actions/…`); `loopt gen` writes it back
byte-for-byte until you edit the source. A file named `action.yml` *inside*
`.github/workflows/` stays a workflow, matching GitHub's own rule.

## Safe `run` templates (TypeScript)

Tagged `run` templates keep untrusted values out of shell source by moving
each interpolation into a step environment variable and referencing it with a
quoted `"${__LOOPT_RUN_N}"`:

```ts
run`echo "PR title: ${github.event.pullRequest.title}"`
```

Strings, numbers, booleans and expressions are accepted; ambiguous quoting,
command substitution and unsupported shells are rejected up front.
`run.unsafe` interpolates source directly when you really mean it.

## Keeping YAML and source in sync

Generated YAML is output, not a second source of truth. `loopt gen --check`
writes nothing and exits 1 if any workflow is out of date, so run it in CI:

```yaml
- run: loopt gen --check
```

Each output is listed as `unchanged`, `stale` (the source changed), `missing`
(never generated), or `edited` (the YAML was changed by hand). For `edited`
you choose a direction: `loopt gen --force` regenerates the YAML from source
and discards the hand edit, while `loopt import` turns the edited YAML back
into source (it refuses to overwrite source you changed unless you pass
`--force`).
