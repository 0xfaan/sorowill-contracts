# cargo-mutants CI job — what was implemented

## Problem

The test suite has good scenario coverage, but nothing verified that the
tests would actually *fail* if a bug were introduced. Line/branch coverage
can't catch a weak assertion or a missing edge case — mutation testing can.

## What changed

- Added `.github/workflows/mutants.yml`: a separate, non-blocking
  (`continue-on-error: true`) GitHub Actions workflow that installs
  `cargo-mutants` and runs it against the `will` package on every push/PR
  to `main`, uploading the report as a build artifact.
- Documented in `CONTRIBUTING.md` ("Mutation testing (cargo-mutants)")
  how to install and run `cargo-mutants` locally, how CI wires it up, and
  how to read a `caught` / `missed` (survived) / `unviable` / `timeout`
  result — including guidance on when a survivor is a real test gap versus
  a defensible, documented exception.

## Initial-run triage

This PR adds the tooling and workflow; it does not include a completed
triage of a first `cargo-mutants` run — that requires actually executing
the tool against a checked-out build environment, which is outside the
scope of this change. Follow-up:

- Run `cargo mutants --package will` once this workflow lands on `main` and
  a first report is available as a build artifact.
- Triage survivors by module, prioritizing `contracts/will/src/lib.rs`
  (the entry points) and `contracts/will/src/storage.rs` (index/state
  bookkeeping), since those are the modules with the most business logic
  per line.
- For each survivor: either strengthen the relevant test in
  `contracts/will/src/test.rs`, or, if it's a defensible exception (e.g. a
  redundant defensive check), leave it and note why in the PR/issue that
  triages it.
- File a follow-up issue for any survivor batch that's large enough to
  warrant its own scoped PR rather than being fixed inline.
