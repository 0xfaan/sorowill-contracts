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

## Local setup

See the [README](./README.md#local-setup) for toolchain installation and how to run the test suite.

## Handling a flagged security advisory

The [Security Audit](.github/workflows/audit.yml) workflow runs `cargo audit` against `Cargo.lock` on every PR, on every push to `main`, and once a day on a schedule so newly published advisories against dependencies already in the lockfile are caught too. If it fails:

1. **Prefer upgrading.** Run `cargo update -p <crate>` (or bump the version in `Cargo.toml`/`contracts/will/Cargo.toml` if the fix needs a semver-major release), then confirm `cargo test` and `cargo clippy --all-targets -- -D warnings` still pass.
2. **If no fix is available yet**, and you've confirmed the advisory doesn't apply to how this contract actually uses the crate (for example, the vulnerable code path is never reachable from the `no_std` wasm build), add the RUSTSEC id to the `ignore` list in [`audit.toml`](./audit.toml) with a comment explaining the justification and, if known, a tracking issue for the real fix. Never add an entry without a comment.
3. Re-run `cargo audit --file Cargo.lock --deny warnings` locally (or `just audit`) to confirm the workflow will pass before opening the PR.

## Updating deployments/testnet.json after a redeploy

`deployments/testnet.json` is the source of truth integrators (the SDK, the app) use to find the live testnet contract, so keep it accurate:

1. Run `DEPLOY_IDENTITY=<your funded identity> ./scripts/deploy-testnet.sh` from the repo root (see [README.md#testnet-deployment](./README.md#testnet-deployment)). It builds the contract, deploys it, and overwrites `deployments/testnet.json` with the new contract id and an ISO-8601 timestamp.
2. Review the diff (`git diff -- deployments/testnet.json`) before committing — a routine redeploy should only ever change `WillContract` and `deployedAt`, never `network`.
3. Commit the updated file on its own, with a message that includes the new contract id, e.g. `chore: record testnet deployment CABC...`.
4. The scheduled [Testnet Deployment Drift Check](.github/workflows/testnet-drift-check.yml) workflow compares the on-chain wasm hash for the recorded contract id against the wasm built from `main` once a day. If you forget to update this file after a redeploy, that job fails loudly instead of letting the recorded id silently drift from what's actually on-chain.

## Learn more

Full details on how Wave Programs work — applying, Points, rewards, and payouts — are documented at <https://drips.network/wave>.
