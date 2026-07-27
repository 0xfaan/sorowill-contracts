#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{Beneficiary, Guardian, WillContract, WillContractClient, WillStatus};

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
        &vec![
            &env,
            Guardian {
                address: new_guardian_1.clone(),
                weight: 1,
            },
            Guardian {
                address: new_guardian_2.clone(),
                weight: 1,
            },
        ],
    );

    let will = client.get_will(&will_id);
    assert_eq!(
        will.guardians,
        vec![
            &env,
            Guardian {
                address: new_guardian_1,
                weight: 1,
            },
            Guardian {
                address: new_guardian_2,
                weight: 1,
            }
        ]
    );
    assert_eq!(will.guardian_vote_weight, 0);
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
        &vec![
            &env,
            Guardian {
                address: guardian_1.clone(),
                weight: 1,
            },
            Guardian {
                address: guardian_2.clone(),
                weight: 1,
            },
        ],
    );

    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_vote_weight, 1);

    // Remove the voting guardian, then add them back. Their old per-guardian
    // flag must not survive either update.
    client.update_guardians(
        &will_id,
        &owner,
        &vec![
            &env,
            Guardian {
                address: guardian_2.clone(),
                weight: 1,
            },
        ],
    );
    assert_eq!(client.get_will(&will_id).guardian_vote_weight, 0);
    client.update_guardians(
        &will_id,
        &owner,
        &vec![
            &env,
            Guardian {
                address: guardian_1.clone(),
                weight: 1,
            },
            Guardian {
                address: guardian_2,
                weight: 1,
            },
        ],
    );

    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_vote_weight, 1);
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
        &vec![
            &env,
            Guardian {
                address: guardian_1.clone(),
                weight: 1,
            },
            Guardian {
                address: guardian_2.clone(),
                weight: 1,
            },
        ],
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
            Guardian {
                address: guardian_1.clone(),
                weight: 1,
            },
            Guardian {
                address: guardian_2.clone(),
                weight: 1,
            },
            Guardian {
                address: guardian_3.clone(),
                weight: 1,
            },
        ],
    );

    client.guardian_trigger(&will_id, &guardian_1);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.guardian_vote_weight, 1);
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
fn test_weighted_guardian_single_high_weight_triggers() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_heavy = Address::generate(&env);
    let guardian_light = Address::generate(&env);

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
            Guardian {
                address: guardian_heavy.clone(),
                weight: 3,
            },
            Guardian {
                address: guardian_light.clone(),
                weight: 1,
            },
        ],
    );

    client.guardian_trigger(&will_id, &guardian_heavy);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.guardian_vote_weight, 3);
    assert_eq!(token.balance(&beneficiary), 1_000_000);
}

#[test]
fn test_weighted_guardian_insufficient_weight_stays_active() {
    let (env, client, _owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_light = Address::generate(&env);

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
        &vec![
            &env,
            Guardian {
                address: guardian_light.clone(),
                weight: 1,
            },
        ],
    );

    client.guardian_trigger(&will_id, &guardian_light);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.guardian_vote_weight, 1);
}

#[test]
fn test_weighted_guardian_combined_votes() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let guardian_a = Address::generate(&env);
    let guardian_b = Address::generate(&env);

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
            Guardian {
                address: guardian_a.clone(),
                weight: 1,
            },
            Guardian {
                address: guardian_b.clone(),
                weight: 1,
            },
        ],
    );

    client.guardian_trigger(&will_id, &guardian_a);
    assert_eq!(client.get_will(&will_id).guardian_vote_weight, 1);

    client.guardian_trigger(&will_id, &guardian_b);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.guardian_vote_weight, 2);
    assert_eq!(token.balance(&beneficiary), 1_000_000);
}

#[test]
fn test_get_wills_by_owner_and_status() {
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
        &250_000,
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

    let active_wills = client.get_wills_by_owner_and_status(&owner, &WillStatus::Active);
    assert_eq!(active_wills.len(), 2);

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id_1);

    let active_wills = client.get_wills_by_owner_and_status(&owner, &WillStatus::Active);
    assert_eq!(active_wills.len(), 1);
    assert_eq!(active_wills.get(0).unwrap().id, will_id_2);

    let triggered_wills = client.get_wills_by_owner_and_status(&owner, &WillStatus::Triggered);
    assert_eq!(triggered_wills.len(), 1);
    assert_eq!(triggered_wills.get(0).unwrap().id, will_id_1);

    let released_wills = client.get_wills_by_owner_and_status(&owner, &WillStatus::Released);
    assert_eq!(released_wills.len(), 0);
}

#[test]
fn test_close_will_marks_settled() {
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

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(token.balance(&beneficiary), 1_000_000);

    client.close_will(&will_id, &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Settled);
}

#[test]
#[should_panic]
fn test_close_will_requires_released_status() {
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

    client.close_will(&will_id, &owner);
}

#[test]
#[should_panic]
fn test_close_will_requires_owner() {
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

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    client.close_will(&will_id, &non_owner);
}

#[test]
fn test_release_event_includes_per_beneficiary_breakdown() {
    let (env, client, owner, _token, token_address) = setup();
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

    use soroban_sdk::{symbol_short, testutils::Events, TryIntoVal};
    let events = env.events().all();
    let mut found = false;
    for event in events.iter() {
        if !event.1.is_empty() {
            if let Ok(topic0) = event.1.get(0).unwrap().try_into_val(&env) {
                let topic0_sym: soroban_sdk::Symbol = topic0;
                if topic0_sym == symbol_short!("released") {
                    found = true;
                    let topic1: u64 = event.1.get(1).unwrap().try_into_val(&env).unwrap();
                    assert_eq!(topic1, will_id);

                    let data: (i128, bool, soroban_sdk::Vec<(Address, u32, i128)>) =
                        event.2.try_into_val(&env).unwrap();
                    assert_eq!(data.0, 1_000_000_i128);
                    assert!(!data.1, "should not be guardian-triggered");
                    assert_eq!(data.2.len(), 2);

                    let first = data.2.get(0).unwrap();
                    assert_eq!(first.0, beneficiary_a);
                    assert_eq!(first.1, 60);
                    assert_eq!(first.2, 600_000);

                    let second = data.2.get(1).unwrap();
                    assert_eq!(second.0, beneficiary_b);
                    assert_eq!(second.1, 40);
                    assert_eq!(second.2, 400_000);
                }
            }
        }
    }
    assert!(found, "released event not found");
}

#[test]
fn test_guardian_release_event_is_guardian_triggered() {
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
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![
            &env,
            Guardian {
                address: guardian_1.clone(),
                weight: 1,
            },
            Guardian {
                address: guardian_2.clone(),
                weight: 1,
            },
        ],
    );

    client.guardian_trigger(&will_id, &guardian_1);
    client.guardian_trigger(&will_id, &guardian_2);

    use soroban_sdk::{symbol_short, testutils::Events, TryIntoVal};
    let events = env.events().all();
    let mut found = false;
    for event in events.iter() {
        if !event.1.is_empty() {
            if let Ok(topic0) = event.1.get(0).unwrap().try_into_val(&env) {
                let topic0_sym: soroban_sdk::Symbol = topic0;
                if topic0_sym == symbol_short!("released") {
                    found = true;
                    let data: (i128, bool, soroban_sdk::Vec<(Address, u32, i128)>) =
                        event.2.try_into_val(&env).unwrap();
                    assert!(data.1, "should be guardian-triggered");
                    assert_eq!(data.0, 1_000_000_i128);
                    assert_eq!(data.2.len(), 1);
                    let entry = data.2.get(0).unwrap();
                    assert_eq!(entry.0, beneficiary);
                    assert_eq!(entry.1, 100);
                    assert_eq!(entry.2, 1_000_000);
                }
            }
        }
    }
    assert!(found, "released event not found");
}
