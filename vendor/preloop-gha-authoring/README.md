# preloop-gha-authoring

Source-preserving GitHub Actions workflow authoring documents.

`WorkflowDocument` retains unknown fields, declaration order, scalar types,
expressions, aliases, and unchanged YAML bytes. It is the shared frontend used
by Loopt to import and generate workflows without losing fields the execution
schema does not know about.

This crate is MIT licensed. It is a subset of Preloop's shared parser focused
on authoring; the execution-plan parser lives in Preloop's private control
plane. See `PROVENANCE.md`.
