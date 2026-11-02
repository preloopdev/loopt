# Publishing

Loopt ships three things: the `loopt` binary (GitHub Releases, via `install.sh`),
the Rust authoring SDK (crates.io), and the TypeScript authoring SDK (npm).
All versions move in lockstep: bump `version` in `Cargo.toml`,
`sdk/rust/Cargo.toml`, and `sdk/typescript/package.json` together.

## Release checklist

```sh
# 1. Bump versions in all three manifests, commit
# 2. Cut the GitHub release (builds + uploads binaries, tags the repo)
scripts/release.sh X.Y.Z

# 3. Publish the SDKs
scripts/publish-sdks.sh
```

`scripts/publish-sdks.sh` runs `npm publish --access public` in
`sdk/typescript` and `cargo publish -p loopt-sdk`.

## Credentials

### crates.io — `loopt-sdk`

crates.io has no org namespacing; `loopt-sdk` is a global name. Auth is a
user token with `publish-new` + `publish-update` scopes (mint one at
<https://crates.io/settings/tokens>), passed as `CARGO_REGISTRY_TOKEN` or via
`cargo login`.

Ownership lives with the **`preloopdev` GitHub org's `loopt` team**
(`github:preloopdev:loopt`), added at <https://crates.io/crates/loopt-sdk/settings>.
Any team member can then publish with their own token — the personal token is
only needed for `publish-new` on the first release and for owner management.

```sh
CARGO_REGISTRY_TOKEN=cio… cargo publish -p loopt-sdk
```

### npm — `@preloopdev/sdk`

npm scopes are npm-side orgs, unrelated to GitHub. The `preloopdev` npm org
must exist (create at <https://www.npmjs.com/org/create> — free plan covers
unlimited public packages) and the publishing account must be a member.

Use a **granular access token** scoped to `@preloopdev` with read+write
(Account → Access Tokens → Granular Access Token). Classic tokens work too.
Pass via `NODE_AUTH_TOKEN`/`npm publish` after `npm login`, or set
`//registry.npmjs.org/:_authToken=` in `~/.npmrc`.

```sh
cd sdk/typescript && npm publish --access public
```

The package ships raw `.ts` sources (`files: ["src"]`, `exports` →
`src/index.ts`); `prepublishOnly` runs typecheck + the test suite.

## Shared crates

`scripts/publish-crates.sh` publishes the `preloop-gha-*` workspace crates
`loopt` depends on (expressions, authoring, lint, sec) plus `loopt-sdk`, in
dependency order. These are published so `cargo install loopt` and library
consumers can resolve them; publish them before `loopt` itself if its
manifest ever declares registry versions.
