#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Bytes, Env,
};

use crate::{Beneficiary, WillContract, WillContractClient, WillStatus};

/// Deploys a Stellar Asset Contract for use as the will's token in tests.
fn create_token<'a>(env: &Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    (
        TokenClient::new(env, &sac.address()),
        StellarAssetClient::new(env, &sac.address()),
    )
}

/// Sets up a will contract, a funded owner, and a token.
fn setup<'a>() -> (
    Env,
    WillContractClient<'a>,
    Address, // owner
    TokenClient<'a>,
    Address, // token address
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let (token_client, token_admin_client) = create_token(&env, &owner);
    token_admin_client.mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (
        env,
        client,
        owner,
        token_client,
        token_admin_client.address.clone(),
    )
}

fn advance_time(env: &Env, seconds: u64) {
    env.ledger().with_mut(|l| {
        l.timestamp += seconds;
    });
}

const DAY: u64 = 86_400;

// ---------------------------------------------------------------------------
// Helpers shared by tests
// ---------------------------------------------------------------------------

/// Creates a basic single-owner, single-beneficiary active will (no confirmation delay).
fn create_basic_will(
    env: &Env,
    client: &WillContractClient,
    owner: &Address,
    token_address: &Address,
    amount: i128,
) -> (u64, Address) {
    let beneficiary = Address::generate(env);
    let will_id = client.create_will(
        owner,
        token_address,
        &amount,
        &vec![
            env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![env],
        &vec![env],
        &1,
        &0, // no confirmation delay
    );
    (will_id, beneficiary)
}

// ---------------------------------------------------------------------------
// Existing baseline tests (adapted for new create_will signature)
// ---------------------------------------------------------------------------

#[test]
fn test_create_will_success() {
    let (env, client, owner, token, token_address) = setup();
    let (will_id, _beneficiary) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    assert_eq!(will_id, 1);
    let will = client.get_will(&will_id);
    assert_eq!(will.owner, owner);
    assert_eq!(will.balance, 1_000_000);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.checkin_period_days, 90);
    assert_eq!(will.grace_period_days, 7);
    assert_eq!(token.balance(&owner), 1_000_000_000 - 1_000_000);
    assert_eq!(token.balance(&client.address), 1_000_000);
}

#[test]
fn test_checkin_resets_deadline() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.last_checkin, 1_700_000_000 + 10 * DAY);
    assert_eq!(will.status, WillStatus::Active);
}

#[test]
fn test_trigger_after_missed_checkin() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Triggered);
    assert!(will.trigger_time.is_some());
}

#[test]
#[should_panic]
fn test_cannot_trigger_before_deadline() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    advance_time(&env, 10 * DAY);
    client.trigger_will(&will_id);
}

#[test]
fn test_emergency_checkin_cancels_trigger() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 2 * DAY);
    client.emergency_checkin(&will_id, &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert!(will.trigger_time.is_none());
    assert_eq!(will.last_checkin, 1_700_000_000 + 91 * DAY + 2 * DAY);
}

#[test]
fn test_release_inheritance_splits_correctly() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary { address: beneficiary_a.clone(), percentage: 60 },
            Beneficiary { address: beneficiary_b.clone(), percentage: 40 },
        ],
        &90,
        &7,
        &vec![&env],
        &vec![&env],
        &1,
        &0,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    assert_eq!(token.balance(&beneficiary_a), 600_000);
    assert_eq!(token.balance(&beneficiary_b), 400_000);
    assert_eq!(token.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
}

#[test]
#[should_panic]
fn test_cannot_release_during_grace_period() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 2 * DAY);
    client.release_inheritance(&will_id);
}

#[test]
fn test_cancel_will_refunds_owner() {
    let (env, client, owner, token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    client.cancel_will(&will_id, &owner);

    assert_eq!(token.balance(&owner), 1_000_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Cancelled);
    assert_eq!(will.balance, 0);
}

#[test]
fn test_update_beneficiaries() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_b = Address::generate(&env);
    let beneficiary_c = Address::generate(&env);

    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    client.update_beneficiaries(
        &will_id,
        &owner,
        &vec![
            &env,
            Beneficiary { address: beneficiary_b.clone(), percentage: 50 },
            Beneficiary { address: beneficiary_c.clone(), percentage: 50 },
        ],
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.beneficiaries.len(), 2);

    let wills_for_b = client.get_wills_by_beneficiary(&beneficiary_b);
    assert_eq!(wills_for_b.len(), 1);
}

#[test]
fn test_update_guardians() {
    let (env, client, owner, _token, token_address) = setup();
    let old_guardian = Address::generate(&env);
    let new_guardian_1 = Address::generate(&env);
    let new_guardian_2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![&env, Beneficiary { address: Address::generate(&env), percentage: 100 }],
        &90, &7,
        &vec![&env, old_guardian],
        &vec![&env], &1, &0,
    );

    client.update_guardians(&will_id, &owner, &vec![&env, new_guardian_1.clone(), new_guardian_2.clone()]);

    let will = client.get_will(&will_id);
    assert_eq!(will.guardians, vec![&env, new_guardian_1, new_guardian_2]);
    assert_eq!(will.guardian_votes, 0);
}

#[test]
#[should_panic]
fn test_update_guardians_rejects_non_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let non_owner = Address::generate(&env);
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);
    client.update_guardians(&will_id, &non_owner, &vec![&env]);
}

#[test]
#[should_panic]
fn test_update_guardians_rejects_too_many_guardians() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);
    client.update_guardians(
        &will_id, &owner,
        &vec![&env,
            Address::generate(&env), Address::generate(&env),
            Address::generate(&env), Address::generate(&env),
        ],
    );
}

#[test]
fn test_update_guardians_resets_votes_and_voted_flags() {
    let (env, client, owner, _token, token_address) = setup();
    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: Address::generate(&env), percentage: 100 }],
        &90, &7,
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
        &vec![&env], &1, &0,
    );

    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    client.update_guardians(&will_id, &owner, &vec![&env, guardian_2.clone()]);
    assert_eq!(client.get_will(&will_id).guardian_votes, 0);
    client.update_guardians(&will_id, &owner, &vec![&env, guardian_1.clone(), guardian_2]);

    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);
}

#[test]
fn test_top_up_increases_balance() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    client.top_up(&will_id, &owner, &500_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.balance, 1_500_000);
}

#[test]
fn test_guardian_trigger_requires_two_votes() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);
    let guardian_3 = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
        &90, &7,
        &vec![&env, guardian_1.clone(), guardian_2.clone(), guardian_3.clone()],
        &vec![&env], &1, &0,
    );

    client.guardian_trigger(&will_id, &guardian_1);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.guardian_votes, 1);
    assert_eq!(token.balance(&beneficiary), 0);

    client.guardian_trigger(&will_id, &guardian_2);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(token.balance(&beneficiary), 1_000_000);
}

#[test]
#[should_panic]
fn test_invalid_percentages_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![
            &env,
            Beneficiary { address: Address::generate(&env), percentage: 60 },
            Beneficiary { address: Address::generate(&env), percentage: 30 },
        ],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );
}

#[test]
fn test_get_wills_by_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
        &owner, &token_address, &500_000,
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );
    client.create_will(
        &owner, &token_address, &250_000,
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &30, &3, &vec![&env], &vec![&env], &1, &0,
    );

    let wills = client.get_wills_by_owner(&owner);
    assert_eq!(wills.len(), 2);
}

#[test]
fn test_get_wills_by_beneficiary() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, beneficiary) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    let wills = client.get_wills_by_beneficiary(&beneficiary);
    assert_eq!(wills.len(), 1);
    assert_eq!(wills.get(0).unwrap().id, will_id);
}

// ---------------------------------------------------------------------------
// Issue #43 — PendingConfirmation / confirm_will tests
// ---------------------------------------------------------------------------

#[test]
fn test_create_will_pending_confirmation_status() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &90, &7, &vec![&env], &vec![&env], &1,
        &(2 * DAY), // 2-day confirmation window
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::PendingConfirmation);
    assert!(will.confirmation_deadline.is_some());
    assert_eq!(
        will.confirmation_deadline.unwrap(),
        1_700_000_000 + 2 * DAY,
    );
}

#[test]
fn test_confirm_will_transitions_to_active() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &90, &7, &vec![&env], &vec![&env], &1,
        &(2 * DAY),
    );

    advance_time(&env, DAY); // still within window
    client.confirm_will(&will_id, &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert!(will.confirmation_deadline.is_none());
    // last_checkin should be updated to confirmation time
    assert_eq!(will.last_checkin, 1_700_000_000 + DAY);
}

#[test]
#[should_panic]
fn test_confirm_will_after_window_expires_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &90, &7, &vec![&env], &vec![&env], &1,
        &(2 * DAY),
    );

    advance_time(&env, 3 * DAY); // past the window
    client.confirm_will(&will_id, &owner);
}

#[test]
fn test_cancel_will_during_pending_window_refunds_owner() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &90, &7, &vec![&env], &vec![&env], &1,
        &(2 * DAY),
    );

    // Cancel before confirming
    client.cancel_will(&will_id, &owner);

    assert_eq!(token.balance(&owner), 1_000_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Cancelled);
    assert_eq!(will.balance, 0);
}

#[test]
#[should_panic]
fn test_checkin_on_pending_will_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &90, &7, &vec![&env], &vec![&env], &1,
        &(2 * DAY),
    );

    // check_in requires Active status — should panic
    client.check_in(&will_id, &owner);
}

#[test]
#[should_panic]
fn test_confirm_will_already_active_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);
    // Will starts Active (no delay) — confirm_will should panic
    client.confirm_will(&will_id, &owner);
}

// ---------------------------------------------------------------------------
// Issue #44 — Multi-sig owner (co_owners + threshold) tests
// ---------------------------------------------------------------------------

#[test]
fn test_create_will_with_co_owners_stored() {
    let (env, client, owner, _token, token_address) = setup();
    let co1 = Address::generate(&env);
    let co2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: Address::generate(&env), percentage: 100 }],
        &90, &7, &vec![&env],
        &vec![&env, co1.clone(), co2.clone()],
        &2, // threshold = 2 out of 3 total owners
        &0,
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.owner_threshold, 2);
    assert_eq!(will.co_owners.len(), 2);
    assert!(will.co_owners.contains(&co1));
    assert!(will.co_owners.contains(&co2));
}

#[test]
#[should_panic]
fn test_create_will_threshold_exceeds_owners_panics() {
    let (env, client, owner, _token, token_address) = setup();

    // Only 1 owner total, threshold = 2 → invalid
    client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: Address::generate(&env), percentage: 100 }],
        &90, &7, &vec![&env],
        &vec![&env], // no co_owners
        &2,          // threshold exceeds total owners
        &0,
    );
}

#[test]
fn test_single_owner_threshold_one_works() {
    // Threshold 1 with no co-owners is the standard single-owner mode.
    let (env, client, owner, token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.owner_threshold, 1);
    assert_eq!(will.co_owners.len(), 0);

    // Normal operations still work.
    client.top_up(&will_id, &owner, &500_000);
    assert_eq!(client.get_will(&will_id).balance, 1_500_000);

    client.cancel_will(&will_id, &owner);
    assert_eq!(token.balance(&owner), 1_000_000_000);
}

#[test]
fn test_co_owner_stored_and_primary_owner_can_operate() {
    // With mock_all_auths the primary owner can still call privileged actions
    // even when co_owners are set — the threshold is met because all auths
    // are mocked.
    let (env, client, owner, _token, token_address) = setup();
    let co1 = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: Address::generate(&env), percentage: 100 }],
        &90, &7, &vec![&env],
        &vec![&env, co1.clone()],
        &1, // threshold 1: any single owner suffices
        &0,
    );

    // check_in by primary owner
    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &owner);
    assert_eq!(client.get_will(&will_id).last_checkin, 1_700_000_000 + 10 * DAY);

    // top_up by primary owner
    client.top_up(&will_id, &owner, &100_000);
    assert_eq!(client.get_will(&will_id).balance, 1_100_000);
}

#[test]
fn test_will_stores_correct_threshold_value() {
    let (env, client, owner, _token, token_address) = setup();
    let co1 = Address::generate(&env);
    let co2 = Address::generate(&env);

    // 3 total owners, threshold = 3
    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: Address::generate(&env), percentage: 100 }],
        &90, &7, &vec![&env],
        &vec![&env, co1, co2],
        &3,
        &0,
    );

    assert_eq!(client.get_will(&will_id).owner_threshold, 3);
}

// ---------------------------------------------------------------------------
// Issue #45 — split_will tests
// ---------------------------------------------------------------------------

#[test]
fn test_split_will_creates_independent_child_will() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![
            &env,
            Beneficiary { address: beneficiary_a.clone(), percentage: 60 },
            Beneficiary { address: beneficiary_b.clone(), percentage: 40 },
        ],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    // Split beneficiary_b (40%) into a child will with 400_000 tokens.
    let child_id = client.split_will(
        &will_id,
        &owner,
        &vec![&env, Beneficiary { address: beneficiary_b.clone(), percentage: 40 }],
        &400_000,
    );

    // Source will should have reduced balance and only beneficiary_a.
    let source = client.get_will(&will_id);
    assert_eq!(source.balance, 600_000);
    assert_eq!(source.beneficiaries.len(), 1);
    assert_eq!(source.beneficiaries.get(0).unwrap().address, beneficiary_a);
    assert_eq!(source.beneficiaries.get(0).unwrap().percentage, 100);

    // Child will is independent and active.
    let child = client.get_will(&child_id);
    assert_eq!(child.balance, 400_000);
    assert_eq!(child.status, WillStatus::Active);
    assert_eq!(child.owner, owner);
    assert_eq!(child.beneficiaries.len(), 1);
    assert_eq!(child.beneficiaries.get(0).unwrap().address, beneficiary_b);
    assert_eq!(child.beneficiaries.get(0).unwrap().percentage, 100);

    // Token balance unchanged in contract (split is internal).
    assert_eq!(token.balance(&client.address), 1_000_000);
}

#[test]
fn test_split_will_child_is_independently_triggerable() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![
            &env,
            Beneficiary { address: beneficiary_a.clone(), percentage: 50 },
            Beneficiary { address: beneficiary_b.clone(), percentage: 50 },
        ],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    let child_id = client.split_will(
        &will_id, &owner,
        &vec![&env, Beneficiary { address: beneficiary_b.clone(), percentage: 50 }],
        &500_000,
    );

    // Trigger and release only the child will.
    advance_time(&env, 91 * DAY);
    client.trigger_will(&child_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&child_id);

    // Child released, source still active.
    assert_eq!(client.get_will(&child_id).status, WillStatus::Released);
    assert_eq!(client.get_will(&will_id).status, WillStatus::Active);
    assert_eq!(token.balance(&beneficiary_b), 500_000);
    assert_eq!(token.balance(&beneficiary_a), 0);
}

#[test]
#[should_panic]
fn test_split_will_amount_exceeds_balance_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, beneficiary) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    // Try to split more than the balance.
    client.split_will(
        &will_id, &owner,
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &2_000_000,
    );
}

#[test]
#[should_panic]
fn test_split_will_empty_beneficiaries_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let (will_id, _) = create_basic_will(&env, &client, &owner, &token_address, 1_000_000);

    client.split_will(&will_id, &owner, &vec![&env], &500_000);
}

#[test]
#[should_panic]
fn test_split_will_all_beneficiaries_leaves_none_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    // Splitting the only beneficiary would leave source will empty.
    client.split_will(
        &will_id, &owner,
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &500_000,
    );
}

#[test]
fn test_split_will_owner_index_includes_child() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![
            &env,
            Beneficiary { address: beneficiary_a, percentage: 50 },
            Beneficiary { address: beneficiary_b.clone(), percentage: 50 },
        ],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    client.split_will(
        &will_id, &owner,
        &vec![&env, Beneficiary { address: beneficiary_b, percentage: 50 }],
        &500_000,
    );

    // Owner should now have 2 wills indexed.
    let owner_wills = client.get_wills_by_owner(&owner);
    assert_eq!(owner_wills.len(), 2);
}

// ---------------------------------------------------------------------------
// Issue #46 — Hashed beneficiary / reveal_and_claim tests
// ---------------------------------------------------------------------------

/// Build a 64-byte pre-image: 32-byte address representation padded + 32-byte salt.
/// In real use the first 32 bytes would be the canonical bytes of the Address;
/// here we just use a deterministic byte pattern for testing.
fn make_preimage(env: &Env, tag: u8) -> Bytes {
    let mut arr = [0u8; 64];
    for (i, b) in arr.iter_mut().enumerate() {
        *b = tag.wrapping_add(i as u8);
    }
    Bytes::from_array(env, &arr)
}

/// Compute SHA-256 of the preimage using the env crypto primitive.
fn sha256_of(env: &Env, preimage: &Bytes) -> Bytes {
    let digest = env.crypto().sha256(preimage);
    Bytes::from_array(env, &digest.to_array())
}

#[test]
fn test_add_hashed_beneficiary_stored() {
    let (env, client, owner, _token, token_address) = setup();

    // Create will where plain beneficiary takes 60%, hashed takes 40%.
    let plain_beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: plain_beneficiary, percentage: 60 }],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    let preimage = make_preimage(&env, 0xAB);
    let commitment = sha256_of(&env, &preimage);

    client.add_hashed_beneficiary(&will_id, &owner, &commitment, &40);

    let will = client.get_will(&will_id);
    assert_eq!(will.hashed_beneficiaries.len(), 1);
    assert_eq!(will.hashed_beneficiaries.get(0).unwrap().percentage, 40);
    assert!(!will.hashed_beneficiaries.get(0).unwrap().claimed);
}

#[test]
fn test_reveal_and_claim_pays_out_correct_amount() {
    let (env, client, owner, token, token_address) = setup();
    let claimant = Address::generate(&env);

    // Plain beneficiary = 60 %, hashed = 40 %
    let plain_beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: plain_beneficiary.clone(), percentage: 60 }],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    let preimage = make_preimage(&env, 0x01);
    let commitment = sha256_of(&env, &preimage);
    client.add_hashed_beneficiary(&will_id, &owner, &commitment, &40);

    // Trigger the will and let the grace period expire.
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);

    // Hashed beneficiary reveals pre-image and claims.
    client.reveal_and_claim(&will_id, &claimant, &preimage);

    assert_eq!(token.balance(&claimant), 400_000);

    // The slot should now be marked claimed.
    let will = client.get_will(&will_id);
    assert!(will.hashed_beneficiaries.get(0).unwrap().claimed);
    // Remaining balance = 600_000 (the plain beneficiary's share hasn't been
    // released yet via release_inheritance).
    assert_eq!(will.balance, 600_000);
}

#[test]
#[should_panic]
fn test_reveal_and_claim_wrong_preimage_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let claimant = Address::generate(&env);

    let plain_beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: plain_beneficiary, percentage: 60 }],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    let correct_preimage = make_preimage(&env, 0x01);
    let commitment = sha256_of(&env, &correct_preimage);
    client.add_hashed_beneficiary(&will_id, &owner, &commitment, &40);

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);

    // Supply the wrong pre-image.
    let wrong_preimage = make_preimage(&env, 0xFF);
    client.reveal_and_claim(&will_id, &claimant, &wrong_preimage);
}

#[test]
#[should_panic]
fn test_reveal_and_claim_double_claim_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let claimant = Address::generate(&env);

    let plain_beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: plain_beneficiary, percentage: 60 }],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    let preimage = make_preimage(&env, 0x02);
    let commitment = sha256_of(&env, &preimage);
    client.add_hashed_beneficiary(&will_id, &owner, &commitment, &40);

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);

    client.reveal_and_claim(&will_id, &claimant, &preimage);
    // Second claim should panic.
    client.reveal_and_claim(&will_id, &claimant, &preimage);
}

#[test]
#[should_panic]
fn test_reveal_and_claim_before_grace_period_panics() {
    let (env, client, owner, _token, token_address) = setup();
    let claimant = Address::generate(&env);

    let plain_beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner, &token_address, &1_000_000,
        &vec![&env, Beneficiary { address: plain_beneficiary, percentage: 60 }],
        &90, &7, &vec![&env], &vec![&env], &1, &0,
    );

    let preimage = make_preimage(&env, 0x03);
    let commitment = sha256_of(&env, &preimage);
    client.add_hashed_beneficiary(&will_id, &owner, &commitment, &40);

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    // Do NOT advance past the grace period — should panic.
    client.reveal_and_claim(&will_id, &claimant, &preimage);
}
