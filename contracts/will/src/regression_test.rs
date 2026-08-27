#![cfg(test)]

//! Regression tests for GitHub issues #191, #192, #193, #194.

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillStatus};

const DAY: u64 = 86_400;

fn setup<'a>() -> (Env, WillContractClient<'a>, Address, TokenClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env.clone(), client, owner, TokenClient::new(&env, &token_address), token_address)
}

/// Issue #191: Regression test asserting `active_will_count` increases after `clone_will`.
#[test]
fn issue_191_clone_will_increments_active_count() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);

    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: beneficiary.clone(),
            allocation: Allocation::Percentage(10_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address.clone(), 100_000_i128)];

    let initial_stats = client.get_protocol_stats();
    let initial_count = initial_stats.active_will_count;

    let source_will_id = client.create_will(&owner, &tokens, &beneficiaries, &90, &7, &vec![&env], &2, &None, &0);

    let stats_after_create = client.get_protocol_stats();
    assert_eq!(stats_after_create.active_will_count, initial_count + 1);

    let clone_tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 50_000_i128)];
    let _cloned_will_id = client.clone_will(&source_will_id, &owner, &clone_tokens);

    let stats_after_clone = client.get_protocol_stats();
    assert_eq!(stats_after_clone.active_will_count, initial_count + 2, "clone_will should increment active_will_count");
}
