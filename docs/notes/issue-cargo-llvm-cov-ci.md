# cargo-llvm-cov CI job — what was implemented

## Problem

There was no code coverage measurement or reporting in CI, making it hard
to spot untested branches before they ship.

## What changed

- Added `.github/workflows/coverage.yml`: runs `cargo-llvm-cov` against the
  workspace on every push/PR to `main`, producing both an `lcov.info` file
  and an HTML report, each uploaded as a build artifact.
- The workflow also runs `cargo llvm-cov --workspace --fail-under-lines 60`
  as a baseline gate. 60% was picked as a threshold the current suite
  already clears comfortably (this repo's test suite has good scenario
  coverage per the existing `test.rs`), so the check is meaningful against
  a large regression (e.g. a new entry point with no tests) without
  blocking unrelated PRs over small, incidental dips.
- Documented local usage in `CONTRIBUTING.md` ("Code coverage
  (cargo-llvm-cov)"): install steps, and the three common invocations
  (terminal summary, lcov output, HTML report).
- Runs against the host target, not `wasm32v1-none` — coverage
  instrumentation doesn't need to run under the real deployment target for
  the numbers to be meaningful, matching how the existing `test.yml`
  already separates `cargo test --workspace` from the release wasm build.

## Follow-up

The 60% baseline is deliberately conservative. Once this workflow has run
on `main` a few times and the actual baseline is visible from the uploaded
reports, consider raising the threshold to match reality more closely so
the gate does more than catch catastrophic regressions.
