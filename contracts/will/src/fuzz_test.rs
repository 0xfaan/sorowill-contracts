#![cfg(test)]

//! Property-based fuzzing of `create_will` and `update_beneficiaries`.
//!
//! These tests drive the same runners as the `cargo-fuzz` targets under
//! `fuzz/` (see [`crate::fuzz_harness`]), but with `proptest` supplying the
//! input. That keeps the invariants enforced by ordinary `cargo test` — and so
//! by CI — on stable Rust, without anyone needing a nightly toolchain.
//!
//! `cargo-fuzz` explores much deeper; `proptest` here is the regression net.
//! Every bug the fuzzer has actually found is additionally pinned by a
//! hand-written test at the bottom of this file, so a regression fails loudly
//! and deterministically rather than waiting for a lucky random draw.

use proptest::prelude::*;
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Vec as SorobanVec,
};

use crate::fuzz_harness::{
    run_create_will, run_update_beneficiaries, BeneficiarySpec, CreateWillInput, Outcome,
    UpdateBeneficiariesInput,
};
use crate::{Beneficiary, WillContract, WillContractClient, WillError};

/// Basis-point shares worth trying: the valid range and extremes.
fn basis_points_strategy() -> impl Strategy<Value = u32> {
    prop_oneof![
        4 => 0u32..=10_001,
        1 => Just(0u32),
        1 => Just(10_000u32),
        1 => Just(u32::MAX),
        1 => Just(u32::MAX / 2),
        1 => Just(u32::MAX - 1),
        1 => any::<u32>(),
    ]
}

fn beneficiary_spec_strategy() -> impl Strategy<Value = BeneficiarySpec> {
    (any::<u8>(), basis_points_strategy()).prop_map(|(address_slot, basis_points)| BeneficiarySpec {
        address_slot,
        basis_points,
    })
}

/// Lists that straddle the 1..=10 limit on both sides, including empty.
fn beneficiaries_strategy() -> impl Strategy<Value = Vec<BeneficiarySpec>> {
    prop::collection::vec(beneficiary_spec_strategy(), 0..13)
}

/// Periods worth trying: zero, sane values, the exact overflow threshold for
/// `days * 86_400`, and the extremes beyond it.
fn period_strategy() -> impl Strategy<Value = u64> {
    prop_oneof![
        4 => 0u64..=4_000,
        1 => Just(0u64),
        1 => Just(u64::MAX),
        1 => Just(u64::MAX / 86_400),
        1 => Just(u64::MAX / 86_400 + 1),
        1 => any::<u64>(),
    ]
}

/// Amounts worth trying: zero, negatives, ordinary values, and amounts far
/// beyond what the owner was funded with.
fn amount_strategy() -> impl Strategy<Value = i128> {
    prop_oneof![
        4 => 1i128..=1_000_000_000,
        1 => Just(0i128),
        1 => Just(-1i128),
        1 => Just(i128::MIN),
        1 => Just(i128::MAX),
        1 => any::<i128>(),
    ]
}

fn create_will_input_strategy() -> impl Strategy<Value = CreateWillInput> {
    (
        amount_strategy(),
        amount_strategy(),
        beneficiaries_strategy(),
        prop::collection::vec(any::<u8>(), 0..6),
        period_strategy(),
        period_strategy(),
        1u32..=5,
        any::<bool>(),
    )
        .prop_map(
            |(
                mint,
                amount,
                beneficiaries,
                guardian_slots,
                checkin_period_days,
                grace_period_days,
                guardian_threshold,
                release_after_create,
            )| CreateWillInput {
                mint,
                amount,
                beneficiaries,
                guardian_slots,
                checkin_period_days,
                grace_period_days,
                guardian_threshold,
                release_after_create,
            },
        )
}

fn update_beneficiaries_input_strategy() -> impl Strategy<Value = UpdateBeneficiariesInput> {
    (
        create_will_input_strategy(),
        prop::collection::vec(beneficiaries_strategy(), 0..5),
        any::<bool>(),
    )
        .prop_map(|(create, updates, probe_non_owner)| UpdateBeneficiariesInput {
            create,
            updates,
            probe_non_owner,
        })
}

// Each case registers a fresh Soroban env plus two contracts, so the case
// counts are kept modest to hold the suite to a few seconds. Deep exploration
// is cargo-fuzz's job; see docs/FUZZING.md.
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 48,
        max_shrink_iters: 512,
        ..ProptestConfig::default()
    })]

    /// `create_will` must reject malformed input with a declared `WillError`,
    /// never by aborting, and every will it accepts must satisfy the
    /// invariants documented on `Will`.
    #[test]
    fn create_will_upholds_invariants(input in create_will_input_strategy()) {
        if let Outcome::Accepted(will_id) = run_create_will(&input) {
            // Ids are allocated from a counter that starts at 1; a zero id
            // would mean `get_will` could never find the will again.
            prop_assert!(will_id >= 1);
        }
    }

    /// `update_beneficiaries` must likewise never abort, must leave the will
    /// untouched when it rejects an update, and must keep the beneficiary
    /// reverse index in step with the list it stores.
    #[test]
    fn update_beneficiaries_upholds_invariants(input in update_beneficiaries_input_strategy()) {
        let outcomes = run_update_beneficiaries(&input);
        // One outcome per replacement list actually applied.
        prop_assert!(outcomes.len() <= input.updates.len());
    }
}

// ---------------------------------------------------------------------------
// Regression tests for the defects this harness found.
// ---------------------------------------------------------------------------

/// Sets up a will contract with a funded owner, mirroring `test::setup` but
/// returning only what the regression tests below need.
fn setup<'a>() -> (Env, WillContractClient<'a>, Address, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env, client, owner, token_address)
}

fn single_beneficiary(env: &Env, basis_points: u32) -> SorobanVec<Beneficiary> {
    vec![
        env,
        Beneficiary {
            address: Address::generate(env),
            basis_points,
        },
    ]
}

/// Two huge basis-point values used to overflow the running total in
/// `assert_valid_percentages`, aborting the contract instead of returning
/// `InvalidPercentages`.
#[test]
fn create_will_rejects_overflowing_basis_points() {
    let (env, client, owner, token) = setup();
    let beneficiaries = vec![
        &env,
        Beneficiary {
            address: Address::generate(&env),
            basis_points: u32::MAX,
        },
        Beneficiary {
            address: Address::generate(&env),
            basis_points: u32::MAX,
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];

    assert_eq!(
        client.try_create_will(&owner, &tokens, &beneficiaries, &90, &7, &vec![&env], &2, &None),
        Err(Ok(WillError::InvalidPercentages.into()))
    );
}

/// A basis-point sum that does not equal 10,000 must be rejected.
#[test]
fn create_will_rejects_non_10000_basis_points() {
    let (env, client, owner, token) = setup();
    let beneficiaries = single_beneficiary(&env, 9_999);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];

    assert_eq!(
        client.try_create_will(&owner, &tokens, &beneficiaries, &90, &7, &vec![&env], &2, &None),
        Err(Ok(WillError::InvalidPercentages.into()))
    );
}

/// A check-in period beyond `u64::MAX / 86_400` used to overflow while
/// computing the deadline for the `WillCreated` event.
#[test]
fn create_will_rejects_overflowing_checkin_period() {
    let (env, client, owner, token) = setup();
    let beneficiaries = single_beneficiary(&env, 10_000);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];
    let overflowing = u64::MAX / 86_400 + 1;

    assert_eq!(
        client.try_create_will(
            &owner,
            &tokens,
            &beneficiaries,
            &overflowing,
            &7,
            &vec![&env],
            &2,
            &None,
        ),
        Err(Ok(WillError::InvalidPeriod.into()))
    );
}

/// A grace period that cannot be converted to a timestamp would leave the
/// balance unreleasable, since `release_inheritance` overflows on every call.
#[test]
fn create_will_rejects_overflowing_grace_period() {
    let (env, client, owner, token) = setup();
    let beneficiaries = single_beneficiary(&env, 10_000);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];

    assert_eq!(
        client.try_create_will(
            &owner,
            &tokens,
            &beneficiaries,
            &90,
            &u64::MAX,
            &vec![&env],
            &2,
            &None,
        ),
        Err(Ok(WillError::InvalidPeriod.into()))
    );
}

/// A zero-day check-in period makes the will triggerable in the very ledger it
/// was created in, which defeats the dead man's switch.
#[test]
fn create_will_rejects_zero_length_periods() {
    let (env, client, owner, token) = setup();
    let beneficiaries = single_beneficiary(&env, 10_000);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];

    assert_eq!(
        client.try_create_will(&owner, &tokens, &beneficiaries, &0, &7, &vec![&env], &2, &None),
        Err(Ok(WillError::InvalidPeriod.into()))
    );
    assert_eq!(
        client.try_create_will(&owner, &tokens, &beneficiaries, &90, &0, &vec![&env], &2, &None),
        Err(Ok(WillError::InvalidPeriod.into()))
    );
}

/// The longest allowed periods must still be accepted.
#[test]
fn create_will_accepts_maximum_periods() {
    let (env, client, owner, token) = setup();
    let beneficiaries = single_beneficiary(&env, 10_000);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];

    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &crate::MAX_PERIOD_DAYS,
        &crate::MAX_PERIOD_DAYS,
        &vec![&env],
        &2,
        &None,
    );
    assert_eq!(
        client.get_will(&will_id).checkin_period_days,
        crate::MAX_PERIOD_DAYS
    );
}

/// A guardian list of `[g, g]` looks like a 2-of-2 quorum but can only ever
/// reach one vote, so the guardian override could never fire.
#[test]
fn create_will_rejects_duplicate_guardians() {
    let (env, client, owner, token) = setup();
    let beneficiaries = single_beneficiary(&env, 10_000);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];
    let guardian = Address::generate(&env);

    assert_eq!(
        client.try_create_will(
            &owner,
            &tokens,
            &beneficiaries,
            &90,
            &7,
            &vec![&env, guardian.clone(), guardian],
            &2,
            &None,
        ),
        Err(Ok(WillError::DuplicateGuardian.into()))
    );
}

/// `update_guardians` shares the validation, so it must reject duplicates too.
#[test]
fn update_guardians_rejects_duplicate_guardians() {
    let (env, client, owner, token) = setup();
    let beneficiaries = single_beneficiary(&env, 10_000);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];
    let guardian = Address::generate(&env);

    let will_id =
        client.create_will(&owner, &tokens, &beneficiaries, &90, &7, &vec![&env], &2, &None);

    assert_eq!(
        client.try_update_guardians(&will_id, &owner, &vec![&env, guardian.clone(), guardian]),
        Err(Ok(WillError::DuplicateGuardian.into()))
    );
}

/// The same overflow reachable through `create_will` is reachable through
/// `update_beneficiaries`, which validates the replacement list independently.
#[test]
fn update_beneficiaries_rejects_overflowing_basis_points() {
    let (env, client, owner, token) = setup();
    let beneficiaries = single_beneficiary(&env, 10_000);
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token.clone(), 1_000_000_i128)];

    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &90,
        &7,
        &vec![&env],
        &2,
        &None,
    );

    let replacement = vec![
        &env,
        Beneficiary {
            address: Address::generate(&env),
            basis_points: u32::MAX,
        },
        Beneficiary {
            address: Address::generate(&env),
            basis_points: 1,
        },
    ];

    assert_eq!(
        client.try_update_beneficiaries(&will_id, &owner, &replacement),
        Err(Ok(WillError::InvalidPercentages.into()))
    );
    // The rejected update must not have disturbed the stored list.
    assert_eq!(client.get_will(&will_id).beneficiaries, beneficiaries);
}
