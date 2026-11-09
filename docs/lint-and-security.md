# Lint and security review

```sh
loopt lint
```

Runs native correctness and security checks on generated workflows. Warnings
exit 0; errors exit 1.

| Flag | Purpose |
|---|---|
| `--json` | Machine-readable findings |
| `--persona regular|pedantic|auditor` | Reviewer strictness |
| `--context inventory.json` | Supply known runner labels and secret names (never values) |
| `--online` | Fetch GitHub repository evidence for the online audits |

`.loopt/secrets.json` (from `loopt secrets sync`) is loaded automatically as
the default context; `--context` overrides it.

`loopt gen` runs the same review automatically; pass `--no-lint` to skip it.

## Security audits

Alongside the authoring checks, lint runs zizmor-compatible security audits.
These include `dangerous-checkout` (an `actions/checkout` step that sets
`allow-unsafe-pr-checkout`, disabling the action's fork-code protection on
`pull_request_target` and `workflow_run`), `template-injection`,
`unpinned-uses`, `artipacked`, `excessive-permissions`, and the repository
audits described below.

## Online audits (`--online`)

Lint is offline by default. The zizmor-style repository audits need facts
that only GitHub knows: which commits belong to a repository, which tags and
branches exist, whether an action's repository is archived, and published
GitHub Security Advisories. Offline, those checks are reported as
"unavailable" rather than treated as clean.

`--online` gathers exactly those facts for every repository referenced by a
`uses:` clause and reruns the audits against them:

```sh
loopt lint --online
```

The audits covered are `impostor-commit`, `ref-confusion`,
`known-vulnerable-actions`, `stale-action-refs` (pedantic persona),
`ref-version-mismatch`, and `archived-uses`. Gathering uses the same
authenticated `gh` CLI (`gh api`) as `loopt lock`, so `gh auth login` or
`GH_TOKEN` is required. Each repository is fetched once. If `gh` is missing or
not authenticated, `--online` fails with a clear error instead of silently
skipping the checks; a repository that cannot be read aborts the run rather
than being reported clean.

`--online` composes with `--persona` and `--json`. GitHub API rate limits
apply, so very large workflow sets may take a while or need a token with a
higher limit.

## Shell scripts (shellcheck)

`run:` scripts are also checked with [shellcheck](https://www.shellcheck.net)
when it is installed. Only `bash` and `sh` scripts are checked: `bash` is the
default on Linux and macOS runners, `pwsh` on Windows, and `shell:` at step,
job or workflow level overrides it. `${{ }}` expressions are masked before the
script is checked, and the codes that conflict with expression syntax (SC1091,
SC2043, SC2050, SC2154, SC2157, SC2194) are excluded. Findings appear as
`shellcheck/SC####`; shellcheck's `error`, `warning` and `info`/`style` levels
map to Loopt's error, warning and info.

```sh
loopt lint --shellcheck on                       # fail when shellcheck is missing
loopt lint --shellcheck off                      # disable
loopt lint --shellcheck-path /opt/bin/shellcheck
```

The default is `auto`: use shellcheck when it can be run and skip it silently
otherwise. shellcheck is GPL-3, so Loopt runs it as a separate program and
never bundles it.
