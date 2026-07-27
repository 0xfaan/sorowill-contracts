//! Persistent storage helpers for the SoroWill contract.
//!
//! All will state lives in persistent storage keyed by a `DataKey`. Owner and
//! beneficiary indexes are maintained as `Vec<u64>` of will ids so the
//! contract can answer `get_wills_by_owner` / `get_wills_by_beneficiary`
//! without an off-chain indexer. Guardian votes are tracked per
//! `(will_id, guardian)` pair with a timestamp so they can expire over time,
//! and cleared independently when a guardian-release cycle resets.

use soroban_sdk::{contracttype, Address, Env, Vec};

use crate::errors::WillError;
use crate::types::{GuardianVoteReason, Will};

/// Ledgers correspond to roughly 5 seconds on the Stellar network, so one day
/// is approximately 17,280 ledgers.
const DAY_IN_LEDGERS: u32 = 17_280;

/// Extend TTL once remaining lifetime drops below this many ledgers.
const LIFETIME_THRESHOLD: u32 = DAY_IN_LEDGERS * 30;

/// Extend TTL out to this many ledgers when a bump is triggered.
const BUMP_AMOUNT: u32 = DAY_IN_LEDGERS * 60;

/// Seconds in a day, used to convert day-denominated expiry windows.
const SECONDS_PER_DAY: u64 = 86_400;

#[contracttype]
#[derive(Clone)]
enum DataKey {
    /// Monotonically increasing counter used to allocate will ids.
    NextWillId,
    /// Full state of a will, keyed by its id.
    Will(u64),
    /// List of will ids owned by an address.
    OwnerWills(Address),
    /// List of will ids an address is named as a beneficiary of.
    BeneficiaryWills(Address),
    /// A guardian's vote record: stores `(timestamp, reason)` for time-weighted expiry.
    GuardianVote(u64, Address),
}

/// The data stored for each guardian vote: the Unix timestamp when the vote
/// was cast and the reason code the guardian provided.
#[contracttype]
#[derive(Clone, Debug)]
pub struct GuardianVoteRecord {
    pub timestamp: u64,
    pub reason: GuardianVoteReason,
}

/// Allocates and returns the next available will id, starting at `1`.
pub fn next_will_id(env: &Env) -> u64 {
    let key = DataKey::NextWillId;
    let current: u64 = env.storage().instance().get(&key).unwrap_or(0);
    let next = current + 1;
    env.storage().instance().set(&key, &next);
    next
}

/// Persists a will's state and refreshes its storage TTL.
pub fn save_will(env: &Env, will: &Will) {
    let key = DataKey::Will(will.id);
    env.storage().persistent().set(&key, will);
    env.storage()
        .persistent()
        .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);
}

/// Loads a will by id, returning `WillError::WillNotFound` if it does not exist.
pub fn load_will(env: &Env, will_id: u64) -> Result<Will, WillError> {
    let key = DataKey::Will(will_id);
    env.storage()
        .persistent()
        .get(&key)
        .ok_or(WillError::WillNotFound)
}

/// Adds `will_id` to the index of wills owned by `owner`, if not already present.
pub fn index_by_owner(env: &Env, owner: &Address, will_id: u64) {
    let key = DataKey::OwnerWills(owner.clone());
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    if !ids.contains(will_id) {
        ids.push_back(will_id);
        env.storage().persistent().set(&key, &ids);
        env.storage()
            .persistent()
            .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);
    }
}

/// Adds `will_id` to the index of wills `beneficiary` is named in, if not already present.
pub fn index_by_beneficiary(env: &Env, beneficiary: &Address, will_id: u64) {
    let key = DataKey::BeneficiaryWills(beneficiary.clone());
    let mut ids: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    if !ids.contains(will_id) {
        ids.push_back(will_id);
        env.storage().persistent().set(&key, &ids);
        env.storage()
            .persistent()
            .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);
    }
}

/// Removes `will_id` from the index of wills `beneficiary` is named in.
///
/// Used by `update_beneficiaries` to keep the reverse index accurate when a
/// beneficiary is dropped from a will.
pub fn remove_beneficiary_index(env: &Env, beneficiary: &Address, will_id: u64) {
    let key = DataKey::BeneficiaryWills(beneficiary.clone());
    if let Some(ids) = env.storage().persistent().get::<_, Vec<u64>>(&key) {
        let mut updated: Vec<u64> = Vec::new(env);
        for id in ids.iter() {
            if id != will_id {
                updated.push_back(id);
            }
        }
        env.storage().persistent().set(&key, &updated);
    }
}

/// Returns the list of will ids owned by `owner`.
pub fn get_owner_wills(env: &Env, owner: &Address) -> Vec<u64> {
    let key = DataKey::OwnerWills(owner.clone());
    env.storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env))
}

/// Returns the list of will ids `beneficiary` is named in.
pub fn get_beneficiary_wills(env: &Env, beneficiary: &Address) -> Vec<u64> {
    let key = DataKey::BeneficiaryWills(beneficiary.clone());
    env.storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env))
}

/// Returns the full vote record for `guardian` in the current trigger cycle
/// for `will_id`, or `None` if they have not voted.
pub fn get_guardian_vote(env: &Env, will_id: u64, guardian: &Address) -> Option<GuardianVoteRecord> {
    let key = DataKey::GuardianVote(will_id, guardian.clone());
    env.storage().persistent().get(&key)
}

/// Returns whether `guardian` has a non-expired vote in the current trigger
/// cycle for `will_id`. A vote is considered expired if `now - vote_timestamp`
/// exceeds `expiry_days * SECONDS_PER_DAY`.
pub fn has_guardian_voted(
    env: &Env,
    will_id: u64,
    guardian: &Address,
    now: u64,
    expiry_days: u64,
) -> bool {
    if let Some(record) = get_guardian_vote(env, will_id, guardian) {
        let expiry_secs = expiry_days * SECONDS_PER_DAY;
        now - record.timestamp <= expiry_secs
    } else {
        false
    }
}

/// Records that `guardian` has voted in the current trigger cycle for `will_id`,
/// storing the vote timestamp and reason code.
pub fn set_guardian_voted(
    env: &Env,
    will_id: u64,
    guardian: &Address,
    timestamp: u64,
    reason: GuardianVoteReason,
) {
    let key = DataKey::GuardianVote(will_id, guardian.clone());
    let record = GuardianVoteRecord { timestamp, reason };
    env.storage().persistent().set(&key, &record);
    env.storage()
        .persistent()
        .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);
}

/// Counts how many distinct guardians have non-expired votes for `will_id`.
pub fn count_valid_guardian_votes(
    env: &Env,
    will_id: u64,
    guardians: &Vec<Address>,
    now: u64,
    expiry_days: u64,
) -> u32 {
    let mut count = 0u32;
    for guardian in guardians.iter() {
        if has_guardian_voted(env, will_id, &guardian, now, expiry_days) {
            count += 1;
        }
    }
    count
}

/// Clears all guardian votes for `will_id`, starting a fresh voting cycle.
///
/// Called whenever a will returns to `Active` (e.g. via `emergency_checkin`)
/// so that guardians can vote again in a subsequent incapacitation event.
pub fn reset_guardian_votes(env: &Env, will_id: u64, guardians: &Vec<Address>) {
    for guardian in guardians.iter() {
        let key = DataKey::GuardianVote(will_id, guardian.clone());
        env.storage().persistent().remove(&key);
    }
}
