# Schema provenance

These files are GitHub's own workflow and action template schemas, vendored unmodified.

- Source: https://github.com/actions/languageservices (path `workflow-parser/src/`)
- Pinned commit: `4043eda158e16579cc5fb1b0b07a4bce2a76f0b5`
- License: MIT (see `LICENSE.actions-languageservices`)
- Files: `workflow-v1.0.json`, `action-v1.0.json`

To update: fetch both files at a newer commit, replace them, update the commit above, and run the crate tests.
The reference-resolution tests fail loudly if the format changed.
