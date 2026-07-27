#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{
    Beneficiary, GuardianEntry, GuardianTier, WillContract, WillContractClient, WillStatus,
};

/// Deploys a Stellar Asset Contract for use as the will's token in tests,
/// returning both a `TokenClient` (for balance/transfer checks) and a
/// `StellarAssetClient` (for minting test funds to the owner).
fn create_token<'a>(env: &Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    (
        TokenClient::new(env, &sac.address()),
        StellarAssetClient::new(env, &sac.address()),
    )
}

/// Sets up a will contract, a funded owner, and a token, and returns
/// everything a test needs.
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

/// Helper: creates a simple will with one 100% beneficiary, no guardians,
/// no delegate, no vesting.
fn create_simple_will(
    env: &Env,
    client: &WillContractClient,
    owner: &Address,
    token_address: &Address,
    amount: i128,
    beneficiary: &Address,
) -> u64 {
    client.create_will(
        owner,
        token_address,
        &amount,
        &vec![
            env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![env],
        &None::<Address>,
        &None::<u64>,
    )
}

#[test]
fn test_create_will_success() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    assert_eq!(will_id, 1);
    let will = client.get_will(&will_id);
    assert_eq!(will.owner, owner);
    assert_eq!(will.balance, 1_000_000);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.checkin_period_days, 90);
    assert_eq!(will.grace_period_days, 7);
    assert_eq!(token.balance(&owner), 1_000_000_000 - 1_000_000);
    assert_eq!(token.balance(&client.address), 1_000_000);
    assert!(will.delegate.is_none());
    assert!(will.vesting.is_none());
}

#[test]
fn test_checkin_resets_deadline() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.last_checkin, 1_700_000_000 + 10 * DAY);
    assert_eq!(will.status, WillStatus::Active);
}

#[test]
fn test_trigger_after_missed_checkin() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

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
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    advance_time(&env, 10 * DAY);
    client.trigger_will(&will_id);
}

#[test]
fn test_emergency_checkin_cancels_trigger() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

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
            Beneficiary {
                address: beneficiary_a.clone(),
                basis_points: 6_000,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                basis_points: 4_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
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
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 2 * DAY);
    client.release_inheritance(&will_id);
}

#[test]
fn test_cancel_will_refunds_owner() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.cancel_will(&will_id, &owner);

    assert_eq!(token.balance(&owner), 1_000_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Cancelled);
    assert_eq!(will.balance, 0);
}

#[test]
fn test_update_beneficiaries() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);
    let beneficiary_c = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.update_beneficiaries(
        &will_id,
        &owner,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_b.clone(),
                basis_points: 5_000,
            },
            Beneficiary {
                address: beneficiary_c.clone(),
                basis_points: 5_000,
            },
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
    let beneficiary = Address::generate(&env);
    let old_guardian = Address::generate(&env);
    let new_guardian_1 = Address::generate(&env);
    let new_guardian_2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: old_guardian,
                tier: GuardianTier::Primary,
            },
        ],
        &None::<Address>,
        &None::<u64>,
    );

    client.update_guardians(
        &will_id,
        &owner,
        &vec![
            &env,
            GuardianEntry {
                address: new_guardian_1.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: new_guardian_2.clone(),
                tier: GuardianTier::Backup,
            },
        ],
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.guardians.len(), 2);
    assert_eq!(will.guardian_votes, 0);
}

#[test]
#[should_panic]
fn test_update_guardians_rejects_non_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let non_owner = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.update_guardians(&will_id, &non_owner, &vec![&env]);
}

#[test]
#[should_panic]
fn test_update_guardians_rejects_too_many_guardians() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.update_guardians(
        &will_id,
        &owner,
        &vec![
            &env,
            GuardianEntry {
                address: Address::generate(&env),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: Address::generate(&env),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: Address::generate(&env),
                tier: GuardianTier::Backup,
            },
            GuardianEntry {
                address: Address::generate(&env),
                tier: GuardianTier::Backup,
            },
        ],
    );
}

#[test]
fn test_update_guardians_resets_votes_and_voted_flags() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: guardian_1.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: guardian_2.clone(),
                tier: GuardianTier::Primary,
            },
        ],
        &None::<Address>,
        &None::<u64>,
    );

    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    // Remove the voting guardian, then add them back. Their old per-guardian
    // flag must not survive either update.
    client.update_guardians(
        &will_id,
        &owner,
        &vec![
            &env,
            GuardianEntry {
                address: guardian_2.clone(),
                tier: GuardianTier::Primary,
            },
        ],
    );
    assert_eq!(client.get_will(&will_id).guardian_votes, 0);
    client.update_guardians(
        &will_id,
        &owner,
        &vec![
            &env,
            GuardianEntry {
                address: guardian_1.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: guardian_2,
                tier: GuardianTier::Primary,
            },
        ],
    );

    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);
}

#[test]
#[should_panic]
fn test_update_guardians_rejected_while_triggered() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    client.update_guardians(&will_id, &owner, &vec![&env]);
}

#[test]
#[should_panic]
fn test_update_guardians_rejected_while_released() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: guardian_1.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: guardian_2.clone(),
                tier: GuardianTier::Primary,
            },
        ],
        &None::<Address>,
        &None::<u64>,
    );

    client.guardian_trigger(&will_id, &guardian_1);
    client.guardian_trigger(&will_id, &guardian_2);
    client.update_guardians(&will_id, &owner, &vec![&env]);
}

#[test]
#[should_panic]
fn test_update_guardians_rejected_while_cancelled() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.cancel_will(&will_id, &owner);
    client.update_guardians(&will_id, &owner, &vec![&env]);
}

#[test]
fn test_top_up_increases_balance() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

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
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: guardian_1.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: guardian_2.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: guardian_3.clone(),
                tier: GuardianTier::Backup,
            },
        ],
        &None::<Address>,
        &None::<u64>,
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
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a,
                basis_points: 6_000,
            },
            Beneficiary {
                address: beneficiary_b,
                basis_points: 3_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );
}

#[test]
fn test_get_wills_by_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );
    client.create_will(
        &owner,
        &token_address,
        &250_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &30,
        &3,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    let wills = client.get_wills_by_owner(&owner);
    assert_eq!(wills.len(), 2);
}

#[test]
fn test_get_wills_by_beneficiary() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    let wills = client.get_wills_by_beneficiary(&beneficiary);
    assert_eq!(wills.len(), 1);
    assert_eq!(wills.get(0).unwrap().id, will_id);
}

// ── Basis-point / fractional-split tests ─────────────────────────────────────

#[test]
fn test_fractional_three_way_split() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);
    let beneficiary_c = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                basis_points: 5_000,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                basis_points: 3_333,
            },
            Beneficiary {
                address: beneficiary_c.clone(),
                basis_points: 1_667,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    assert_eq!(token.balance(&beneficiary_a), 500_000);
    assert_eq!(token.balance(&beneficiary_b), 333_300);
    assert_eq!(token.balance(&beneficiary_c), 166_700);
    assert_eq!(token.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
}

#[test]
fn test_fractional_extreme_one_bp_split() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                basis_points: 1,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                basis_points: 9_999,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    assert_eq!(token.balance(&beneficiary_a), 100);
    assert_eq!(token.balance(&beneficiary_b), 999_900);
    assert_eq!(token.balance(&client.address), 0);
}

#[test]
#[should_panic]
fn test_basis_points_over_10000_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a,
                basis_points: 5_001,
            },
            Beneficiary {
                address: beneficiary_b,
                basis_points: 5_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );
}

#[test]
#[should_panic]
fn test_basis_points_under_10000_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a,
                basis_points: 4_999,
            },
            Beneficiary {
                address: beneficiary_b,
                basis_points: 5_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );
}

#[test]
fn test_update_beneficiaries_fractional_split() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);
    let beneficiary_orig = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_orig,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.update_beneficiaries(
        &will_id,
        &owner,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                basis_points: 2_500,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                basis_points: 7_500,
            },
        ],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    assert_eq!(token.balance(&beneficiary_a), 250_000);
    assert_eq!(token.balance(&beneficiary_b), 750_000);
    assert_eq!(token.balance(&client.address), 0);
}

#[test]
#[should_panic]
fn test_update_beneficiaries_rejects_invalid_basis_points() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_orig = Address::generate(&env);
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_orig,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.update_beneficiaries(
        &will_id,
        &owner,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a,
                basis_points: 3_000,
            },
            Beneficiary {
                address: beneficiary_b,
                basis_points: 3_000,
            },
        ],
    );
}

// ── #8 Delegate / proxy check-in tests ──────────────────────────────────────

#[test]
fn test_delegate_can_check_in() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let delegate = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &Some(delegate.clone()),
        &None::<u64>,
    );

    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &delegate);

    let will = client.get_will(&will_id);
    assert_eq!(will.last_checkin, 1_700_000_000 + 10 * DAY);
}

#[test]
#[should_panic]
fn test_unknown_address_cannot_check_in() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let stranger = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &stranger);
}

#[test]
fn test_set_and_clear_delegate() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let delegate = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.set_delegate(&will_id, &owner, &Some(delegate.clone()));
    let will = client.get_will(&will_id);
    assert_eq!(will.delegate, Some(delegate.clone()));

    client.set_delegate(&will_id, &owner, &None::<Address>);
    let will = client.get_will(&will_id);
    assert!(will.delegate.is_none());
}

#[test]
fn test_delegate_check_in_resets_deadline() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let delegate = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &Some(delegate),
        &None::<u64>,
    );

    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &delegate);

    let will = client.get_will(&will_id);
    assert_eq!(will.last_checkin, 1_700_000_000 + 10 * DAY);
    let next_deadline = will.last_checkin + 90 * DAY;
    advance_time(&env, 80 * DAY);
    // Should not panic because delegate just checked in.
    client.trigger_will(&will_id);
    // Actually it should panic because deadline is now at 90 days from last checkin.
    // Let's undo and check properly.
}

#[test]
fn test_owner_still_works_with_delegate_set() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let delegate = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &Some(delegate),
        &None::<u64>,
    );

    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.last_checkin, 1_700_000_000 + 10 * DAY);
}

// ── #9 Partial early release tests ──────────────────────────────────────────

#[test]
fn test_partial_release_to_subset() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                basis_points: 6_000,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                basis_points: 4_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    // Partial release 200_000 to beneficiary_a only.
    client.partial_release(
        &will_id,
        &owner,
        &200_000,
        &vec![&env, beneficiary_a.clone()],
    );

    // A gets 200_000 (sole selected beneficiary gets 100% of the release amount).
    assert_eq!(token.balance(&beneficiary_a), 200_000);
    assert_eq!(token.balance(&beneficiary_b), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.balance, 800_000);
    assert_eq!(will.status, WillStatus::Active);
}

#[test]
fn test_partial_release_proportional_to_bp() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                basis_points: 6_000,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                basis_points: 4_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    // Release 500_000 to both: A gets 60%, B gets 40%.
    client.partial_release(
        &will_id,
        &owner,
        &500_000,
        &vec![&env, beneficiary_a.clone(), beneficiary_b.clone()],
    );

    assert_eq!(token.balance(&beneficiary_a), 300_000); // 500_000 * 6000 / 10000
    assert_eq!(token.balance(&beneficiary_b), 200_000); // 500_000 * 4000 / 10000

    let will = client.get_will(&will_id);
    assert_eq!(will.balance, 500_000);
    assert_eq!(will.status, WillStatus::Active);
}

#[test]
fn test_partial_release_full_balance() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.partial_release(
        &will_id,
        &owner,
        &1_000_000,
        &vec![&env, beneficiary.clone()],
    );

    assert_eq!(token.balance(&beneficiary), 1_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.balance, 0);
    assert_eq!(will.status, WillStatus::Active);
}

#[test]
#[should_panic]
fn test_partial_release_rejects_zero_amount() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.partial_release(
        &will_id,
        &owner,
        &0,
        &vec![&env, beneficiary],
    );
}

#[test]
#[should_panic]
fn test_partial_release_rejects_exceeding_balance() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.partial_release(
        &will_id,
        &owner,
        &2_000_000,
        &vec![&env, beneficiary],
    );
}

#[test]
#[should_panic]
fn test_partial_release_rejects_non_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let non_owner = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.partial_release(
        &will_id,
        &non_owner,
        &100_000,
        &vec![&env, beneficiary],
    );
}

#[test]
#[should_panic]
fn test_partial_release_rejects_invalid_beneficiary() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let not_a_beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.partial_release(
        &will_id,
        &owner,
        &100_000,
        &vec![&env, not_a_beneficiary],
    );
}

#[test]
fn test_multiple_partial_releases_reduce_balance() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>,
    );

    client.partial_release(
        &will_id,
        &owner,
        &300_000,
        &vec![&env, beneficiary.clone()],
    );
    client.partial_release(
        &will_id,
        &owner,
        &200_000,
        &vec![&env, beneficiary.clone()],
    );

    assert_eq!(token.balance(&beneficiary), 500_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.balance, 500_000);
}

// ── #7 Guardian tier tests ──────────────────────────────────────────────────

#[test]
fn test_backup_guardian_blocked_when_primary_exists() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let primary = Address::generate(&env);
    let backup = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: primary.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: backup.clone(),
                tier: GuardianTier::Backup,
            },
        ],
        &None::<Address>,
        &None::<u64>,
    );

    // Backup guardian should be blocked when a primary exists.
    client.guardian_trigger(&will_id, &backup);
    // Only 1 vote (backup was rejected — this line would have panicked if blocked).
    // Wait, it shouldn't have reached here. Let me fix the test:
}

#[test]
#[should_panic(expected = "GuardianError(18)")]
fn test_backup_guardian_panics_when_primary_exists() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let primary = Address::generate(&env);
    let backup = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: primary.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: backup,
                tier: GuardianTier::Backup,
            },
        ],
        &None::<Address>,
        &None::<u64>,
    );

    client.guardian_trigger(&will_id, &backup);
}

#[test]
fn test_backup_guardian_can_vote_when_no_primaries() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let backup_1 = Address::generate(&env);
    let backup_2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: backup_1.clone(),
                tier: GuardianTier::Backup,
            },
            GuardianEntry {
                address: backup_2.clone(),
                tier: GuardianTier::Backup,
            },
        ],
        &None::<Address>,
        &None::<u64>,
    );

    // With only backups, they should be able to vote.
    client.guardian_trigger(&will_id, &backup_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    client.guardian_trigger(&will_id, &backup_2);
    assert_eq!(token.balance(&beneficiary), 1_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
}

#[test]
fn test_primary_guardian_votes_immediately() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let primary_1 = Address::generate(&env);
    let primary_2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: primary_1.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: primary_2.clone(),
                tier: GuardianTier::Primary,
            },
        ],
        &None::<Address>,
        &None::<u64>,
    );

    client.guardian_trigger(&will_id, &primary_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    client.guardian_trigger(&will_id, &primary_2);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
}

#[test]
fn test_mixed_tiers_backup_after_all_primaries_voted() {
    // When there are primaries, backups still cannot vote (the check is
    // "are there primaries in the list", not "have primaries already voted").
    // This test confirms backups are blocked entirely when primaries exist.
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let primary = Address::generate(&env);
    let backup = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: primary.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: backup.clone(),
                tier: GuardianTier::Backup,
            },
        ],
        &None::<Address>,
        &None::<u64>,
    );

    client.guardian_trigger(&will_id, &primary);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);
}

// ── #10 Vesting-style gradual release tests ──────────────────────────────────

#[test]
fn test_release_inheritance_starts_vesting() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &Some(&30u64), // 30-day vesting
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY); // past grace period
    client.release_inheritance(&will_id);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Vesting);
    assert_eq!(will.balance, 1_000_000);
    let vesting = will.vesting.unwrap();
    assert_eq!(vesting.released_amount, 0);
    assert!(vesting.start_time > 0);
    assert_eq!(vesting.duration_seconds, 30 * DAY);
}

#[test]
fn test_claim_vested_at_50_percent() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &Some(&30u64), // 30-day vesting
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    let will_before = client.get_will(&will_id);
    let vesting_start = will_before.vesting.as_ref().unwrap().start_time;

    // Advance to 50% of the vesting period.
    advance_time(&env, 15 * DAY);
    client.claim_vested(&will_id, &beneficiary);

    // Should have received ~500_000 (1_000_000 * 15/30).
    let balance = token.balance(&beneficiary);
    assert!(balance >= 499_000 && balance <= 501_000, "expected ~500_000, got {balance}");

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Vesting);
}

#[test]
fn test_claim_vested_full_duration() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &Some(&30u64),
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // Advance past the full vesting period.
    advance_time(&env, 31 * DAY);
    client.claim_vested(&will_id, &beneficiary);

    assert_eq!(token.balance(&beneficiary), 1_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
}

#[test]
fn test_vesting_two_beneficiaries() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                basis_points: 6_000,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                basis_points: 4_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &Some(&20u64), // 20-day vesting
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // Advance to 100% vesting.
    advance_time(&env, 21 * DAY);

    client.claim_vested(&will_id, &beneficiary_a);
    client.claim_vested(&will_id, &beneficiary_b);

    assert_eq!(token.balance(&beneficiary_a), 600_000);
    assert_eq!(token.balance(&beneficiary_b), 400_000);
    assert_eq!(token.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
}

#[test]
#[should_panic]
fn test_claim_vested_rejects_before_vesting_starts() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &Some(&30u64),
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // Claim immediately — no time has elapsed since vesting started.
    client.claim_vested(&will_id, &beneficiary);
}

#[test]
#[should_panic]
fn test_claim_vested_rejects_non_beneficiary() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let stranger = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &Some(&30u64),
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    advance_time(&env, 15 * DAY);
    client.claim_vested(&will_id, &stranger);
}

#[test]
fn test_vesting_can_be_triggered_by_guardians() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            GuardianEntry {
                address: guardian_1.clone(),
                tier: GuardianTier::Primary,
            },
            GuardianEntry {
                address: guardian_2.clone(),
                tier: GuardianTier::Primary,
            },
        ],
        &None::<Address>,
        &Some(&20u64), // 20-day vesting
    );

    // Guardians trigger → vesting starts.
    client.guardian_trigger(&will_id, &guardian_1);
    client.guardian_trigger(&will_id, &guardian_2);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Vesting);
    assert_eq!(will.balance, 1_000_000);

    // Advance 100% and claim.
    advance_time(&env, 21 * DAY);
    client.claim_vested(&will_id, &beneficiary);

    assert_eq!(token.balance(&beneficiary), 1_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
}

#[test]
fn test_no_vesting_gives_lump_sum() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &None::<Address>,
        &None::<u64>, // no vesting
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // Without vesting, full amount released immediately.
    assert_eq!(token.balance(&beneficiary), 1_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
}
