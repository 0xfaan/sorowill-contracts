# Contributing to sorowill-contracts

This repo participates in the **Stellar Wave Program** on [Drips](https://drips.network/wave). Contribution work is tied to issues that maintainers tag for an active Wave, and contributors earn rewards proportional to the Points assigned to the issues they resolve.

## Ground rules

- **Do not start work on any issue until you have been assigned by the maintainer.** Applying to an issue does not mean you're assigned — wait for confirmation (via the Drips Wave dashboard or a direct assignment on GitHub) before opening a PR.
- Keep PRs scoped to the issue they resolve. Unrelated changes slow down review and can cost you the Wave window.
- Be responsive during an active Wave — issues must be resolved before the Wave ends for Points to be awarded.

## Branch naming

Use the issue number in your branch name:

```
feat/N-short-description
fix/N-short-description
```

For example: `feat/42-guardian-quorum-check` or `fix/17-checkin-deadline-rounding`.

## Pull requests

- Your PR description must reference the issue it resolves (e.g. `Closes #42`).
- Make sure `cargo test` and `cargo clippy --all-targets` both pass cleanly before requesting review.
- Add or update unit tests for any behavior change in `contracts/will/src/test.rs`.
- If you change validation rules or add an entry point, extend the fuzzing
  harness too — see [docs/FUZZING.md](./docs/FUZZING.md#adding-a-target).
- If your PR changes contract behavior (new entrypoint, validation change,
  event/error schema change, etc.), add an entry under `[Unreleased]` in
  [CHANGELOG.md](./CHANGELOG.md) describing it in the "Added" / "Changed" /
  "Fixed" section that fits. Pure docs/tooling/test-only PRs don't need one.
- If your PR adds a new `WillError` variant, add a row for it to the
  [error code table in the README](./README.md#error-codes) in the same PR —
  don't let the table and `contracts/will/src/errors.rs` drift apart.
- If your PR changes a public entry-point signature or a shared type
  (`Will`, `Beneficiary`, `Guardian`, `WillStatus`, `WillError`, ...), bump
  the crate `version` in `contracts/will/Cargo.toml` and regenerate the
  contract spec artifact — see [spec/README.md](./spec/README.md).

## Local setup

See the [README](./README.md#local-setup) for toolchain installation and how to run the test suite.

## Learn more

Full details on how Wave Programs work — applying, Points, rewards, and payouts — are documented at <https://drips.network/wave>.
