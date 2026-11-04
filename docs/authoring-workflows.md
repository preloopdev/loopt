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

### What survives regeneration

The authoring model is the workflow's data; formatting is not part of it. When
`.loopt/manifest.json` still holds the last accepted YAML for an output — the
normal case — `gen` writes that text back byte-identically, so comments,
anchors, quote style, and blank lines survive edits and even a deleted output
file.

Commit `.loopt/manifest.json` alongside the sources. Deleting it (or running
`gen` in a checkout that lacks it) forces a render from the model alone: every
value, key, expression, and ordering still round-trips, but comments are gone,
`&anchor`/`*alias` pairs are expanded inline, flow collections become block
style, and quotes are normalized. `loopt gen --check` in CI needs the manifest
for the same reason.

