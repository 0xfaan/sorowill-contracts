#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{Beneficiary, GuardianConsent, WillContract, WillContractClient, WillStatus};

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
        &false,
        &None,
    );

    assert_eq!(will_id, 1);
    let will = client.get_will(&will_id);
    assert_eq!(will.owner, owner);
    assert_eq!(will.balance, 1_000_000);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.checkin_period_days, 90);
    assert_eq!(will.grace_period_days, 7);
    assert_eq!(will.pull_distribution, false);
    assert_eq!(will.fallback_beneficiary, None);
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
    );

    client.accept_guardian_role(&will_id, &guardian_1);
    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    // Remove the voting guardian, then add them back. Their old per-guardian
    // flag must not survive either update.
    client.update_guardians(&will_id, &owner, &vec![&env, guardian_2.clone()]);
    assert_eq!(client.get_will(&will_id).guardian_votes, 0);
    client.update_guardians(
        &will_id,
        &owner,
        &vec![&env, guardian_1.clone(), guardian_2],
    );

    client.accept_guardian_role(&will_id, &guardian_1);
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
        &false,
        &None,
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
        &false,
        &None,
    );

    client.accept_guardian_role(&will_id, &guardian_1);
    client.accept_guardian_role(&will_id, &guardian_2);
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
    );

    client.accept_guardian_role(&will_id, &guardian_1);
    client.guardian_trigger(&will_id, &guardian_1);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.guardian_votes, 1);
    assert_eq!(token.balance(&beneficiary), 0);

    client.accept_guardian_role(&will_id, &guardian_2);
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
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
        &false,
        &None,
    );

    let wills = client.get_wills_by_beneficiary(&beneficiary);
    assert_eq!(wills.len(), 1);
    assert_eq!(wills.get(0).unwrap().id, will_id);
}

// ── Basis-point / fractional-split tests ─────────────────────────────────────

/// A three-way split that is only representable with basis points:
///   A: 50.00 % → 5_000 bp
///   B: 33.33 % → 3_333 bp
///   C: 16.67 % → 1_667 bp  (sum = 10_000)
///
/// On a balance of 1_000_000 the expected payouts are:
///   A = 1_000_000 * 5_000 / 10_000 = 500_000
///   B = 1_000_000 * 3_333 / 10_000 = 333_300
///   C = remainder                   = 166_700
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
        &false,
        &None,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    assert_eq!(token.balance(&beneficiary_a), 500_000);
    assert_eq!(token.balance(&beneficiary_b), 333_300);
    // Remainder goes to the last beneficiary so the full balance is drained.
    assert_eq!(token.balance(&beneficiary_c), 166_700);
    assert_eq!(token.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
}

/// Extreme split: 1 bp for A, 9_999 bp for B.
/// On a balance of 1_000_000:
///   A = 1_000_000 * 1 / 10_000 = 100
///   B = remainder               = 999_900
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
        &false,
        &None,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    assert_eq!(token.balance(&beneficiary_a), 100);
    assert_eq!(token.balance(&beneficiary_b), 999_900);
    assert_eq!(token.balance(&client.address), 0);
}

/// Validation must reject a basis-point sum of 10_001 (one over the limit).
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
        &false,
        &None,
    );
}

/// Validation must reject a basis-point sum of 9_999 (one under the limit).
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
        &false,
        &None,
    );
}

/// update_beneficiaries must also enforce the 10_000 bp invariant on the
/// replacement list, and fractional splits set via that path must distribute
/// correctly when inheritance is released.
///
/// Updated split: A = 2_500 bp (25 %), B = 7_500 bp (75 %).
/// On a balance of 1_000_000: A = 250_000, B = 750_000.
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
        &false,
        &None,
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

/// update_beneficiaries must reject a replacement list whose basis points
/// do not sum to exactly 10_000.
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
        &false,
        &None,
    );

    // 3_000 + 3_000 = 6_000 ≠ 10_000 — must panic.
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

// ── Issue #11: Pull-based beneficiary claim tests ────────────────────────────

/// Pull-mode distribute stores claimable shares instead of transferring tokens.
#[test]
fn test_pull_distribution_stores_shares() {
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
        &true,
        &None,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // Tokens stay in the contract — not transferred to beneficiaries.
    assert_eq!(token.balance(&beneficiary_a), 0);
    assert_eq!(token.balance(&beneficiary_b), 0);
    assert_eq!(token.balance(&client.address), 1_000_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
}

/// Beneficiaries can independently claim their shares.
#[test]
fn test_claim_share_transfers_tokens() {
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
        &true,
        &None,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // Beneficiary A claims first.
    client.claim_share(&will_id, &beneficiary_a);
    assert_eq!(token.balance(&beneficiary_a), 600_000);
    assert_eq!(token.balance(&client.address), 400_000);

    // Beneficiary B claims second.
    client.claim_share(&will_id, &beneficiary_b);
    assert_eq!(token.balance(&beneficiary_b), 400_000);
    assert_eq!(token.balance(&client.address), 0);
}

/// A non-beneficiary cannot claim a share.
#[test]
#[should_panic]
fn test_claim_share_rejects_non_beneficiary() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let non_beneficiary = Address::generate(&env);

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
        &true,
        &None,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    client.claim_share(&will_id, &non_beneficiary);
}

/// A beneficiary cannot claim twice.
#[test]
#[should_panic]
fn test_claim_share_rejects_already_claimed() {
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
        &true,
        &None,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    client.claim_share(&will_id, &beneficiary);
    client.claim_share(&will_id, &beneficiary);
}

/// Pull-mode with three-way fractional split: each beneficiary claims independently.
#[test]
fn test_pull_distribution_fractional_split() {
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
        &true,
        &None,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // All tokens stay in the contract.
    assert_eq!(token.balance(&client.address), 1_000_000);

    // Each beneficiary claims their share.
    client.claim_share(&will_id, &beneficiary_a);
    assert_eq!(token.balance(&beneficiary_a), 500_000);

    client.claim_share(&will_id, &beneficiary_b);
    assert_eq!(token.balance(&beneficiary_b), 333_300);

    client.claim_share(&will_id, &beneficiary_c);
    assert_eq!(token.balance(&beneficiary_c), 166_700);

    assert_eq!(token.balance(&client.address), 0);
}

/// Guardian trigger also works with pull-mode distribution.
#[test]
fn test_guardian_trigger_pull_distribution() {
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
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
        &true,
        &None,
    );

    client.accept_guardian_role(&will_id, &guardian_1);
    client.accept_guardian_role(&will_id, &guardian_2);
    client.guardian_trigger(&will_id, &guardian_1);
    client.guardian_trigger(&will_id, &guardian_2);

    // Tokens stay in contract (pull mode).
    assert_eq!(token.balance(&beneficiary), 0);
    assert_eq!(token.balance(&client.address), 1_000_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);

    // Beneficiary can claim.
    client.claim_share(&will_id, &beneficiary);
    assert_eq!(token.balance(&beneficiary), 1_000_000);
    assert_eq!(token.balance(&client.address), 0);
}

// ── Issue #12: Fallback beneficiary tests ────────────────────────────────────

/// Owner can set and read the fallback beneficiary.
#[test]
fn test_set_fallback_beneficiary() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let fallback = Address::generate(&env);

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
        &false,
        &None,
    );

    assert_eq!(client.get_fallback_beneficiary(&will_id), None);

    client.set_fallback_beneficiary(&will_id, &owner, &Some(fallback.clone()));
    assert_eq!(client.get_fallback_beneficiary(&will_id), Some(fallback.clone()));

    client.set_fallback_beneficiary(&will_id, &owner, &None);
    assert_eq!(client.get_fallback_beneficiary(&will_id), None);
}

/// Fallback can be set at will creation.
#[test]
fn test_create_will_with_fallback() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let fallback = Address::generate(&env);

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
        &false,
        &Some(fallback.clone()),
    );

    assert_eq!(client.get_fallback_beneficiary(&will_id), Some(fallback));
    let will = client.get_will(&will_id);
    assert_eq!(will.fallback_beneficiary, Some(fallback));
}

// ── Issue #13: Guardian consent flow tests ───────────────────────────────────

/// Guardian must accept before voting.
#[test]
fn test_guardian_must_accept_before_voting() {
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
        &false,
        &None,
    );

    // Guardian 1 tries to vote without accepting — must panic.
    let result = client.try_guardian_trigger(&will_id, &guardian_1);
    assert!(result.is_err());

    // Guardian 1 accepts, then votes successfully.
    client.accept_guardian_role(&will_id, &guardian_1);
    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);
}

/// Guardian can accept their role.
#[test]
fn test_accept_guardian_role() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian = Address::generate(&env);

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
        &vec![&env, guardian.clone()],
        &false,
        &None,
    );

    client.accept_guardian_role(&will_id, &guardian);

    // Accepting again must panic.
    let result = client.try_accept_guardian_role(&will_id, &guardian);
    assert!(result.is_err());
}

/// Guardian trigger rejects unaccepted guardian.
#[test]
#[should_panic]
fn test_guardian_trigger_rejects_unaccepted() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian = Address::generate(&env);

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
        &vec![&env, guardian.clone()],
        &false,
        &None,
    );

    // Must panic because guardian has not accepted.
    client.guardian_trigger(&will_id, &guardian);
}

/// Non-guardian cannot accept guardian role.
#[test]
#[should_panic]
fn test_accept_guardian_role_rejects_non_guardian() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let non_guardian = Address::generate(&env);

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
        &false,
        &None,
    );

    client.accept_guardian_role(&will_id, &non_guardian);
}

/// update_guardians resets consent for old guardians and sets Pending for new ones.
#[test]
fn test_update_guardians_resets_consent() {
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
                address: beneficiary,
                basis_points: 10_000,
            },
        ],
        &90,
        &7,
        &vec![&env, guardian_1.clone()],
        &false,
        &None,
    );

    // Guardian 1 accepts.
    client.accept_guardian_role(&will_id, &guardian_1);

    // Replace with guardian_2 — guardian_1's consent should be cleared.
    client.update_guardians(&will_id, &owner, &vec![&env, guardian_2.clone()]);

    // Guardian 1 is no longer a guardian, so accepting must fail.
    let result = client.try_accept_guardian_role(&will_id, &guardian_1);
    assert!(result.is_err());

    // Guardian 2 is named but has not accepted, so voting must fail.
    let result = client.try_guardian_trigger(&will_id, &guardian_2);
    assert!(result.is_err());

    // Guardian 2 accepts, then votes successfully.
    client.accept_guardian_role(&will_id, &guardian_2);
    client.guardian_trigger(&will_id, &guardian_2);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

    // Add guardian_3 alongside guardian_2 — guardian_2's consent should persist.
    client.update_guardians(
        &will_id,
        &owner,
        &vec![&env, guardian_2.clone(), guardian_3.clone()],
    );

    // Guardian 2 should still be accepted (consent was not cleared for them
    // because they were re-added). Actually, update_guardians clears ALL old
    // consents and resets new ones to Pending. So guardian_2 must re-accept.
    let result = client.try_guardian_trigger(&will_id, &guardian_2);
    assert!(result.is_err());
}

// ── Issue #14: Guardian self-initiated replacement request tests ──────────────

/// Guardian can request replacement.
#[test]
fn test_request_guardian_replacement() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian = Address::generate(&env);

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
        &vec![&env, guardian.clone()],
        &false,
        &None,
    );

    client.request_guardian_replacement(&will_id, &guardian);
}

/// Non-guardian cannot request replacement.
#[test]
#[should_panic]
fn test_request_guardian_replacement_rejects_non_guardian() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let non_guardian = Address::generate(&env);

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
        &false,
        &None,
    );

    client.request_guardian_replacement(&will_id, &non_guardian);
}

/// Cannot request replacement on a non-Active will.
#[test]
#[should_panic]
fn test_request_guardian_replacement_rejected_while_triggered() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian = Address::generate(&env);

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
        &vec![&env, guardian.clone()],
        &false,
        &None,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    client.request_guardian_replacement(&will_id, &guardian);
}
