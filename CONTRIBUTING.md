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

## Integration tests

`cargo test` only exercises contract logic through soroban_sdk's in-process
`Env::default()` test harness — it never touches the compiled `.wasm`
artifact or a real Soroban host. To catch build- or host-environment-specific
issues (wasm size limits, host function availability, XDR encoding
mismatches), there's a separate integration layer that builds the real wasm,
deploys it to a local Soroban network, and drives a handful of core lifecycle
calls (`create_will`, `get_will`, `get_will_status`, `check_in`,
`cancel_will`) through the real `stellar contract invoke` CLI.

To run it locally:

```bash
# Requires: stellar-cli (>= 22.0.0), docker, jq
./scripts/integration_test.sh
```

The script builds `will.wasm` for `wasm32v1-none`, starts a local standalone
Soroban network (via `stellar network start local`, which uses the quickstart
docker image), deploys the contract, and asserts the will's status
transitions correctly across the calls above. It tears the local network
down on exit, whether it succeeds or fails.

The same lifecycle check is also wrapped in an `#[ignore]`-gated Rust test at
[`contracts/will/tests/integration.rs`](./contracts/will/tests/integration.rs),
runnable directly with:

```bash
cargo test --package will --test integration -- --ignored
```

This is intentionally excluded from the fast `cargo test --workspace` path
(see the `test.yml` CI job) since it needs `stellar` + `docker` and takes
substantially longer. It instead runs as its own `Integration` CI job (see
`.github/workflows/integration.yml`) on pushes to `main`, on a daily
schedule, and on manual `workflow_dispatch` — mirroring how the `fuzz.yml`
job is kept out of the per-PR path.

## Learn more

Full details on how Wave Programs work — applying, Points, rewards, and payouts — are documented at <https://drips.network/wave>.
