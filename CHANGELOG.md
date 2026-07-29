# Changelog

All notable changes to the SoroWill contracts are documented in this file.

## [Unreleased]

### Breaking

- **`benefup` (`beneficiaries_updated`) event payload changed.** Previously
  published as `(will_id) -> owner`. Now published as
  `(will_id) -> (owner, beneficiary_count: u32, beneficiaries: Vec<Beneficiary>)`.
  Off-chain indexers and event subscribers (the SDK's event subscription
  layer, the app's activity feed) that deserialize this event's data as a
  single `Address` must update to deserialize a 3-tuple instead, or they will
  fail to decode the event. This removes the need for a follow-up `get_will`
  call just to see the new beneficiary list after an update.

### Added

- `get_will_status(env, will_id) -> WillStatus` and
  `get_time_until_deadline(env, will_id) -> Option<i64>` query entry points,
  for callers that only need a will's status or deadline without loading the
  full `Will` struct.
- `docs/adr/0001-guardian-threshold.md`, recording the rationale for the
  2-of-3 guardian default and how it relates to the proposed configurable
  M-of-N guardian feature.
- An integration test layer (`scripts/integration_test.sh`,
  `contracts/will/tests/integration.rs`) that runs the compiled `.wasm`
  artifact through `stellar contract invoke` against a local Soroban
  network, plus a dedicated `Integration` CI job.
