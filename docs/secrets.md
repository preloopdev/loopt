# Secrets

```sh
loopt secrets sync --repo owner/repo --environment prod
```

Writes **names only** — never values — to `.loopt/secrets.json` (a
`--context`-compatible lint inventory) and `.loopt/secrets.ts` (typed
`secrets` accessors for the SDK, `--sdk-import` to override). Repository and
organization-visible names merge into `available_secrets`; `--environment`
scopes the per-environment inventory while `known_environments` records
every environment GitHub returned. Nothing is written unless every API call
succeeds.

`loopt lint` loads `.loopt/secrets.json` automatically, so missing secrets
become lint findings instead of surprises at runtime.
