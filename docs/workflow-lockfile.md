# Workflow lockfile

```sh
loopt lock                # write .github/workflows/actions.lock and reusable.lock
loopt lock --verify       # network: fail if a pinned ref moved or ids changed
loopt lock --verify-local # offline: every uses: has a lockfile entry
```

`loopt lock` writes two files:

- `.github/workflows/actions.lock` — the `v0.0.2` format GitHub's
  `gh-actions-lock` extension and the hosted runner enforce. It pins every
  remote step-level `uses:` to a full commit SHA plus the repository's
  `owner_id`/`repo_id`, and expands composite actions' own `uses:` steps
  transitively. A lockfile written by either tool verifies under the other;
  loopt also reads the `v0.0.3` files current `gh actions-lock` releases
  write (v0.0.3 only adds an optional `hostname`, and loopt rejects
  hostname-bound pins it cannot resolve).
- `.github/workflows/reusable.lock` — loopt's own `v0.0.1` format for
  reusable-workflow calls (`jobs.<id>.uses`). GitHub Actions and
  `gh actions-lock` never read this file.

## Reusable workflows

Remote reusable calls are pinned by their full key, so two workflows in one
repository never collide:

```yaml
version: 'v0.0.1'
dependencies:
  'octo/ci/.github/workflows/lint.yml@v2':
    ref: 'v2'
    commit: 'sha1-…'
    owner_id: 1234
    repo_id: 5678
    uses:
      - 'actions/checkout@v4'
      - 'octo/ci/.github/workflows/test.yml@v2'
workflows:
  '.github/workflows/ci.yml':
    - 'octo/ci/.github/workflows/lint.yml@v2'
    - 'actions/checkout@v4'
    - 'octo/ci/.github/workflows/test.yml@v2'
```

Each remote call resolves to a commit SHA and the callee workflow is fetched
at that SHA, so its step `uses:` and its own nested reusable calls are locked
transitively (with cycle protection). Local `./…` callees cannot be pinned
themselves, but their files are read so their `uses:` are locked too; a local
callee is a workflow in this repository, so its pins live in `actions.lock`
under its own workflow entry. `${{ }}`-computed and `docker://` values are
reported as notes.

### Why a second file?

GitHub's `actions.lock` schema cannot represent reusable workflows:

- The schema sets `additionalProperties: false` at the root and on every
  dependency entry, and its parser rejects unknown top-level or action fields,
  so a namespaced loopt section inside `actions.lock` would make
  `gh actions-lock` treat the file as corrupt.
- Pin keys are `OWNER/REPO@REF` with no workflow path, so two reusable
  workflows in one repository at the same ref would collide.
- GitHub's documentation states the lockfile "doesn't pin calls to reusable
  workflows (`jobs.<job_id>.uses`)" and that "a reusable workflow's
  dependencies are covered by the lockfile of the repository where it's
  defined, not by the lockfile of the repository that calls it." Upstream
  keeps them out deliberately
  ([gh-actions-lock#179](https://github.com/github/gh-actions-lock/issues/179)).

Keeping reusable pins in a sibling file leaves `actions.lock` byte-for-byte
compatible with `gh actions-lock`: `gh actions-lock --verify` passes, and its
write mode never prunes or rewrites `reusable.lock`. The hosted runner does
not enforce reusable calls from any lockfile, so the strongest production
control remains referencing `jobs.<id>.uses` by full commit SHA.

## Upgrading

`actions.lock` files written before reusable support still parse unchanged.
After upgrading, `loopt lock --verify-local` reports every remote
`jobs.<id>.uses` that has no entry in `reusable.lock`; run `loopt lock` once
to create it. Repositories without reusable calls never get a `reusable.lock`.

Local `./…` actions, `$/…` specifiers, `docker://…` images and `${{ }}`
expressions cannot be pinned to a commit and are reported as notes.
