# Architectural decision record (ADR) 002: Main owns CodeScene coverage publication

## Status

Accepted, 2026-10-10.

## Date

2026-10-10.

## Context and problem statement

CodeScene accepts `cs-coverage upload` only for an analysed branch, which a
pull request head is not, and its check mode fails on every project whose
coverage gates are off. A pull-request lane that contacts CodeScene therefore
fails for reasons the change under review does not cause, and a repository that
has no CodeScene project yet cannot be wired at all. The estate rule CV-005
moves publication to `main`.

## Decision outcome

Pull requests generate coverage for their own ratchet check only
(`with-ratchet: 'true'`, `publish-artefact: 'false'`) and never contact
CodeScene. A single publisher, `.github/workflows/coverage-main.yml`, runs on
push to `main` in the `codescene` environment, generates coverage and uploads
it with `mode: upload`, guarded on `github.ref == 'refs/heads/main'` alone. The
token reaches the uploader only through its `access-token` input. When the
repository has no `CS_ACCESS_TOKEN`, the uploader records a
`CodeScene upload skipped` notice and a step-summary line and the run succeeds,
so a missing project is visible without a red run. The shared `cv005-contracts`
check, run from a pinned commit by `make test-workflow-contracts`, holds this
shape.

## Consequences

- A merge that fires no push event (a Dependabot pull request merged with
  `GITHUB_TOKEN`) publishes nothing until the next push to `main`.
- The concurrency group stops runs overlapping but does not order them by
  commit; a manual re-run of an older run republishes that commit's coverage
  until the next push.
- A repository gains CodeScene coverage by creating its project and setting the
  secret, with no workflow change.
