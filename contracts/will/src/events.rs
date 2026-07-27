//! Contract events published by SoroWill.
//!
//! Every state-changing entry point publishes exactly one event so that
//! off-chain indexers (such as the SoroWill SDK/app) can reconstruct will
//! history without re-simulating transactions.

use soroban_sdk::{symbol_short, Address, Env, Vec};

/// Published when a new will is created.
///
/// `token_count` is the number of distinct tokens locked at creation time.
pub fn will_created(
    env: &Env,
    will_id: u64,
    owner: &Address,
    token_count: u32,
    beneficiaries_count: u32,
    checkin_deadline: u64,
) {
    env.events().publish(
        (symbol_short!("created"), will_id),
        (
            owner.clone(),
            token_count,
            beneficiaries_count,
            checkin_deadline,
        ),
    );
}

/// Published when the owner checks in, resetting the check-in deadline.
pub fn check_in(env: &Env, will_id: u64, owner: &Address, next_deadline: u64) {
    env.events().publish(
        (symbol_short!("checkin"), will_id),
        (owner.clone(), next_deadline),
    );
}

/// Published when a will is triggered after a missed check-in.
pub fn will_triggered(env: &Env, will_id: u64, grace_period_ends: u64) {
    env.events()
        .publish((symbol_short!("triggered"), will_id), grace_period_ends);
}

/// Published when the owner emergency-checks-in during the grace period,
/// cancelling the trigger.
pub fn emergency_checkin(env: &Env, will_id: u64, owner: &Address, next_deadline: u64) {
    env.events().publish(
        (symbol_short!("emerg"), will_id),
        (owner.clone(), next_deadline),
    );
}

/// Published when inheritance is released to all beneficiaries.
///
/// `token_count` is the number of distinct tokens distributed.
pub fn inheritance_released(
    env: &Env,
    will_id: u64,
    total_released: i128,
    breakdown: &Vec<(Address, u32, i128)>,
    guardian_triggered: bool,
) {
    env.events().publish(
        (symbol_short!("released"), will_id),
        (total_released, guardian_triggered, breakdown.clone()),
    token_count: u32,
    beneficiaries_count: u32,
) {
    env.events().publish(
        (symbol_short!("released"), will_id),
        (token_count, beneficiaries_count),
    );
}

/// Published when the owner cancels the will and withdraws all token balances.
///
/// `token_count` is the number of distinct tokens refunded.
pub fn will_cancelled(env: &Env, will_id: u64, owner: &Address, token_count: u32) {
    env.events().publish(
        (symbol_short!("cancelled"), will_id),
        (owner.clone(), token_count),
    );
}

/// Published when the owner updates the beneficiary list.
pub fn beneficiaries_updated(env: &Env, will_id: u64, owner: &Address) {
    env.events()
        .publish((symbol_short!("benefup"), will_id), owner.clone());
}

/// Published when the owner updates the guardian list.
pub fn guardians_updated(env: &Env, will_id: u64, owner: &Address) {
    env.events()
        .publish((symbol_short!("guardup"), will_id), owner.clone());
}

/// Published when the owner closes a Released will, marking it Settled.
pub fn will_closed(env: &Env, will_id: u64, owner: &Address) {
    env.events()
        .publish((symbol_short!("closed"), will_id), owner.clone());
}

/// Published when the owner tops up the will's balance.
pub fn top_up(env: &Env, will_id: u64, owner: &Address, amount: i128, new_balance: i128) {
/// Published when the owner tops up a specific token's balance in the will.
pub fn top_up(
    env: &Env,
    will_id: u64,
    owner: &Address,
    token: &Address,
    amount: i128,
    new_balance: i128,
) {
    env.events().publish(
        (symbol_short!("topup"), will_id),
        (owner.clone(), token.clone(), amount, new_balance),
    );
}

/// Published each time a guardian votes to trigger an early release.
pub fn guardian_voted(env: &Env, will_id: u64, guardian: &Address, weight: u32, total_weight: u32) {
    env.events().publish(
        (symbol_short!("gvote"), will_id),
        (guardian.clone(), weight, total_weight),
    );
}

/// Published when two wills are merged into one.
pub fn wills_merged(
    env: &Env,
    surviving_will_id: u64,
    consumed_will_id: u64,
    owner: &Address,
    new_balance: i128,
) {
    env.events().publish(
        (symbol_short!("merged"), surviving_will_id),
        (owner.clone(), consumed_will_id, new_balance),
/// Published when a will is migrated to a new schema version.
pub fn will_migrated(env: &Env, will_id: u64, owner: &Address, from_version: u32, to_version: u32) {
    env.events().publish(
        (symbol_short!("migrated"), will_id),
        (owner.clone(), from_version, to_version),
    );
}
