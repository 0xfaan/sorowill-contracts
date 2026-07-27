#![cfg(test)]

use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token::{Client as TokenClient, StellarAssetClient},
    vec, Address, Env, Vec as SorobanVec,
};

use crate::{Beneficiary, WillContract, WillContractClient, WillError, WillStatus};

fn create_token<'a>(env: &Env, admin: &Address) -> (TokenClient<'a>, StellarAssetClient<'a>) {
    let sac = env.register_stellar_asset_contract_v2(admin.clone());
    (
        TokenClient::new(env, &sac.address()),
        StellarAssetClient::new(env, &sac.address()),
    )
}

fn setup<'a>() -> (
    Env,
    WillContractClient<'a>,
    Address,
    TokenClient<'a>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);
    let owner = Address::generate(&env);
    let (token_client, token_admin) = create_token(&env, &owner);
    token_admin.mint(&owner, &1_000_000_000);
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);
    (env, client, owner, token_client, token_admin.address.clone())
}

fn setup_two_tokens<'a>() -> (
    Env,
    WillContractClient<'a>,
    Address,
    TokenClient<'a>,
    Address,
    TokenClient<'a>,
    Address,
) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);
    let owner = Address::generate(&env);
    let (token_a_client, token_a_admin) = create_token(&env, &owner);
    token_a_admin.mint(&owner, &1_000_000_000);
    let token_a_addr = token_a_admin.address.clone();
    let token_b_admin_addr = Address::generate(&env);
    let (token_b_client, token_b_admin) = create_token(&env, &token_b_admin_addr);
    token_b_admin.mint(&owner, &1_000_000_000);
    let token_b_addr = token_b_admin.address.clone();
    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);
    (env, client, owner, token_a_client, token_a_addr, token_b_client, token_b_addr)
}

fn advance_time(env: &Env, seconds: u64) {
    env.ledger().with_mut(|l| { l.timestamp += seconds; });
}

const DAY: u64 = 86_400;

fn bp(beneficiary: &Address, points: u32) -> Beneficiary {
    Beneficiary { address: beneficiary.clone(), basis_points: points }
}

// ── create_will ──────────────────────────────────────────────────────────────

#[test]
fn test_create_will_success() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    assert_eq!(will_id, 1);
    let will = client.get_will(&will_id);
    assert_eq!(will.owner, owner);
    assert_eq!(will.balances.get(token_address.clone()).unwrap(), 1_000_000);
    assert_eq!(will.status, WillStatus::Active);
    assert_eq!(will.checkin_period_days, 90);
    assert_eq!(will.grace_period_days, 7);
    assert_eq!(token.balance(&owner), 1_000_000_000 - 1_000_000);
    assert_eq!(token.balance(&client.address), 1_000_000);
}

#[test]
fn test_create_will_multi_token() {
    let (env, client, owner, token_a, token_a_addr, token_b, token_b_addr) = setup_two_tokens();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_000_i128), (token_b_addr.clone(), 2_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    let will = client.get_will(&will_id);
    assert_eq!(will.balances.len(), 2);
    assert_eq!(will.balances.get(token_a_addr).unwrap(), 1_000_000);
    assert_eq!(will.balances.get(token_b_addr).unwrap(), 2_000_000);
}

#[test]
#[should_panic]
fn test_create_will_zero_amount_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    client.create_will(
        &owner,
        &vec![&env, (token_address, 0_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
}

#[test]
#[should_panic]
fn test_invalid_percentages_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&a, 6_000), bp(&b, 3_000)],
        &90, &7, &vec![&env],
    );
}

// ── check_in ─────────────────────────────────────────────────────────────────

#[test]
fn test_checkin_resets_deadline() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    advance_time(&env, 10 * DAY);
    client.check_in(&will_id, &owner);
    let will = client.get_will(&will_id);
    assert_eq!(will.last_checkin, 1_700_000_000 + 10 * DAY);
    assert_eq!(will.status, WillStatus::Active);
}

// ── trigger ──────────────────────────────────────────────────────────────────

#[test]
fn test_trigger_after_missed_checkin() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
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
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    advance_time(&env, 10 * DAY);
    client.trigger_will(&will_id);
}

// ── emergency_checkin ────────────────────────────────────────────────────────

#[test]
fn test_emergency_checkin_cancels_trigger() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
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

// ── release_inheritance ──────────────────────────────────────────────────────

#[test]
fn test_release_inheritance_splits_correctly() {
    let (env, client, owner, token, token_address) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&a, 6_000), bp(&b, 4_000)],
        &90, &7, &vec![&env],
    );
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);
    assert_eq!(token.balance(&a), 600_000);
    assert_eq!(token.balance(&b), 400_000);
    assert_eq!(token.balance(&client.address), 0);
    assert_eq!(client.get_will(&will_id).status, WillStatus::Released);
}

#[test]
#[should_panic]
fn test_cannot_release_during_grace_period() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 2 * DAY);
    client.release_inheritance(&will_id);
}

#[test]
fn test_fractional_three_way_split() {
    let (env, client, owner, token, token_address) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let c = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&a, 5_000), bp(&b, 3_333), bp(&c, 1_667)],
        &90, &7, &vec![&env],
    );
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);
    assert_eq!(token.balance(&a), 500_000);
    assert_eq!(token.balance(&b), 333_300);
    assert_eq!(token.balance(&c), 166_700);
    assert_eq!(token.balance(&client.address), 0);
}

#[test]
fn test_release_inheritance_rounding_remainder() {
    let (env, client, owner, token, token_address) = setup();
    let b1 = Address::generate(&env);
    let b2 = Address::generate(&env);
    let b3 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_001_i128)],
        &vec![&env, bp(&b1, 3_333), bp(&b2, 3_333), bp(&b3, 3_334)],
        &90, &7, &vec![&env],
    );
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);
    let s1 = token.balance(&b1);
    let s2 = token.balance(&b2);
    let s3 = token.balance(&b3);
    assert_eq!(s1 + s2 + s3, 1_000_001);
    assert_eq!(token.balance(&client.address), 0);
}

#[test]
fn test_release_multi_token_proportionally() {
    let (env, client, owner, token_a, token_a_addr, token_b, token_b_addr) = setup_two_tokens();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_000_i128), (token_b_addr.clone(), 3_000_000_i128)],
        &vec![&env, bp(&a, 6_000), bp(&b, 4_000)],
        &90, &7, &vec![&env],
    );
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);
    assert_eq!(token_a.balance(&a), 600_000);
    assert_eq!(token_a.balance(&b), 400_000);
    assert_eq!(token_b.balance(&a), 1_800_000);
    assert_eq!(token_b.balance(&b), 1_200_000);
}

// ── cancel_will ──────────────────────────────────────────────────────────────

#[test]
fn test_cancel_will_refunds_owner() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.cancel_will(&will_id, &owner);
    assert_eq!(token.balance(&owner), 1_000_000_000);
    assert_eq!(client.get_will(&will_id).status, WillStatus::Cancelled);
}

#[test]
fn test_cancel_will_refunds_all_tokens() {
    let (env, client, owner, token_a, token_a_addr, token_b, token_b_addr) = setup_two_tokens();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_000_i128), (token_b_addr.clone(), 2_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.cancel_will(&will_id, &owner);
    assert_eq!(token_a.balance(&owner), 1_000_000_000);
    assert_eq!(token_b.balance(&owner), 1_000_000_000);
}

// ── update_beneficiaries ─────────────────────────────────────────────────────

#[test]
fn test_update_beneficiaries() {
    let (env, client, owner, _token, token_address) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let c = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&a, 10_000)],
        &90, &7, &vec![&env],
    );
    client.update_beneficiaries(&will_id, &owner, &vec![&env, bp(&b, 5_000), bp(&c, 5_000)]);
    assert_eq!(client.get_will(&will_id).beneficiaries.len(), 2);
    assert_eq!(client.get_wills_by_beneficiary(&b, &None, &100).len(), 1);
}

#[test]
fn test_update_beneficiaries_fractional_split() {
    let (env, client, owner, token, token_address) = setup();
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let orig = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&orig, 10_000)],
        &90, &7, &vec![&env],
    );
    client.update_beneficiaries(&will_id, &owner, &vec![&env, bp(&a, 2_500), bp(&b, 7_500)]);
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    advance_time(&env, 8 * DAY);
    client.release_inheritance(&will_id);
    assert_eq!(token.balance(&a), 250_000);
    assert_eq!(token.balance(&b), 750_000);
}

#[test]
#[should_panic]
fn test_update_beneficiaries_rejects_invalid_bp() {
    let (env, client, owner, _token, token_address) = setup();
    let orig = Address::generate(&env);
    let a = Address::generate(&env);
    let b = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&orig, 10_000)],
        &90, &7, &vec![&env],
    );
    client.update_beneficiaries(&will_id, &owner, &vec![&env, bp(&a, 3_000), bp(&b, 3_000)]);
}

// ── update_guardians ─────────────────────────────────────────────────────────

#[test]
fn test_update_guardians() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let old = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env, old],
    );
    client.update_guardians(&will_id, &owner, &vec![&env, g1.clone(), g2.clone()]);
    let will = client.get_will(&will_id);
    assert_eq!(will.guardians, vec![&env, g1, g2]);
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
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.update_guardians(&will_id, &non_owner, &vec![&env]);
}

#[test]
#[should_panic]
fn test_update_guardians_rejects_too_many() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.update_guardians(&will_id, &owner, &vec![
        &env, Address::generate(&env), Address::generate(&env),
        Address::generate(&env), Address::generate(&env),
    ]);
}

#[test]
fn test_update_guardians_resets_votes() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env, g1.clone(), g2.clone()],
    );
    advance_time(&env, 8 * DAY);
    client.guardian_trigger(&will_id, &g1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);
    client.update_guardians(&will_id, &owner, &vec![&env, g2.clone()]);
    assert_eq!(client.get_will(&will_id).guardian_votes, 0);
}

#[test]
#[should_panic]
fn test_update_guardians_rejected_while_triggered() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    advance_time(&env, 91 * DAY);
    client.trigger_will(&will_id);
    client.update_guardians(&will_id, &owner, &vec![&env]);
}

// ── top_up ───────────────────────────────────────────────────────────────────

#[test]
fn test_top_up_increases_balance() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.top_up(&will_id, &owner, &token_address, &500_000);
    assert_eq!(client.get_will(&will_id).balances.get(token_address.clone()).unwrap(), 1_500_000);
    assert_eq!(token.balance(&client.address), 1_500_000);
}

#[test]
fn test_top_up_new_token() {
    let (env, client, owner, _token_a, token_a_addr, token_b, token_b_addr) = setup_two_tokens();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.top_up(&will_id, &owner, &token_b_addr, &500_000);
    let will = client.get_will(&will_id);
    assert_eq!(will.balances.len(), 2);
    assert_eq!(will.balances.get(token_b_addr).unwrap(), 500_000);
}

#[test]
fn test_top_up_existing_token_accumulates() {
    let (env, client, owner, _token_a, token_a_addr, _token_b, _token_b_addr) = setup_two_tokens();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.top_up(&will_id, &owner, &token_a_addr, &250_000);
    client.top_up(&will_id, &owner, &token_a_addr, &250_000);
    assert_eq!(client.get_will(&will_id).balances.get(token_a_addr).unwrap(), 1_500_000);
}

#[test]
#[should_panic]
fn test_top_up_zero_amount_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.top_up(&will_id, &owner, &token_address, &0);
}

// ── guardian_trigger ─────────────────────────────────────────────────────────

#[test]
fn test_guardian_trigger_requires_two_votes() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let g3 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env, g1.clone(), g2.clone(), g3],
    );
    advance_time(&env, 8 * DAY);
    client.guardian_trigger(&will_id, &g1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);
    assert_eq!(token.balance(&beneficiary), 0);
    client.guardian_trigger(&will_id, &g2);
    assert_eq!(client.get_will(&will_id).status, WillStatus::Released);
    assert_eq!(token.balance(&beneficiary), 1_000_000);
}

#[test]
fn test_guardian_trigger_multi_token() {
    let (env, client, owner, _token_a, token_a_addr, _token_b, token_b_addr) = setup_two_tokens();
    let beneficiary = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_a_addr.clone(), 1_000_000_i128), (token_b_addr.clone(), 500_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env, g1.clone(), g2],
    );
    advance_time(&env, 8 * DAY);
    client.guardian_trigger(&will_id, &g1);
    client.guardian_trigger(&will_id, &g2);
    assert_eq!(client.get_will(&will_id).status, WillStatus::Released);
}

// ── pagination ───────────────────────────────────────────────────────────────

#[test]
fn test_get_wills_by_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 500_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    client.create_will(
        &owner,
        &vec![&env, (token_address, 250_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &30, &3, &vec![&env],
    );
    let wills = client.get_wills_by_owner(&owner, &None, &100);
    assert_eq!(wills.len(), 2);
}

#[test]
fn test_get_wills_by_beneficiary() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    let wills = client.get_wills_by_beneficiary(&beneficiary, &None, &100);
    assert_eq!(wills.len(), 1);
    assert_eq!(wills.get(0).unwrap().id, will_id);
}

#[test]
fn test_pagination_owner() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    for _ in 0..5 {
        client.create_will(
            &owner,
            &vec![&env, (token_address.clone(), 100_000_i128)],
            &vec![&env, bp(&beneficiary, 10_000)],
            &90, &7, &vec![&env],
        );
    }
    let page1 = client.get_wills_by_owner(&owner, &None, &2);
    assert_eq!(page1.len(), 2);
    let last_id = page1.get(1).unwrap().id;
    let page2 = client.get_wills_by_owner(&owner, &Some(last_id), &2);
    assert_eq!(page2.len(), 2);
    assert!(page2.get(0).unwrap().id > last_id);
}

#[test]
fn test_pagination_beneficiary() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    for _ in 0..4 {
        client.create_will(
            &owner,
            &vec![&env, (token_address.clone(), 100_000_i128)],
            &vec![&env, bp(&beneficiary, 10_000)],
            &90, &7, &vec![&env],
        );
    }
    let page1 = client.get_wills_by_beneficiary(&beneficiary, &None, &2);
    assert_eq!(page1.len(), 2);
    let last_id = page1.get(1).unwrap().id;
    let page2 = client.get_wills_by_beneficiary(&beneficiary, &Some(last_id), &10);
    assert_eq!(page2.len(), 2);
}

// ── clone_will ───────────────────────────────────────────────────────────────

#[test]
fn test_clone_will_copies_configuration() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let g1 = Address::generate(&env);
    let source_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env, g1.clone()],
    );
    advance_time(&env, 5 * DAY);
    let clone_id = client.clone_will(
        &source_id,
        &owner,
        &vec![&env, (token_address.clone(), 500_000_i128)],
    );
    let clone = client.get_will(&clone_id);
    assert_eq!(clone.id, 2);
    assert_eq!(clone.checkin_period_days, 90);
    assert_eq!(clone.grace_period_days, 7);
    assert_eq!(clone.beneficiaries, vec![&env, bp(&beneficiary, 10_000)]);
    assert_eq!(clone.guardians, vec![&env, g1]);
    assert_eq!(clone.status, WillStatus::Active);
    assert_eq!(clone.balances.get(token_address).unwrap(), 500_000);
    assert_eq!(clone.owner, owner);
}

#[test]
fn test_clone_will_independent_from_source() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let source_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    let clone_id = client.clone_will(
        &source_id,
        &owner,
        &vec![&env, (token_address.clone(), 500_000_i128)],
    );
    client.top_up(&clone_id, &owner, &token_address, &100_000);
    assert_eq!(client.get_will(&source_id).balances.get(token_address.clone()).unwrap(), 1_000_000);
    assert_eq!(client.get_will(&clone_id).balances.get(token_address).unwrap(), 600_000);
}

#[test]
fn test_clone_will_indexed() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let source_id = client.create_will(
        &owner,
        &vec![&env, (token_address.clone(), 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env],
    );
    let clone_id = client.clone_will(
        &source_id,
        &owner,
        &vec![&env, (token_address, 500_000_i128)],
    );
    let owner_wills = client.get_wills_by_owner(&owner, &None, &100);
    assert_eq!(owner_wills.len(), 2);
    let beneficiary_wills = client.get_wills_by_beneficiary(&beneficiary, &None, &100);
    assert_eq!(beneficiary_wills.len(), 2);
    assert!(beneficiary_wills.iter().any(|w| w.id == clone_id));
}

// ── guardian cooldown ────────────────────────────────────────────────────────

#[test]
#[should_panic]
fn test_guardian_cooldown_blocks_trigger() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env, g1.clone(), g2],
    );
    advance_time(&env, 8 * DAY);
    client.guardian_trigger(&will_id, &g1);
    client.update_guardians(&will_id, &owner, &vec![&env, g1.clone()]);
    // Immediately try to trigger — cooldown is active.
    advance_time(&env, 1 * DAY);
    client.guardian_trigger(&will_id, &g1);
}

#[test]
fn test_guardian_cooldown_allows_after_period() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env, g1.clone(), g2],
    );
    advance_time(&env, 8 * DAY);
    client.guardian_trigger(&will_id, &g1);
    client.update_guardians(&will_id, &owner, &vec![&env, g1.clone()]);
    advance_time(&env, 8 * DAY);
    client.guardian_trigger(&will_id, &g1);
    assert_eq!(client.get_will(&will_id).guardian_votes, 1);
}

#[test]
fn test_initial_guardian_cooldown() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let g1 = Address::generate(&env);
    let g2 = Address::generate(&env);
    let will_id = client.create_will(
        &owner,
        &vec![&env, (token_address, 1_000_000_i128)],
        &vec![&env, bp(&beneficiary, 10_000)],
        &90, &7, &vec![&env, g1.clone(), g2],
    );
    // Will just created — cooldown should be active.
    let result = client.try_guardian_trigger(&will_id, &g1);
    assert!(result.is_err());
}

// ── batch_create_wills ──────────────────────────────────────────────────────

#[test]
fn test_batch_create_wills() {
    let (env, client, owner, token, token_address) = setup();
    let b1 = Address::generate(&env);
    let b2 = Address::generate(&env);
    let b3 = Address::generate(&env);
    let ids = client.batch_create_wills(
        &owner,
        &vec![
            &env,
            (
                vec![&env, (token_address.clone(), 100_000_i128)].into(),
                vec![&env, bp(&b1, 10_000)].into(),
                90u64,
                7u64,
                vec![&env].into(),
            ),
            (
                vec![&env, (token_address.clone(), 200_000_i128)].into(),
                vec![&env, bp(&b2, 10_000)].into(),
                30u64,
                3u64,
                vec![&env].into(),
            ),
            (
                vec![&env, (token_address.clone(), 300_000_i128)].into(),
                vec![&env, bp(&b3, 10_000)].into(),
                60u64,
                5u64,
                vec![&env].into(),
            ),
        ],
    );
    assert_eq!(ids.len(), 3);
    assert_eq!(ids.get(0).unwrap(), 1);
    assert_eq!(ids.get(1).unwrap(), 2);
    assert_eq!(ids.get(2).unwrap(), 3);
    assert_eq!(token.balance(&client.address), 600_000);
    let w1 = client.get_will(&1);
    assert_eq!(w1.checkin_period_days, 90);
    let w2 = client.get_will(&2);
    assert_eq!(w2.checkin_period_days, 30);
    let w3 = client.get_will(&3);
    assert_eq!(w3.checkin_period_days, 60);
}

#[test]
#[should_panic]
fn test_batch_empty_rejected() {
    let (env, client, owner, _token, _token_address) = setup();
    client.batch_create_wills(&owner, &Vec::new(&env));
}

#[test]
#[should_panic]
fn test_batch_too_many_rejected() {
    let (env, client, owner, _token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let specs: SorobanVec<_> = (0..11)
        .map(|_| {
            (
                vec![&env, (token_address.clone(), 100_000_i128)].into(),
                vec![&env, bp(&beneficiary, 10_000)].into(),
                90u64,
                7u64,
                vec![&env].into(),
            )
        })
        .collect();
    client.batch_create_wills(&owner, &specs);
}

#[test]
fn test_batch_transfers_tokens() {
    let (env, client, owner, token, token_address) = setup();
    let beneficiary = Address::generate(&env);
    let ids = client.batch_create_wills(
        &owner,
        &vec![
            &env,
            (
                vec![&env, (token_address.clone(), 400_000_i128)].into(),
                vec![&env, bp(&beneficiary, 10_000)].into(),
                90u64,
                7u64,
                vec![&env].into(),
            ),
            (
                vec![&env, (token_address.clone(), 600_000_i128)].into(),
                vec![&env, bp(&beneficiary, 10_000)].into(),
                90u64,
                7u64,
                vec![&env].into(),
            ),
        ],
    );
    assert_eq!(ids.len(), 2);
    assert_eq!(token.balance(&owner), 0);
    assert_eq!(token.balance(&client.address), 1_000_000);
}
