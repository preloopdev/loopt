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

`.loopt/secrets.json` (from `loopt secrets sync`) is loaded automatically as
the default context; `--context` overrides it.

`loopt gen` runs the same review automatically; pass `--no-lint` to skip it.

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
