//! Contract events published by SoroWill.
//!
//! Every state-changing entry point publishes exactly one event so that
//! off-chain indexers (such as the SoroWill SDK/app) can reconstruct will
//! history without re-simulating transactions.

use soroban_sdk::{symbol_short, Address, Env};

use crate::types::GuardianVoteReason;

/// Published when a new will is created.
pub fn will_created(
    env: &Env,
    will_id: u64,
    owner: &Address,
    balance: i128,
    beneficiaries_count: u32,
    checkin_deadline: u64,
) {
    env.events().publish(
        (symbol_short!("created"), will_id),
        (
            owner.clone(),
            balance,
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
pub fn inheritance_released(
    env: &Env,
    will_id: u64,
    total_released: i128,
    beneficiaries_count: u32,
) {
    env.events().publish(
        (symbol_short!("released"), will_id),
        (total_released, beneficiaries_count),
    );
}

/// Published when the owner cancels the will and withdraws the balance.
pub fn will_cancelled(env: &Env, will_id: u64, owner: &Address, refund_amount: i128) {
    env.events().publish(
        (symbol_short!("cancelled"), will_id),
        (owner.clone(), refund_amount),
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

/// Published when the owner tops up the will's balance.
pub fn top_up(env: &Env, will_id: u64, owner: &Address, amount: i128, new_balance: i128) {
    env.events().publish(
        (symbol_short!("topup"), will_id),
        (owner.clone(), amount, new_balance),
    );
}

/// Published each time a guardian votes to trigger an early release.
/// Includes the reason code for transparency and dispute review.
pub fn guardian_voted(
    env: &Env,
    will_id: u64,
    guardian: &Address,
    votes_so_far: u32,
    reason: GuardianVoteReason,
) {
    env.events().publish(
        (symbol_short!("gvote"), will_id),
        (guardian.clone(), votes_so_far, reason),
    );
}

/// Published when a grace-tier payout is released to beneficiaries.
pub fn tier_released(
    env: &Env,
    will_id: u64,
    tier_index: u32,
    amount: i128,
    released_basis_points: u32,
) {
    env.events().publish(
        (symbol_short!("tier_rel"), will_id),
        (tier_index, amount, released_basis_points),
    );
}

/// Published when a batch check-in is performed across multiple wills.
pub fn batch_checkin(env: &Env, owner: &Address, will_ids: &soroban_sdk::Vec<u64>, count: u32) {
    env.events().publish(
        (symbol_short!("batchck"),),
        (owner.clone(), will_ids.clone(), count),
    );
}
