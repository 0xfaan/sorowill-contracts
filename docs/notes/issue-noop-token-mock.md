# Non-conforming token mock — what was implemented

## Problem

Every existing test exercises `WillContract` against the real Stellar Asset
Contract (SAC), which faithfully implements SEP-41's `transfer`. But
`create_will`'s `tokens` parameter accepts any `Address` with no interface
validation (see issue #74), so nothing in the contract stops an owner from
locking a will against a token that doesn't behave like a real SAC — e.g.
one whose `transfer` silently no-ops instead of moving funds. That gap had
no test coverage.

## What changed

Added to `contracts/will/src/test.rs`:

- `NoopToken`, a minimal mock contract (`mint`, `balance`, `transfer`)
  whose `transfer` is a pure no-op — it never touches storage and never
  moves any balance, unlike `test_support::MaliciousToken` (issue #55's
  reentrancy harness), which does track balances and instead attempts to
  reenter the caller.
- `test_create_will_with_noop_token_records_unbacked_balance`, which calls
  `create_will` against `NoopToken` and shows the contract happily records
  a `Will` with a locked balance that was never actually backed by a real
  transfer (the owner is never even funded, and the mock's `balance` always
  returns `0`).

## Current (likely broken/inconsistent) behavior demonstrated

`create_will` trusts `token::Client::transfer` unconditionally — it never
checks that a balance actually moved. Against a real SAC this is safe
because SAC's `transfer` reverts on failure. Against a token like
`NoopToken` that "succeeds" without moving funds, the contract's internal
accounting (`Will.balances`) silently diverges from the token's real
balances, and later a `release_inheritance`/`cancel_will` payout against
that token would itself no-op or fail, stranding beneficiaries with a will
that claims to hold funds it never received.

## Follow-up

Fixing this is out of scope here — it depends on the upfront token
interface/behavior validation tracked in issue #74 (e.g. reading the
owner's and/or contract's balance before and after each transfer and
asserting it changed by the expected amount). This test exists to make the
current behavior explicit and regression-testable once that follow-up
lands.
