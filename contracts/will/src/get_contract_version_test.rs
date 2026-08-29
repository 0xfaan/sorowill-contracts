#![cfg(test)]

use soroban_sdk::{
    testutils::Address as _,
    token::StellarAssetClient,
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillError, WillStatus, CONTRACT_VERSION};

const DAY: u64 = 86_400;

fn setup<'a>() -> (Env, WillContractClient<'a>, Address) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();
    StellarAssetClient::new(&env, &token_address).mint(&owner, &1_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    (env, client, owner)
}

#[test]
fn test_get_contract_version_returns_contract_version() {
    let (env, client, _owner) = setup();

    let version = client.get_contract_version(&env);
    assert_eq!(version, CONTRACT_VERSION);
}

#[test]
fn test_get_contract_version_matches_constant() {
    let (env, client, _owner) = setup();

    let version = client.get_contract_version(&env);
    // CONTRACT_VERSION should be encoded as major * 1_000_000 + minor * 1_000 + patch
    // Currently it's 1_000_000 which represents version 1.0.0
    assert_eq!(version, 1_000_000);
}

#[test]
fn test_get_contract_version_with_migrate_will_version_check() {
    let (env, client, owner) = setup();
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();

    // Create a will
    let beneficiary = Address::generate(&env);
    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: beneficiary,
            allocation: Allocation::Percentage(10_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 1_000_000_i128)];

    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &30,
        &7,
        &vec![&env],
        &1,
        &None,
        &0,
    );

    client.confirm_will(&will_id, &owner);

    // Get the current version
    let version = client.get_contract_version(&env);

    // Attempting to migrate with the current version should succeed
    // (migrate_will should not fail due to version mismatch)
    let migrate_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.migrate_will(&will_id, &owner, &version);
    }));

    // Should succeed because versions match
    assert!(migrate_result.is_ok(), "Migrate should succeed with matching version");
}

#[test]
fn test_get_contract_version_with_mismatched_version_fails_migrate() {
    let (env, client, owner) = setup();
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();

    // Create a will
    let beneficiary = Address::generate(&env);
    let beneficiaries: SorobanVec<Beneficiary> = vec![
        &env,
        Beneficiary {
            address: beneficiary,
            allocation: Allocation::Percentage(10_000),
        },
    ];
    let tokens: SorobanVec<(Address, i128)> = vec![&env, (token_address, 1_000_000_i128)];

    let will_id = client.create_will(
        &owner,
        &tokens,
        &beneficiaries,
        &30,
        &7,
        &vec![&env],
        &1,
        &None,
        &0,
    );

    client.confirm_will(&will_id, &owner);

    // Use a mismatched version (e.g., 2.0.0 = 2_000_000)
    let mismatched_version = 2_000_000u32;

    // Attempting to migrate with a mismatched version should fail
    let migrate_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        client.migrate_will(&will_id, &owner, &mismatched_version);
    }));

    // Should panic due to version mismatch
    assert!(migrate_result.is_err(), "Migrate should fail with mismatched version");
}
