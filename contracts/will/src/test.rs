                                                                                                                                                                                                                                                                                                                                         #![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env,
};

use crate::{Beneficiary, WillContract, WillContractClient, WillStatus};

// ── helpers ──────────────────────────────────────────────────────────────────

/// Deploys a Stellar Asset Contract, returning a token client and an admin
/// client (for minting in tests).
fn create_token<'a>(env: &Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    (
        TokenClient::new(env, &sac.address()),
        StellarAssetClient::new(env, &sac.address()),
    )
}

/// Basic single-token setup: one owner with 1_000_000_000 units of one token.
fn setup<'a>() -> (
    Env,
    WillContractClient<'a>,
    Address,           // owner
    TokenClient<'a>,   // token_a client
    Address,           // token_a address
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let (token_client, token_admin_client) = create_token(&env, &owner);
    token_admin_client.mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env, client, owner, token_client, token_admin_client.address.clone())
}

/// Two-token setup: owner holds 1_000_000_000 of token_a and token_b.
fn setup_two_tokens<'a>() -> (
    Env,
    WillContractClient<'a>,
    Address,           // owner
    TokenClient<'a>,   // token_a client
    Address,           // token_a address
    TokenClient<'a>,   // token_b client
    Address,           // token_b address
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);

    let (token_a_client, token_a_admin) = create_token(&env, &owner);
    token_a_admin.mint(&owner, &1_000_000_000);
    let token_a_addr = token_a_admin.address.clone();

    // Use a separate admin for token_b to avoid address collisions.
    let token_b_admin_addr = Address::generate(&env);
    let (token_b_client, token_b_admin) = create_token(&env, &token_b_admin_addr);
    token_b_admin.mint(&owner, &1_000_000_000);
    let token_b_addr = token_b_admin.address.clone();

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env, client, owner, token_a_client, token_a_addr, token_b_client, token_b_addr)
}

/// Sets up a will contract and funds the owner with native XLM by
/// transferring from the test environment's source account (which has
/// a large native XLM balance in test mode).
fn setup_native<'a>() -> (
    Env,
    WillContractClient<'a>,
    Address, // owner
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);

    // Capture the test source address *before* registering our contract.
    // In the test environment, env.current_contract_address() returns the
    // default source/invoker account which holds a large native XLM balance.
    let test_source = env.current_contract_address();

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    // Transfer native XLM from the test source account to the owner,
    // giving the owner XLM to use in the will.
    env.transfer(&test_source, &owner, &10_000_000_000_000i128);

    (env, client, owner)
}

fn advance_time(env: &Env, seconds: u64) {
    env.ledger().with_mut(|l| {
        l.timestamp += seconds;
    });
}

const DAY: u64 = 86_400;

// ── Token-based (SAC) tests ────────────────────────────────────────────
// ── existing tests updated for multi-token API ────────────────────────────────

#[test]
fn test_create_will_success() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
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
    );

    assert_eq!(will_id, 1);
    let will = client.get_will(&will_id);
    assert_eq!(will.owner, owner);
    assert_eq!(will.balances.get(token_address.clone()).unwrap(), 1_000_000);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.checkin_period_days, 90);
    assert_eq!(will.grace_period_days, 7);
    assert!(!will.is_native);
    assert_eq!(token.balance(&owner), 1_000_000_000 - 1_000_000);
    assert_eq!(token.balance(&client.address), 1_000_000);
}

#[test]
fn test_protocol_stats_track_create_cancel_and_release() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let second_admin = Address::generate(&env);
    let (second_token_client, second_token_admin_client) = create_token(&env, &second_admin);
    second_token_admin_client.mint(&owner, &1_000_000_000);
    let second_token_address = second_token_admin_client.address.clone();

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

    let stats = client.get_protocol_stats();
    assert_eq!(stats.active_will_count, 1);
    assert_eq!(stats.total_locked_by_token.len(), 1);
    assert_eq!(
        stats.total_locked_by_token.get(0).unwrap().token,
        token_address
    );
    assert_eq!(
        stats.total_locked_by_token.get(0).unwrap().total_locked,
        1_000_000
    );

    client.cancel_will(&will_id, &owner);

    let stats = client.get_protocol_stats();
    assert_eq!(stats.active_will_count, 0);
    assert_eq!(stats.total_locked_by_token.get(0).unwrap().total_locked, 0);

    let will_id_2 = client.create_will(
        &owner,
        &second_token_address,
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

    let stats = client.get_protocol_stats();
    assert_eq!(stats.active_will_count, 1);
    assert_eq!(stats.total_locked_by_token.len(), 2);
    assert_eq!(
        stats.total_locked_by_token.get(0).unwrap().token,
        token_address
    );
    assert_eq!(stats.total_locked_by_token.get(0).unwrap().total_locked, 0);
    assert_eq!(
        stats.total_locked_by_token.get(1).unwrap().token,
        second_token_address
    );
    assert_eq!(
        stats.total_locked_by_token.get(1).unwrap().total_locked,
        500_000
    );

    advance_time(&env, 31 * DAY);
    client.trigger_will(&will_id_2);
    advance_time(&env, 4 * DAY);
    client.release_inheritance(&will_id_2);

    let stats = client.get_protocol_stats();
    assert_eq!(stats.active_will_count, 0);
    assert_eq!(stats.total_locked_by_token.get(0).unwrap().total_locked, 0);
    assert_eq!(stats.total_locked_by_token.get(1).unwrap().total_locked, 0);
    assert_eq!(second_token_client.balance(&owner), 1_000_000_000 - 500_000);
}

#[test]
fn test_checkin_resets_deadline() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![
            &env,
            Beneficiary { address: beneficiary_a.clone(), percentage: 60 },
            Beneficiary { address: beneficiary_b.clone(), percentage: 40 },
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
    assert_eq!(will.balances.len(), 0);
}

#[test]
#[should_panic]
fn test_cannot_release_during_grace_period() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
    );

    client.cancel_will(&will_id, &owner);

    assert_eq!(token.balance(&owner), 1_000_000_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Cancelled);
    assert_eq!(will.balances.len(), 0);
}

#[test]
fn test_update_beneficiaries() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);
    let beneficiary_c = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary_a, percentage: 100 }],
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
    );

    client.update_beneficiaries(
        &will_id,
        &owner,
        &vec![
            &env,
            Beneficiary { address: beneficiary_b.clone(), percentage: 50 },
            Beneficiary { address: beneficiary_c.clone(), percentage: 50 },
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
    );

    client.guardian_trigger(&will_id, &guardian_1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);

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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
    );

    client.cancel_will(&will_id, &owner);
    client.update_guardians(&will_id, &owner, &vec![&env]);
}

#[test]
fn test_top_up_increases_balance() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &90,
        &7,
        &vec![&env],
    );

    client.top_up(&will_id, &owner, &token_address, &500_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.balances.get(token_address.clone()).unwrap(), 1_500_000);
    // Confirm token moved from owner to contract.
    assert_eq!(token.balance(&owner), 1_000_000_000 - 1_000_000 - 500_000);
    assert_eq!(token.balance(&client.address), 1_500_000);
}

#[test]
fn test_top_up_emits_event() {
    use soroban_sdk::{symbol_short, testutils::Events, TryIntoVal};

    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
    );

    client.top_up(&will_id, &owner, &token_address, &500_000);

    use soroban_sdk::{symbol_short, testutils::Events, TryIntoVal};
    let events = env.events().all();
    let mut found = false;
    for event in events.iter() {
        if !event.1.is_empty() {
            if let Ok(topic0) = event.1.get(0).unwrap().try_into_val(&env) {
                let topic0_sym: soroban_sdk::Symbol = topic0;
                if topic0_sym == symbol_short!("topup") {
                    found = true;
                    let topic1: u64 = event.1.get(1).unwrap().try_into_val(&env).unwrap();
                    assert_eq!(topic1, will_id);
                    // data: (owner, token, amount, new_balance)
                    let data: (Address, Address, i128, i128) =
                        event.2.try_into_val(&env).unwrap();
                    assert_eq!(data, (owner.clone(), token_address.clone(), 500_000_i128, 1_500_000_i128));
                }
            }
        }
    }
    assert!(found, "topup event not found");
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
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
        &vec![&env, guardian_1.clone(), guardian_2.clone(), guardian_3.clone()],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![
            &env,
            Beneficiary { address: beneficiary_a, percentage: 60 },
            Beneficiary { address: beneficiary_b, percentage: 30 },
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
    );
}

#[test]
fn test_get_wills_by_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 500_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
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
    );
    client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 250_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
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
    );

    let wills = client.get_wills_by_beneficiary(&beneficiary);
    assert_eq!(wills.len(), 1);
    assert_eq!(wills.get(0).unwrap().id, will_id);
}

// ── Native XLM tests ──────────────────────────────────────────────────

#[test]
fn test_native_create_will_success() {
    let (env, client, owner) = setup_native();
    let beneficiary = Address::generate(&env);

    let owner_initial = env.balance(&owner);

    let will_id = client.create_will(
        &owner,
        &owner, // token address is unused for native, pass owner as placeholder
        &1_000_000_000,
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
        &true,
    );

    assert_eq!(will_id, 1);
    let will = client.get_will(&will_id);
    assert_eq!(will.owner, owner);
    assert_eq!(will.balance, 1_000_000_000);
    assert_eq!(will.status, WillStatus::Active);
    assert!(will.is_native);
    assert_eq!(will.checkin_period_days, 90);
    assert_eq!(will.grace_period_days, 7);

    // Owner should have lost the amount, contract gained it
    assert_eq!(env.balance(&owner), owner_initial - 1_000_000_000);
    assert_eq!(env.balance(&client.address), 1_000_000_000);
}

#[test]
fn test_native_checkin_resets_deadline() {
    let (env, client, owner) = setup_native();

    let will_id = client.create_will(
        &owner,
        &owner,
        &1_000_000_000,
        &vec![
            &env,
            Beneficiary {
                address: Address::generate(&env),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &true,
    );

    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.last_checkin, 1_700_000_000 + 10 * DAY);
    assert_eq!(will.status, WillStatus::Active);
}

#[test]
fn test_native_trigger_and_release() {
    let (env, client, owner) = setup_native();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &owner,
        &1_000_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
// ── new multi-token tests ─────────────────────────────────────────────────────

#[test]
fn test_create_will_multi_token_balances_stored() {
    let (env, client, owner, token_a, token_a_addr, token_b, token_b_addr) =
        setup_two_tokens();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![
            &env,
            (token_a_addr.clone(), 1_000_000_i128),
            (token_b_addr.clone(), 2_000_000_i128),
        ],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
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
        &true,
    );

    let will = client.get_will(&will_id);
    assert_eq!(will.balances.len(), 2);
    assert_eq!(will.balances.get(token_a_addr.clone()).unwrap(), 1_000_000);
    assert_eq!(will.balances.get(token_b_addr.clone()).unwrap(), 2_000_000);

    // Tokens must have moved from owner to contract.
    assert_eq!(token_a.balance(&owner), 1_000_000_000 - 1_000_000);
    assert_eq!(token_b.balance(&owner), 1_000_000_000 - 2_000_000);
    assert_eq!(token_a.balance(&client.address), 1_000_000);
    assert_eq!(token_b.balance(&client.address), 2_000_000);
}

#[test]
fn test_top_up_new_token_adds_to_map() {
    let (env, client, owner, _token_a, token_a_addr, token_b, token_b_addr) =
        setup_two_tokens();
    let beneficiary = Address::generate(&env);

    // Create with only token_a.
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Triggered);

    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // Beneficiary should have received the full balance
    assert_eq!(env.balance(&beneficiary), 1_000_000_000);
    assert_eq!(env.balance(&client.address), 0);
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

#[test]
fn test_native_release_splits_multiple_beneficiaries() {
    let (env, client, owner) = setup_native();
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
        &owner,
        &1_000_000_000,
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
        &true,
    );

    // Top up with a brand-new token_b — it should appear as a new map entry.
    client.top_up(&will_id, &owner, &token_b_addr, &500_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.balances.len(), 2);
    assert_eq!(will.balances.get(token_a_addr.clone()).unwrap(), 1_000_000);
    assert_eq!(will.balances.get(token_b_addr.clone()).unwrap(), 500_000);

    assert_eq!(token_b.balance(&owner), 1_000_000_000 - 500_000);
    assert_eq!(token_b.balance(&client.address), 500_000);
}

#[test]
fn test_top_up_existing_token_accumulates() {
    let (env, client, owner, token_a, token_a_addr, _token_b, _token_b_addr) =
        setup_two_tokens();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    assert_eq!(env.balance(&beneficiary_a), 600_000_000);
    assert_eq!(env.balance(&beneficiary_b), 400_000_000);
    assert_eq!(env.balance(&client.address), 0);
}

#[test]
fn test_native_emergency_checkin() {
    let (env, client, owner) = setup_native();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &owner,
        &1_000_000_000,
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
        &true,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);

    advance_time(&env, 2 * DAY);
    client.emergency_checkin(&will_id, &owner);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert!(will.trigger_time.is_none());
    assert_eq!(will.last_checkin, 1_700_000_000 + 91 * DAY + 2 * DAY);
    // Balance should still be in the contract
    assert_eq!(env.balance(&client.address), 1_000_000_000);
}

#[test]
fn test_native_cancel_will() {
    let (env, client, owner) = setup_native();
    let owner_initial = env.balance(&owner);
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &owner,
        &1_000_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                percentage: 100,
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
        &true,
    );

    // Contract should have the native XLM
    assert_eq!(env.balance(&client.address), 1_000_000_000);

    client.cancel_will(&will_id, &owner);

    // Owner should be fully refunded
    assert_eq!(env.balance(&owner), owner_initial);
    assert_eq!(env.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Cancelled);
    assert_eq!(will.balance, 0);
}

#[test]
fn test_native_top_up() {
    let (env, client, owner) = setup_native();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &owner,
        &500_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary,
                percentage: 100,
    );

    client.top_up(&will_id, &owner, &token_a_addr, &250_000);
    client.top_up(&will_id, &owner, &token_a_addr, &250_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.balances.get(token_a_addr.clone()).unwrap(), 1_500_000);
    assert_eq!(token_a.balance(&client.address), 1_500_000);
}

#[test]
fn test_cancel_will_refunds_all_tokens() {
    let (env, client, owner, token_a, token_a_addr, token_b, token_b_addr) =
        setup_two_tokens();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![
            &env,
            (token_a_addr.clone(), 1_000_000_i128),
            (token_b_addr.clone(), 2_000_000_i128),
        ],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
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
        &true,
    );

    let owner_before_topup = env.balance(&owner);
    client.top_up(&will_id, &owner, &300_000_000);

    assert_eq!(env.balance(&owner), owner_before_topup - 300_000_000);
    assert_eq!(env.balance(&client.address), 800_000_000);

    let will = client.get_will(&will_id);
    assert_eq!(will.balance, 800_000_000);
    client.cancel_will(&will_id, &owner);

    // Full balances must be returned to the owner.
    assert_eq!(token_a.balance(&owner), 1_000_000_000);
    assert_eq!(token_b.balance(&owner), 1_000_000_000);
    // Contract must hold nothing.
    assert_eq!(token_a.balance(&client.address), 0);
    assert_eq!(token_b.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Cancelled);
    assert_eq!(will.balances.len(), 0);
}

#[test]
fn test_release_inheritance_distributes_all_tokens_proportionally() {
    let (env, client, owner, token_a, token_a_addr, token_b, token_b_addr) =
        setup_two_tokens();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![
            &env,
            (token_a_addr.clone(), 1_000_000_i128),
            (token_b_addr.clone(), 3_000_000_i128),
        ],
        &vec![
            &env,
            Beneficiary { address: beneficiary_a.clone(), percentage: 60 },
            Beneficiary { address: beneficiary_b.clone(), percentage: 40 },
}

#[test]
fn test_native_guardian_trigger() {
    let (env, client, owner) = setup_native();
    let beneficiary = Address::generate(&env);
    let guardian_a = Address::generate(&env);
    let guardian_b = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &owner,
        &1_000_000_000,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env, guardian_a.clone(), guardian_b.clone()],
        &true,
    );

    // First vote should not trigger release
    client.guardian_trigger(&will_id, &guardian_a);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.guardian_votes, 1);
    assert_eq!(env.balance(&beneficiary), 0);

    // Second vote should release
    client.guardian_trigger(&will_id, &guardian_b);
    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(env.balance(&beneficiary), 1_000_000_000);
    assert_eq!(env.balance(&client.address), 0);
}

#[test]
fn test_native_rounding_remainder() {
    let (env, client, owner) = setup_native();
    let beneficiary_a = Address::generate(&env);
    let beneficiary_b = Address::generate(&env);
    let beneficiary_c = Address::generate(&env);

    // Amount that does not split evenly among 3 beneficiaries (10+33+57=100 -> 10/100, 33/100, 57/100)
    let will_id = client.create_will(
        &owner,
        &owner,
        &100, // 100 XLM
        &vec![&env],
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);

    // token_a: 60% → 600_000, 40% → 400_000
    assert_eq!(token_a.balance(&beneficiary_a), 600_000);
    assert_eq!(token_a.balance(&beneficiary_b), 400_000);

    // token_b: 60% → 1_800_000, 40% → 1_200_000
    assert_eq!(token_b.balance(&beneficiary_a), 1_800_000);
    assert_eq!(token_b.balance(&beneficiary_b), 1_200_000);

    // Contract must hold nothing.
    assert_eq!(token_a.balance(&client.address), 0);
    assert_eq!(token_b.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balances.len(), 0);
}

#[test]
fn test_release_inheritance_rounding_remainder_goes_to_last_beneficiary() {
    // Use an amount not evenly divisible: 1_000_001 split 33/33/34.
    let (env, client, owner, token_a, token_a_addr, _token_b, _token_b_addr) =
        setup_two_tokens();
    let b1 = Address::generate(&env);
    let b2 = Address::generate(&env);
    let b3 = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_001_i128)],
        &vec![
            &env,
            Beneficiary { address: b1.clone(), percentage: 33 },
            Beneficiary { address: b2.clone(), percentage: 33 },
            Beneficiary { address: b3.clone(), percentage: 34 },
        ],
        &90,
        &7,
        &vec![&env],
    client.update_beneficiaries(
        &will_id,
        &owner,
        &vec![
            &env,
            Beneficiary {
                address: beneficiary_a.clone(),
                percentage: 10,
            },
            Beneficiary {
                address: beneficiary_b.clone(),
                percentage: 33,
            },
            Beneficiary {
                address: beneficiary_c.clone(),
                percentage: 57,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &true,
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

    // Expected: 10% of 100 = 10, 33% of 100 = 33, remainder = 100 - 10 - 33 = 57
    assert_eq!(env.balance(&beneficiary_a), 10);
    assert_eq!(env.balance(&beneficiary_b), 33);
    assert_eq!(env.balance(&beneficiary_c), 57);
    assert_eq!(env.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balance, 0);
    let share1 = token_a.balance(&b1); // floor(1_000_001 * 33 / 100) = 330_000
    let share2 = token_a.balance(&b2); // 330_000
    let share3 = token_a.balance(&b3); // remainder = 1_000_001 - 330_000 - 330_000 = 340_001
    assert_eq!(share1, 330_000);
    assert_eq!(share2, 330_000);
    assert_eq!(share3, 340_001);
    // Total must equal the locked amount exactly.
    assert_eq!(share1 + share2 + share3, 1_000_001);
    assert_eq!(token_a.balance(&client.address), 0);
}

#[test]
fn test_guardian_trigger_distributes_all_tokens() {
    let (env, client, owner, token_a, token_a_addr, token_b, token_b_addr) =
        setup_two_tokens();
    let beneficiary = Address::generate(&env);
    let guardian_1 = Address::generate(&env);
    let guardian_2 = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![
            &env,
            (token_a_addr.clone(), 1_000_000_i128),
            (token_b_addr.clone(), 500_000_i128),
        ],
        &vec![&env, Beneficiary { address: beneficiary.clone(), percentage: 100 }],
        &90,
        &7,
        &vec![&env, guardian_1.clone(), guardian_2.clone()],
    );

    client.guardian_trigger(&will_id, &guardian_1);
    // Only one vote: will still active, no tokens moved yet.
    assert_eq!(token_a.balance(&beneficiary), 0);
    assert_eq!(token_b.balance(&beneficiary), 0);

    client.guardian_trigger(&will_id, &guardian_2);
    // Quorum reached: all tokens distributed.
    assert_eq!(token_a.balance(&beneficiary), 1_000_000);
    assert_eq!(token_b.balance(&beneficiary), 500_000);
    assert_eq!(token_a.balance(&client.address), 0);
    assert_eq!(token_b.balance(&client.address), 0);

    let will = client.get_will(&will_id);
    assert_eq!(will.status, WillStatus::Released);
    assert_eq!(will.balances.len(), 0);
}

#[test]
#[should_panic]
fn test_native_cannot_trigger_before_deadline() {
    let (env, client, owner) = setup_native();

    let will_id = client.create_will(
        &owner,
        &owner,
        &1_000_000_000,
        &vec![
            &env,
            Beneficiary {
                address: Address::generate(&env),
                percentage: 100,
fn test_create_will_zero_amount_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 0_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
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
        &true,
    );

    advance_time(&env, 10 * DAY);
    client.trigger_will(&will_id);
}

#[test]
#[should_panic]
fn test_native_cannot_release_during_grace_period() {
    let (env, client, owner) = setup_native();

    let will_id = client.create_will(
        &owner,
        &owner,
        &1_000_000_000,
        &vec![
            &env,
            Beneficiary {
                address: Address::generate(&env),
                percentage: 100,
            },
        ],
        &90,
        &7,
        &vec![&env],
        &true,
    );

    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 2 * DAY);
    // Grace period is 7 days, so 2 days is still within it
    client.release_inheritance(&will_id);
}

    );
}

#[test]
#[should_panic]
fn test_top_up_zero_amount_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, Beneficiary { address: beneficiary, percentage: 100 }],
        &90,
        &7,
        &vec![&env],
    );

    client.top_up(&will_id, &owner, &token_address, &0);

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
