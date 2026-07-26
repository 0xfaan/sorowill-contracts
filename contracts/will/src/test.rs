#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{Beneficiary, WillContract, WillContractClient, WillStatus};

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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 60,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                percentage: 40,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    client.update_beneficiaries(
        &will_id,
        &owner,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_b.clone(),
                percentage: 50,
            },
            Beneficiary {
                address: beneficiary_c.clone(),
                percentage: 50,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env, old_guardian],
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
    );

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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
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
                percentage: 60,
            },
            Beneficiary {
                address: beneficiary_b,
                percentage: 30,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
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
                percentage: 100,
            },
        ],
        &30,
        &3,
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let wills = client.get_wills_by_beneficiary(&beneficiary);
    assert_eq!(wills.len(), 1);
    assert_eq!(wills.get(0).unwrap().id, will_id);
}


#[test]
fn test_merge_wills_basic() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &600_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &400_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_b.clone(),
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    client.merge_wills(&owner, &will_id_1, &will_id_2);

    let merged_will = client.get_will(&will_id_1);
    assert_eq!(merged_will.balance, 1_000_000);
    assert_eq!(merged_will.beneficiaries.len(), 2);
    assert_eq!(merged_will.status, WillStatus::Active);
    // Check-in period should be minimum (30 days)
    assert_eq!(merged_will.checkin_period_days, 30);
    // Grace period should be maximum (7 days)
    assert_eq!(merged_will.grace_period_days, 7);

    let consumed_will = client.get_will(&will_id_2);
    assert_eq!(consumed_will.balance, 0);
    assert_eq!(consumed_will.status, WillStatus::Cancelled);
}

#[test]
fn test_merge_wills_matching_beneficiaries() {
    let (env, client, owner, _token, token_address) = setup();
    let shared_beneficiary = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &600_000,
        &vec![
            &env,
            Beneficiary {
                address: shared_beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &400_000,
        &vec![
            &env,
            Beneficiary {
                address: shared_beneficiary.clone(),
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    client.merge_wills(&owner, &will_id_1, &will_id_2);

    let merged_will = client.get_will(&will_id_1);
    assert_eq!(merged_will.balance, 1_000_000);
    // Should merge into a single beneficiary entry
    assert_eq!(merged_will.beneficiaries.len(), 1);
    assert_eq!(
        merged_will.beneficiaries.get(0).unwrap().percentage,
        100
    );
}

#[test]
fn test_merge_wills_with_guardians() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);
    let guardian_3 = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env, guardian_2.clone(), guardian_3.clone()],
    );

    client.merge_wills(&owner, &will_id_1, &will_id_2);

    let merged_will = client.get_will(&will_id_1);
    assert_eq!(merged_will.guardians.len(), 3);
    assert!(merged_will.guardians.contains(&guardian_1));
    assert!(merged_will.guardians.contains(&guardian_2));
    assert!(merged_will.guardians.contains(&guardian_3));
}

#[test]
#[should_panic]
fn test_merge_wills_same_will_id() {
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    client.merge_wills(&owner, &will_id, &will_id);
}

#[test]
#[should_panic]
fn test_merge_wills_not_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let not_owner = Address::generate(&env);
    let beneficiary = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    client.merge_wills(&not_owner, &will_id_1, &will_id_2);
}

#[test]
#[should_panic]
fn test_merge_wills_first_not_active() {
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    // Cancel the first will
    client.cancel_will(&will_id_1, &owner);

    client.merge_wills(&owner, &will_id_1, &will_id_2);
}

#[test]
#[should_panic]
fn test_merge_wills_second_not_active() {
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
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    // Cancel the second will
    client.cancel_will(&will_id_2, &owner);

    client.merge_wills(&owner, &will_id_1, &will_id_2);
}

#[test]
fn test_merge_wills_recalculates_percentages() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_b.clone(),
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    client.merge_wills(&owner, &will_id_1, &will_id_2);

    let merged_will = client.get_will(&will_id_1);
    assert_eq!(merged_will.balance, 2_000_000);
    assert_eq!(merged_will.beneficiaries.len(), 2);

    let mut total_percentage: u32 = 0;
    for beneficiary in merged_will.beneficiaries.iter() {
        total_percentage += beneficiary.percentage;
    }
    assert_eq!(total_percentage, 100);
}

#[test]
fn test_merge_wills_complex_beneficiaries() {
    let (env, client, owner, _token, token_address) = setup();
    let benef_a = Address::generate(&env);
    let benef_b = Address::generate(&env);
    let benef_c = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: benef_a.clone(),
                percentage: 60,
            },
            Beneficiary {
                address: benef_b.clone(),
                percentage: 40,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &vec![
            &env,
            Beneficiary {
                address: benef_b.clone(),
                percentage: 50,
            },
            Beneficiary {
                address: benef_c.clone(),
                percentage: 50,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    client.merge_wills(&owner, &will_id_1, &will_id_2);

    let merged_will = client.get_will(&will_id_1);
    assert_eq!(merged_will.balance, 2_000_000);
    // Should have 3 beneficiaries total
    assert_eq!(merged_will.beneficiaries.len(), 3);

    let mut total_percentage: u32 = 0;
    for beneficiary in merged_will.beneficiaries.iter() {
        total_percentage += beneficiary.percentage;
    }
    assert_eq!(total_percentage, 100);
}

#[test]
#[should_panic]
fn test_merge_wills_exceeds_beneficiary_limit() {
    let (env, client, owner, _token, token_address) = setup();
    let mut benefs_a = Vec::new(&env);
    let mut benefs_b = Vec::new(&env);

    // Create 6 beneficiaries for will_a
    for i in 0..6 {
        benefs_a.push_back(Beneficiary {
            address: Address::generate(&env),
            percentage: if i == 5 { 34 } else { 11 },
        });
    }

    // Create 5 beneficiaries for will_b
    for i in 0..5 {
        benefs_b.push_back(Beneficiary {
            address: Address::generate(&env),
            percentage: if i == 4 { 20 } else { 20 },
        });
    }

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &benefs_a,
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &1_000_000,
        &benefs_b,
        &30,
        &3,
        &vec![&env],
    );

    // This should panic because it would exceed MAX_BENEFICIARIES (10)
    client.merge_wills(&owner, &will_id_1, &will_id_2);
}

#[test]
#[should_panic]
fn test_merge_wills_exceeds_guardian_limit() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);
    let guardian_3 = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
    );

    // Create a second will with different guardians to exceed the limit
    let g4 = Address::generate(&env);
    let g5 = Address::generate(&env);

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env, g4.clone(), g5.clone()],
    );

    // This should panic because it would exceed MAX_GUARDIANS (3)
    client.merge_wills(&owner, &will_id_1, &will_id_2);
}

#[test]
fn test_merge_wills_preserves_token_address() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &600_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &400_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    client.merge_wills(&owner, &will_id_1, &will_id_2);

    let merged_will = client.get_will(&will_id_1);
    assert_eq!(merged_will.token, token_address);
}

#[test]
fn test_merge_wills_updates_beneficiary_index() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &600_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &400_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_b.clone(),
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env],
    );

    client.merge_wills(&owner, &will_id_1, &will_id_2);

    // Beneficiary B should now be indexed to the merged will
    let wills_for_b = client.get_wills_by_beneficiary(&beneficiary_b);
    assert!(wills_for_b.len() >= 1);

    let mut found = false;
    for will in wills_for_b.iter() {
        if will.id == will_id_1 {
            found = true;
            break;
        }
    }
    assert!(found);
}

#[test]
fn test_merge_wills_clears_guardian_votes() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);
    let guardian_3 = Address::generate(&env);

    let will_id_1 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
    );

    let will_id_2 = client.create_will(
        &owner,
        &token_address,
        &500_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &30,
        &3,
        &vec![&env, guardian_3],
    );

    // Cast a guardian vote on will_id_1
    client.guardian_trigger(&will_id_1, &guardian_1);
    assert_eq!(client.get_will(&will_id_1).guardian_votes, 1);

    // Merge wills - should clear guardian votes
    client.merge_wills(&owner, &will_id_1, &will_id_2);

    let merged_will = client.get_will(&will_id_1);
    assert_eq!(merged_will.guardian_votes, 0);
}
