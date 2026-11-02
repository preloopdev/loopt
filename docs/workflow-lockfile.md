# Workflow lockfile

```sh
loopt lock                # write .github/workflows/actions.lock
loopt lock --verify       # network: fail if a pinned ref moved or ids changed
loopt lock --verify-local # offline: every uses: has a lockfile entry
```

`loopt lock` resolves each step-level `uses:` to a full commit SHA plus the
repository's `owner_id`/`repo_id`, expands composite actions' own `uses:`
steps transitively, and writes `.github/workflows/actions.lock` — the same
file and `v0.0.2` format GitHub's `gh-actions-lock` extension and the hosted
runner enforce, so a lockfile written by one tool verifies under the other.

Local `./…`, `$/…`, `docker://…` and `${{ }}`-computed `uses:` values cannot
be pinned to a commit and are reported as notes; reusable-workflow calls
(`jobs.<id>.uses`) are skipped, matching upstream.
