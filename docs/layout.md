# Repository layout

```text
.loopt/
  manifest.json          source → generated workflow mapping + fingerprints
  workflows/             editable workflow sources (per language)
    ci.ts
    deploy/              Rust/Go-style project sources are directories
      main.rs
      Cargo.toml
  sdk/
    rust/                vendored loopt-sdk (written by `import --language rust`)
    typescript/          vendored TypeScript SDK (written by actions/secrets)
  actions/               generated typed action bindings (loopt actions add)
  actions.lock           typed-binding pins (see docs/action-bindings.md)
  secrets.json           names-only secret inventory (loopt secrets sync)
  .cache/                shared build caches (e.g. rust-target)
.github/
  workflows/             generated output — never edit
    ci.yml
    actions.lock         workflow uses: pins (loopt lock)
    reusable.lock        reusable-workflow call pins (loopt lock)
```

`.loopt/manifest.json` records which source produced each output, the last
accepted YAML, and fingerprints — `loopt gen --check` and the clobber guards
key off it.
