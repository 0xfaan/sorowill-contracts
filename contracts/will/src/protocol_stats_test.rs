#![cfg(test)]

//! Regression coverage for `get_protocol_stats` (#211): asserts
//! `active_will_count` reflects reality across `create_will`, `cancel_will`,
//! and `release_inheritance`.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient};

const DAY: u64 = 86_400;

fn setup(env: &Env) -> (WillContractClient<'_>, Address, Address) {
    let owner = Address::generate(env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(env, &token_address).mint(&owner, &10_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(env, &contract_id);
    (client, owner, token_address)
}

#[test]
fn active_will_count_reflects_create_cancel_and_release() {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let (client, owner, token_address) = setup(&env);
    let beneficiary = Address::generate(&env);

    assert_eq!(client.get_protocol_stats().active_will_count, 0);

    let will_a = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![&env],
        &2,
        &None,
        &0,
    );
    assert_eq!(client.get_protocol_stats().active_will_count, 1);

    let will_b = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![
            &env,
            Beneficiary {
                address: beneficiary.clone(),
                allocation: Allocation::Percentage(10_000),
            },
        ],
        &90,
        &7,
        &vec![&env],
        &2,
        &None,
        &0,
    );
    assert_eq!(client.get_protocol_stats().active_will_count, 2);

    // Cancelling a will while it is still Active must decrement the counter.
    client.cancel_will(&will_a, &owner);
    assert_eq!(client.get_protocol_stats().active_will_count, 1);

    // Releasing the remaining will must also decrement the counter.
    env.ledger().with_mut(|l| l.timestamp += 91 * DAY);
    client.trigger_will(&will_b);
    env.ledger().with_mut(|l| l.timestamp += 8 * DAY);
    client.release_inheritance(&will_b, &None);
    assert_eq!(client.get_protocol_stats().active_will_count, 0);
}
