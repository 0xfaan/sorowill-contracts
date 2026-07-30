# Native XLM Support - Implementation TODO

## Step 1: Modify `types.rs` — Add `is_native` field to `Will` struct
- [x] Add `pub is_native: bool` field to the `Will` struct

## Step 2: Modify `lib.rs` — Add native XLM transfer logic
- [x] Add `transfer_funds` helper function that routes to either `env.transfer()` (native) or `token::Client::transfer()` (SAC)
- [x] Add `balance_of` helper to check balance (native via `env.balance()` or SAC via `token_client.balance()`)
- [x] Update `create_will`: add `is_native` parameter, route transfer, store in will
- [x] Update `cancel_will`: route refund based on `will.is_native`
- [x] Update `top_up`: route transfer based on `will.is_native`
- [x] Update `distribute`: route beneficiary transfers based on `will.is_native`

## Step 3: Modify `test.rs` — Add native XLM test coverage
- [x] Add `setup_native()` helper (funds owner from test source account)
- [x] Add `test_native_create_will_success`
- [x] Add `test_native_checkin_resets_deadline`
- [x] Add `test_native_trigger_and_release`
- [x] Add `test_native_release_splits_multiple_beneficiaries`
- [x] Add `test_native_emergency_checkin`
- [x] Add `test_native_cancel_will`
- [x] Add `test_native_top_up`
- [x] Add `test_native_guardian_trigger`
- [x] Add `test_native_rounding_remainder` (multi-beneficiary with remainder)
- [x] Add `test_native_cannot_trigger_before_deadline`
- [x] Add `test_native_cannot_release_during_grace_period`

## Step 4: Run tests and verify
- [ ] Run `cargo test` and confirm all checks pass (Rust toolchain not available on this machine)
