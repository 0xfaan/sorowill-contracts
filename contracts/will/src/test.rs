#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{
    Beneficiary, GraceTier, GuardianVoteReason, WillContract, WillContractClient, WillStatus,
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

// ── Existing tests (updated for new create_will signature) ─────────────────

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
        &0,
        &vec![&env],
    );

    assert_eq!(will_id, 1);
    let will = client.get_will(&will_id);
    assert_eq!(will.owner, owner);
    assert_eq!(will.balance, 1_000_000);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.checkin_period_days, 90);
    assert_eq!(will.grace_period_days, 7);
    assert_eq!(will.guardian_vote_expiry_days, 7);
    assert_eq!(will.grace_tiers.len(), 0);
    assert_eq!(will.released_basis_points, 0);
    assert_eq!(token.balance(&owner), 1_000_000_000 - 1_000_000);
    assert_eq!(token.balance(&client.address), 1_000_000);
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Triggered);
    assert!(will.trigger_time.is_some());
    assert_eq!(will.trigger_balance, 1_000_000);
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &vec![&env, old_guardian],
        &0,
        &vec![&env],
    );

    client.update_guardians(
        &will_id,
        &owner,
        &vec![&env, new_guardian_1.clone(), new_guardian_2.clone()],
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.guardians, vec![&env, new_guardian_1, new_guardian_2]);
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
    );

    client.update_guardians(
        &will_id,
        &owner,
        &vec![
            &env,
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
            Address::generate(&env),
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
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
        &0,
        &vec![&env],
    );

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Incapacitated);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    client.update_guardians(&will_id, &owner, &vec![&env, guardian_2.clone()]);
    assert_eq!(client.get_will(&will_id).guardian_votes, 0);
    client.update_guardians(
        &will_id,
        &owner,
        &vec![&env, guardian_1.clone(), guardian_2],
    );

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Incapacitated);
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
        &0,
        &vec![&env],
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
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
        &0,
        &vec![&env],
    );

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Deceased);
    client.guardian_trigger(&will_id, &guardian_2, &GuardianVoteReason::Deceased);
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
    );

    client.top_up(&will_id, &owner, &500_000);

    use soroban_sdk::{testutils::Events, symbol_short, TryIntoVal};
    let events = env.events().all();
    let mut found = false;
    for event in events.iter() {
        if !event.1.is_empty() {
            if let Ok(topic0) = event.1.get(0).unwrap().try_into_val(&env) {
                let topic0_sym: soroban_sdk::Symbol = topic0;
                if topic0_sym == symbol_short!("topup") {
                    found = true;
                    assert_eq!(event.0, client.address.clone());
                    let topic1: u64 = event.1.get(1).unwrap().try_into_val(&env).unwrap();
                    assert_eq!(topic1, will_id);
                    let data: (Address, i128, i128) = event.2.try_into_val(&env).unwrap();
                    assert_eq!(data, (owner.clone(), 500_000_i128, 1_500_000_i128));
                }
            }
        }
    }
    assert!(found, "topup event not found");

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
            guardian_1.clone(),
            guardian_2.clone(),
            guardian_3.clone(),
        ],
        &0,
        &vec![&env],
    );

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Incapacitated);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.guardian_votes, 1);
    assert_eq!(token.balance(&beneficiary), 0);

    client.guardian_trigger(&will_id, &guardian_2, &GuardianVoteReason::Incapacitated);
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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
        &0,
        &vec![&env],
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

// ── #15: Time-weighted guardian vote expiry tests ────────────────────────────

#[test]
fn test_guardian_vote_expiry_defaults_to_grace_period() {
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
        &0,
        &vec![&env],
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.guardian_vote_expiry_days, 7);
}

#[test]
fn test_guardian_vote_expiry_custom_value() {
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
        &3,
        &vec![&env],
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.guardian_vote_expiry_days, 3);
}

#[test]
fn test_expired_guardian_vote_does_not_count() {
    let (env, client, owner, _token, token_address) = setup();
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
        &vec![&env, guardian_1.clone(), guardian_2.clone(), guardian_3.clone()],
        &2,
        &vec![&env],
    );

    guardian_1 votes, then 3 days pass (beyond 2-day expiry), then guardian_2 votes.
    guardian_1's vote should be expired, so only 1 valid vote — no release.
    guardian_3 can then vote to reach 2 valid votes and trigger release.

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Incapacitated);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    advance_time(&env, 3 * DAY);

    client.guardian_trigger(&will_id, &guardian_2, &GuardianVoteReason::Deceased);
    let will = client.get_will(&will_id);
    assert_eq!(will.guardian_votes, 1);
    assert_eq!(will.status, WillStatus::Active);

    client.guardian_trigger(&will_id, &guardian_3, &GuardianVoteReason::Unreachable);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(token.balance(&beneficiary), 1_000_000);
}

#[test]
fn test_same_guardian_cannot_revote_before_expiry() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);

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
        &vec![&env, guardian_1.clone()],
        &5,
        &vec![&env],
    );

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Other);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    advance_time(&env, 1 * DAY);

    let result = client.try_guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Other);
    assert!(result.is_err());
}

#[test]
fn test_same_guardian_can_revote_after_expiry() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);

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
        &vec![&env, guardian_1.clone()],
        &2,
        &vec![&env],
    );

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Incapacitated);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    advance_time(&env, 3 * DAY);

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Incapacitated);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);
}

// ── #17: Guardian vote reason code tests ─────────────────────────────────────

#[test]
fn test_guardian_vote_reason_stored_and_emitted() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);

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
        &vec![&env, guardian_1.clone()],
        &0,
        &vec![&env],
    );

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Deceased);

    use soroban_sdk::testutils::Events;
    let events = env.events().all();
    let mut found_gvote = false;
    for event in events.iter() {
        if !event.1.is_empty() {
            if let Ok(topic0) = event.1.get(0).unwrap().try_into_val(&env) {
                let topic0_sym: soroban_sdk::Symbol = topic0;
                if topic0_sym == soroban_sdk::symbol_short!("gvote") {
                    found_gvote = true;
                    let data: (Address, u32, GuardianVoteReason) =
                        event.2.try_into_val(&env).unwrap();
                    assert_eq!(data.0, guardian_1);
                    assert_eq!(data.1, 1);
                    assert_eq!(data.2, GuardianVoteReason::Deceased);
                }
            }
        }
    }
    assert!(found_gvote, "gvote event not found");
}

#[test]
fn test_all_guardian_reason_codes() {
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
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
        &0,
        &vec![&env],
    );

    client.guardian_trigger(&will_id, &guardian_1, &GuardianVoteReason::Incapacitated);
    client.guardian_trigger(&will_id, &guardian_2, &GuardianVoteReason::Other);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
}

// ── #16: Multi-tier grace period tests ──────────────────────────────────────

#[test]
fn test_create_will_with_grace_tiers() {
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
        &14,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 5_000,
            },
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 5_000,
            },
        ],
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.grace_tiers.len(), 2);
    assert_eq!(will.released_basis_points, 0);
}

#[test]
fn test_release_tier_first_milestone() {
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
        &14,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 5_000,
            },
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 5_000,
            },
        ],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    advance_time(&env, 8 * DAY);
    client.release_tier(&will_id, &0);

    assert_eq!(token.balance(&beneficiary_a), 300_000);
    assert_eq!(token.balance(&beneficiary_b), 200_000);
    assert_eq!(token.balance(&client.address), 500_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.balance, 500_000);
    assert_eq!(will.released_basis_points, 1);
    assert_eq!(will.status, WillStatus::Triggered);
}

#[test]
fn test_release_tier_both_milestones() {
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
        &14,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 3_000,
            },
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 7_000,
            },
        ],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    advance_time(&env, 8 * DAY);
    client.release_tier(&will_id, &0);
    assert_eq!(token.balance(&beneficiary), 300_000);

    advance_time(&env, 7 * DAY);
    client.release_tier(&will_id, &1);
    assert_eq!(token.balance(&beneficiary), 1_000_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
}

#[test]
#[should_panic]
fn test_release_tier_before_deadline() {
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
        &14,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 5_000,
            },
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 5_000,
            },
        ],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    advance_time(&env, 3 * DAY);
    client.release_tier(&will_id, &0);
}

#[test]
#[should_panic]
fn test_release_tier_already_released() {
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
        &14,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 5_000,
            },
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 5_000,
            },
        ],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_tier(&will_id, &0);
    client.release_tier(&will_id, &0);
}

#[test]
#[should_panic]
fn test_release_tier_out_of_range() {
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
        &14,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 5_000,
            },
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 5_000,
            },
        ],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_tier(&will_id, &5);
}

#[test]
#[should_panic]
fn test_release_tier_no_grace_tiers() {
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
        &0,
        &vec![&env],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_tier(&will_id, &0);
}

#[test]
#[should_panic]
fn test_invalid_grace_tiers_bp_not_10000() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
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
        &14,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 4_000,
            },
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 5_000,
            },
        ],
    );
}

#[test]
#[should_panic]
fn test_invalid_grace_tiers_not_ascending() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
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
        &14,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 5_000,
            },
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 5_000,
            },
        ],
    );
}

#[test]
#[should_panic]
fn test_invalid_grace_tiers_beyond_grace_period() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
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
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 7 * DAY,
                basis_points: 5_000,
            },
            GraceTier {
                day_offset: 14 * DAY,
                basis_points: 5_000,
            },
        ],
    );
}

#[test]
fn test_release_inheritance_still_works_with_empty_tiers() {
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
        &0,
        &vec![&env],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    assert_eq!(token.balance(&beneficiary), 1_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
}

#[test]
fn test_grace_tiers_three_way_split() {
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
        &30,
        &vec![&env],
        &0,
        &vec![
            &env,
            GraceTier {
                day_offset: 10 * DAY,
                basis_points: 2_000,
            },
            GraceTier {
                day_offset: 20 * DAY,
                basis_points: 3_000,
            },
            GraceTier {
                day_offset: 30 * DAY,
                basis_points: 5_000,
            },
        ],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    advance_time(&env, 11 * DAY);
    client.release_tier(&will_id, &0);
    assert_eq!(token.balance(&beneficiary_a), 120_000);
    assert_eq!(token.balance(&beneficiary_b), 80_000);
    assert_eq!(token.balance(&client.address), 800_000);

    advance_time(&env, 10 * DAY);
    client.release_tier(&will_id, &1);
    assert_eq!(token.balance(&beneficiary_a), 300_000);
    assert_eq!(token.balance(&beneficiary_b), 200_000);
    assert_eq!(token.balance(&client.address), 500_000);

    advance_time(&env, 10 * DAY);
    client.release_tier(&will_id, &2);
    assert_eq!(token.balance(&beneficiary_a), 600_000);
    assert_eq!(token.balance(&beneficiary_b), 400_000);
    assert_eq!(token.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
}

// ── #20: Batch check-in tests ───────────────────────────────────────────────

#[test]
fn test_batch_checkin_multiple_wills() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id_1 = client.create_will(
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
        &0,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &300_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                basis_points: 10_000,
            },
        ],
        &30,
        &5,
        &vec![&env],
        &0,
        &vec![&env],
    );

    advance_time(&env, 10 * DAY);

    client.batch_check_in(
        &vec![&env, will_id_1, will_id_2],
        &owner,
    );

    let will_1 = client.get_will(&will_id_1);
    assert_eq!(will_1.last_checkin, 1_700_000_000 + 10 * DAY);

    let will_2 = client.get_will(&will_id_2);
    assert_eq!(will_2.last_checkin, 1_700_000_000 + 10 * DAY);
}

#[test]
fn test_batch_checkin_single_will() {
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
        &0,
        &vec![&env],
    );

    advance_time(&env, 5 * DAY);

    client.batch_check_in(&vec![&env, will_id], &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.last_checkin, 1_700_000_000 + 5 * DAY);
}

#[test]
#[should_panic]
fn test_batch_checkin_rejects_non_active_will() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id_1 = client.create_will(
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
        &0,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &300_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &30,
        &5,
        &vec![&env],
        &0,
        &vec![&env],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id_2);

    advance_time(&env, 1 * DAY);
    client.batch_check_in(&vec![&env, will_id_1, will_id_2], &owner);
}

#[test]
fn test_batch_checkin_emits_event() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id_1 = client.create_will(
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
        &0,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
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
        &60,
        &5,
        &vec![&env],
        &0,
        &vec![&env],
    );

    use soroban_sdk::{testutils::Events, symbol_short, TryIntoVal};
    advance_time(&env, 5 * DAY);
    client.batch_check_in(&vec![&env, will_id_1, will_id_2], &owner);

    let events = env.events().all();
    let mut found_batch = false;
    for event in events.iter() {
        if !event.1.is_empty() {
            if let Ok(topic0) = event.1.get(0).unwrap().try_into_val(&env) {
                let topic0_sym: soroban_sdk::Symbol = topic0;
                if topic0_sym == symbol_short!("batchck") {
                    found_batch = true;
                }
            }
        }
    }
    assert!(found_batch, "batch checkin event not found");
}
