# create_will atomicity test — what was implemented

## Problem

`create_will` transfers each locked token from the owner into the contract
via `token::Client::transfer` *before* writing the `Will` record, bumping
`NextWillId`, and writing the owner/beneficiary index entries. Soroban
guarantees that a trap anywhere in a contract invocation rolls back every
storage write made earlier in that same invocation, so a failed transfer
(insufficient balance, insufficient allowance to the SAC, etc.) should leave
no trace behind. That guarantee was never directly exercised by a test in
this repo — it was an assumption about host behavior, not a verified
contract property.

## What changed

Added `test_create_will_reverts_atomically_on_insufficient_balance` to
`contracts/will/src/test.rs`:

- Calls `try_create_will` with a token amount far exceeding the owner's
  minted balance and asserts the call returns an error instead of panicking
  the test harness.
- Asserts `get_wills_by_owner` and `get_wills_by_beneficiary` return empty
  lists for the failed attempt — i.e. no orphaned `Will` record and no
  leftover index entries.
- Asserts the owner's and contract's token balances are unchanged (the
  failed transfer moved no funds).
- Immediately follows up with a real `create_will` call and asserts it
  allocates will id `1` — proving `NextWillId` was not incremented by the
  failed attempt.

## Follow-up

None identified; this closes the gap described in the issue. If a future
change makes any storage write in `create_will` happen *before* the token
transfer loop, this test should start failing and would need updating
alongside that change.
