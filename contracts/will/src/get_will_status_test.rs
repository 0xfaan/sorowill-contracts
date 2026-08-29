#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{Allocation, Beneficiary, WillContract, WillContractClient, WillStatus};

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
fn test_get_will_status_pending_confirmation() {
    let (env, client, owner) = setup();
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();

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

    let status = client.get_will_status(&will_id);
    assert_eq!(status, WillStatus::PendingConfirmation);
}

#[test]
fn test_get_will_status_active() {
    let (env, client, owner) = setup();
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();

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

    // Confirm the will
    client.confirm_will(&will_id, &owner);

    let status = client.get_will_status(&will_id);
    assert_eq!(status, WillStatus::Active);
}

#[test]
fn test_get_will_status_triggered() {
    let (env, client, owner) = setup();
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();

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

    // Confirm the will
    client.confirm_will(&will_id, &owner);

    // Advance past check-in deadline
    env.ledger().with_mut(|l| l.timestamp += (31 * DAY) as u64);

    // Trigger the will
    client.trigger_will(&will_id);

    let status = client.get_will_status(&will_id);
    assert_eq!(status, WillStatus::Triggered);
}

#[test]
fn test_get_will_status_released() {
    let (env, client, owner) = setup();
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();

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

    // Confirm and trigger
    client.confirm_will(&will_id, &owner);
    env.ledger().with_mut(|l| l.timestamp += (31 * DAY) as u64);
    client.trigger_will(&will_id);

    // Release
    env.ledger().with_mut(|l| l.timestamp += (8 * DAY) as u64);
    client.release_inheritance(&will_id, &None);

    let status = client.get_will_status(&will_id);
    assert_eq!(status, WillStatus::Released);
}

#[test]
fn test_get_will_status_cancelled() {
    let (env, client, owner) = setup();
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token_address = sac.address();

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

    // Confirm and cancel
    client.confirm_will(&will_id, &owner);
    client.cancel_will(&will_id, &owner);

    let status = client.get_will_status(&will_id);
    assert_eq!(status, WillStatus::Cancelled);
}

#[test]
#[should_panic(match = "WillNotFound")]
fn test_get_will_status_nonexistent_will_panics() {
    let (env, client, _owner) = setup();
    client.get_will_status(&9999);
}
