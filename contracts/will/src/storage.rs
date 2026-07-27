//! Persistent storage helpers for the SoroWill contract.
//!
//! All will state lives in persistent storage keyed by a `DataKey`. Owner and
//! beneficiary indexes are maintained as `Vec<u64>` of will ids so the
//! contract can answer `get_wills_by_owner` / `get_wills_by_beneficiary`
//! without an off-chain indexer. Guardian votes are tracked per
//! `(will_id, guardian)` pair so they can be cleared independently when a
//! guardian-release cycle resets.

use soroban_sdk::{contracttype, Address, Env, Vec};

use crate::errors::WillError;
use crate::types::{Will, WillStatus};

/// Ledgers correspond to roughly 5 seconds on the Stellar network, so one day
/// is approximately 17,280 ledgers.
const DAY_IN_LEDGERS: u32 = 17_280;

/// Extend TTL once remaining lifetime drops below this many ledgers.
const LIFETIME_THRESHOLD: u32 = DAY_IN_LEDGERS * 30;

/// Extend TTL out to this many ledgers when a bump is triggered.
const BUMP_AMOUNT: u32 = DAY_IN_LEDGERS * 60;

#[contracttype]
#[derive(Clone)]
pub(crate) enum DataKey {
    /// Monotonically increasing counter used to allocate will ids.
    NextWillId,
    /// Full state of a will, keyed by its id.
    Will(u64),
    /// List of will ids owned by an address.
    OwnerWills(Address),
    /// List of will ids an address is named as a beneficiary of.
    BeneficiaryWills(Address),
    /// Whether a guardian has already voted in the current trigger cycle.
    GuardianVote(u64, Address),
}

/// Allocates and returns the next available will id, starting at `1`.
pub fn next_will_id(env: &Env) -> u64 {
    let key = DataKey::NextWillId;
    let current: u64 = env.storage().instance().get(&key).unwrap_or(0);
    let next = current + 1;
    env.storage().instance().set(&key, &next);
    next
}

/// Persists a will's state, refreshing its storage TTL unless the will has
/// reached a terminal state.
///
/// `Released` and `Cancelled` are terminal: every entry point that could touch
/// a will again first asserts it is `Active` or `Triggered`, so nothing can
/// ever mutate one. Extending the TTL of a terminal will buys 60 more days of
/// rent for an entry that only `get_will` can read, at a price proportional to
/// its size. The entry is still written — readers keep seeing the final state
/// for the remainder of the TTL bought by the will's last active operation —
/// it just stops being renewed.
pub fn save_will(env: &Env, will: &Will) {
    let key = DataKey::Will(will.id);
    env.storage().persistent().set(&key, will);
    if !matches!(will.status, WillStatus::Released | WillStatus::Cancelled) {
        env.storage()
            .persistent()
            .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);
    }
}

/// Loads a will by id, returning `WillError::WillNotFound` if it does not exist.
pub fn load_will(env: &Env, will_id: u64) -> Result<Will, WillError> {
    let key = DataKey::Will(will_id);
    env.storage()
        .persistent()
        .get(&key)
        .ok_or(WillError::WillNotFound)
}

/// Adds `will_id` to the index list stored at `key`, if not already present.
///
/// The write and the TTL bump are skipped when the id is already indexed: the
/// resulting entry would be byte-for-byte identical, so paying to store it
/// again buys nothing.
fn index_push(env: &Env, key: DataKey, will_id: u64) {
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

/// Adds `will_id` to the index of wills owned by `owner`, if not already present.
pub fn index_by_owner(env: &Env, owner: &Address, will_id: u64) {
    index_push(env, DataKey::OwnerWills(owner.clone()), will_id);
}

/// Adds `will_id` to the index of wills `beneficiary` is named in, if not already present.
pub fn index_by_beneficiary(env: &Env, beneficiary: &Address, will_id: u64) {
    index_push(env, DataKey::BeneficiaryWills(beneficiary.clone()), will_id);
}

/// Removes `will_id` from the index of wills `beneficiary` is named in.
///
/// Used by `update_beneficiaries` to keep the reverse index accurate when a
/// beneficiary is dropped from a will. Returns without writing when the id is
/// not in the list, so an address that was never indexed costs a read instead
/// of a read plus a pointless rewrite.
pub fn remove_beneficiary_index(env: &Env, beneficiary: &Address, will_id: u64) {
    let key = DataKey::BeneficiaryWills(beneficiary.clone());
    let Some(mut ids) = env.storage().persistent().get::<_, Vec<u64>>(&key) else {
        return;
    };
    let Some(index) = ids.first_index_of(will_id) else {
        return;
    };
    // `first_index_of` just proved the index is in bounds.
    ids.remove_unchecked(index);
    env.storage().persistent().set(&key, &ids);
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

/// Returns whether `guardian` has already voted in the current trigger cycle for `will_id`.
pub fn has_guardian_voted(env: &Env, will_id: u64, guardian: &Address) -> bool {
    let key = DataKey::GuardianVote(will_id, guardian.clone());
    env.storage().persistent().get(&key).unwrap_or(false)
}

/// Records that `guardian` has voted in the current trigger cycle for `will_id`.
pub fn set_guardian_voted(env: &Env, will_id: u64, guardian: &Address) {
    let key = DataKey::GuardianVote(will_id, guardian.clone());
    env.storage().persistent().set(&key, &true);
    env.storage()
        .persistent()
        .extend_ttl(&key, LIFETIME_THRESHOLD, BUMP_AMOUNT);
}

/// Clears all guardian votes cast against `will`, starting a fresh voting cycle.
///
/// Called whenever a will returns to `Active` (e.g. via `emergency_checkin`)
/// so that guardians can vote again in a subsequent incapacitation event.
///
/// `will.guardian_votes` is incremented in lockstep with every
/// [`set_guardian_voted`] and zeroed alongside every reset, so a zero count
/// means no `GuardianVote` entry exists for the current guardian list and the
/// removals — up to [`crate::MAX_GUARDIANS`] of them — can be skipped. This is
/// the common case: most wills never see a guardian vote at all.
pub fn reset_guardian_votes(env: &Env, will: &Will) {
    if will.guardian_votes == 0 {
        return;
    }
    for guardian in will.guardians.iter() {
        let key = DataKey::GuardianVote(will.id, guardian.clone());
        env.storage().persistent().remove(&key);
    }
}

// ── Paginated index lookups ──────────────────────────────────────────────

/// Returns a bounded page of will ids from a `Vec<u64>` index.
///
/// If `cursor` is `Some(id)`, results start *after* that id (exclusive).
/// `limit` is capped at [`MAX_PAGE_SIZE`].
pub fn paginate_ids(env: &Env, ids: &Vec<u64>, cursor: Option<u64>, limit: u32) -> Vec<u64> {
    let page_size = limit.min(MAX_PAGE_SIZE);
    let mut result = Vec::new(env);
    let mut skip = cursor.is_some();
    let cursor_val = cursor.unwrap_or(0);

    for id in ids.iter() {
        if skip {
            if id <= cursor_val {
                continue;
            }
            skip = false;
        }
        if result.len() >= page_size {
            break;
        }
        result.push_back(id);
    }
    result
}

/// Maximum number of wills returned per page.
pub const MAX_PAGE_SIZE: u32 = 50;
