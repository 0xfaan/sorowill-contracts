// The deployed contract is `no_std`. The test and fuzzing harnesses need
// `std` (the Soroban test host, `proptest` and `libfuzzer-sys` all pull it
// in), so `std` is linked only for those configurations. The wasm build,
// which sees neither `cfg(test)` nor the `fuzzing` feature, stays `no_std`.
#![cfg_attr(not(any(test, feature = "fuzzing")), no_std)]

//! SoroWill — a trustless on-chain inheritance and dead man's switch protocol
//! for Stellar Soroban.
//!
//! An owner locks one or more tokens (e.g. USDC, XLM, any SEP-41 asset) into
//! a `Will`, names beneficiaries with percentage shares, and periodically
//! calls [`WillContract::check_in`] to prove they are still active. If the
//! owner misses a check-in deadline, anyone may call
//! [`WillContract::trigger_will`] to start a grace period. The owner can
//! still call [`WillContract::emergency_checkin`] during the grace period to
//! prove they are alive and reset the countdown. If the grace period elapses
//! without an emergency check-in, anyone may call
//! [`WillContract::release_inheritance`] to split every locked token balance
//! among the beneficiaries proportionally to their configured percentages.
//!
//! Optionally, up to three guardians may be named on a will; any two of them
//! calling [`WillContract::guardian_trigger`] force an immediate release,
//! bypassing the check-in/grace-period flow entirely (e.g. if the owner is
//! known to be incapacitated). Guardians must first accept their role via
//! [`WillContract::accept_guardian_role`] before they can vote.
//!
//! Two distribution modes are supported:
//! - **Push mode** (default): `distribute` transfers tokens directly to each
//!   beneficiary in a single call.
//! - **Pull mode**: `distribute` stores each beneficiary's share as a
//!   claimable amount. Beneficiaries call [`WillContract::claim_share`]
//!   independently to withdraw their share.
//! known to be incapacitated). Guardian votes expire after a configurable
//! window so stale votes cannot combine with fresh ones.
//!
//! Grace periods may optionally be split into multiple tiers, each releasing
//! a configurable percentage of the balance at a different time offset.

mod errors;
mod events;
mod storage;
mod types;

/// Resource-cost profile for every public entry point. Measurement rather
/// than assertion — see the module docs for how to read the numbers.
#[cfg(test)]
mod profile;
/// Reusable harness that drives entry points with arbitrary input and asserts
/// the contract's invariants. Shared by the `proptest` suite in
/// [`fuzz_test`] and by the `cargo-fuzz` targets under `fuzz/`.
#[cfg(any(test, feature = "fuzzing"))]
pub mod fuzz_harness;

#[cfg(test)]
mod fuzz_test;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, panic_with_error, symbol_short, token, Address, Env, Map, Vec};
/// Malicious/reentrant SEP-41 token mock used for reentrancy regression
/// testing. See the module docs for details.
#[cfg(test)]
mod test_support;

use soroban_sdk::{contract, contractimpl, panic_with_error, symbol_short, token, Address, Env, Vec};

pub use errors::WillError;
pub use types::{Beneficiary, Guardian, GuardianVoteReason, ProtocolStats, Will, WillStatus, WillStatusTransition};

/// Semantic version of the contract logic, encoded as
/// `major * 1_000_000 + minor * 1_000 + patch`.
///
/// Bump this constant in every PR that changes observable contract behaviour
/// so that SDKs and apps can detect version mismatches at runtime via
/// [`WillContract::get_contract_version`].
///
/// Current baseline: **1.0.0** → `1_000_000`.
pub const CONTRACT_VERSION: u32 = 1_000_000;

/// Number of seconds in a day, used to convert the day-denominated periods
/// stored on a `Will` into absolute ledger timestamps.
const SECONDS_PER_DAY: u64 = 86_400;

/// Maximum number of beneficiaries a single will may have.
const MAX_BENEFICIARIES: u32 = 10;

/// Maximum number of guardians a single will may have.
const MAX_GUARDIANS: u32 = 3;

/// Maximum length, in days, of a will's check-in or grace period (10 years).
///
/// Periods are converted to absolute timestamps by multiplying by
/// [`SECONDS_PER_DAY`]. Bounding them here guarantees that conversion can
/// never overflow the `u64` ledger timestamp, which would otherwise panic
/// outright — or, worse, produce a will whose deadline is unreachable, so
/// that `trigger_will` can never run and the locked balance can never be
/// released.
const MAX_PERIOD_DAYS: u64 = 3_650;
/// Maximum number of distinct tokens a single will may hold.
const MAX_TOKENS: u32 = 10;

/// Number of distinct guardian votes required to force an early release.
const GUARDIAN_THRESHOLD: u32 = 2;

/// Minimum number of wills that can be created in a single batch call.
const BATCH_MIN: u32 = 1;

/// Maximum number of wills that can be created in a single batch call.
const BATCH_MAX: u32 = 10;

/// Cooldown period in days after a guardian-list change before `guardian_trigger`
/// takes effect. Prevents a compromised owner from swapping guardians right
/// before attempting something malicious.
const GUARDIAN_COOLDOWN_DAYS: u64 = 7;

/// Maximum keeper bounty in basis points (100 bps = 1%).
const MAX_KEEPER_BOUNTY_BPS: u32 = 100;

#[contract]
pub struct WillContract;

/// Current contract schema version. Must match storage::CURRENT_SCHEMA_VERSION.
const CURRENT_SCHEMA_VERSION: u32 = 1;

#[contractimpl]
impl WillContract {
    /// Creates a new will, locking one or more token balances in the contract.
    ///
    /// # Parameters
    /// - `owner`: the address creating the will; must authorize this call.
    /// - `tokens`: a list of `(token_address, amount)` pairs to lock. Each
    ///   token address must be unique, each amount must be positive, and the
    ///   list must contain between 1 and `MAX_TOKENS` entries.
    /// - `beneficiaries`: 1 to `MAX_BENEFICIARIES` entries whose basis points
    ///   sum to exactly 10,000.
    /// - `checkin_period_days`: how many days the owner may go without checking
    ///   in; 1 to `MAX_PERIOD_DAYS`.
    /// - `grace_period_days`: how many days after being triggered the owner has
    ///   to prove they are alive; 1 to `MAX_PERIOD_DAYS`.
    /// - `guardians`: 0 to `MAX_GUARDIANS` distinct addresses that may jointly
    ///   force an early release.
    /// - `guardian_threshold`: number of guardian votes required to trigger.
    ///   Must be between 1 and `guardians.len()`. Ignored when `guardians` is empty.
    /// - `keeper_bounty_bps`: optional keeper bounty in basis points.
    ///
    /// # Returns
    /// The newly allocated will id.
    ///
    /// # Panics
    /// - [`WillError::ZeroAmount`] if any token amount is not positive.
    /// - [`WillError::TooManyBeneficiaries`] if the beneficiary/guardian/token lists are
    ///   empty or exceed their respective caps.
    /// - [`WillError::InvalidPercentages`] if beneficiary basis points do not sum to 10,000.
    /// - [`WillError::DuplicateBeneficiary`] if the same address is supplied twice.
    /// - [`WillError::DuplicateGuardian`] if the same guardian is supplied twice.
    /// - [`WillError::InvalidPeriod`] if either period is zero or exceeds
    ///   [`MAX_PERIOD_DAYS`].
    #[allow(clippy::too_many_arguments)]
    pub fn create_will(
        env: Env,
        owner: Address,
        tokens: Vec<(Address, i128)>,
        beneficiaries: Vec<Beneficiary>,
        checkin_period_days: u64,
        grace_period_days: u64,
        guardians: Vec<Address>,
        guardian_threshold: u32,
        keeper_bounty_bps: Option<u32>,
    ) -> u64 {
        owner.require_auth();

        // Validate keeper bounty if provided
        let keeper_bounty = match keeper_bounty_bps {
            Some(bps) if bps <= MAX_KEEPER_BOUNTY_BPS => bps,
            Some(_) => panic_with_error!(&env, WillError::KeeperBountyExceedsMax),
            None => 0,
        };

        if tokens.is_empty() || tokens.len() > MAX_TOKENS {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }
        if beneficiaries.is_empty() || beneficiaries.len() > MAX_BENEFICIARIES {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }
        assert_valid_percentages(&env, &beneficiaries);
        assert_valid_guardians(&env, &owner, &guardians);
        assert_valid_periods(&env, checkin_period_days, grace_period_days);

        // Validate guardian threshold when guardians are present.
        if !guardians.is_empty() {
            let threshold_range = 1..=guardians.len() as u32;
            if !threshold_range.contains(&guardian_threshold) {
                panic_with_error!(&env, WillError::InvalidGuardianThreshold);
            }
        }

        // Convert addresses to Guardian structs with weight 1 and build balances.
        let mut guardian_structs: Vec<Guardian> = Vec::new(&env);
        for addr in guardians.iter() {
            guardian_structs.push_back(Guardian {
                address: addr,
                weight: 1,
            });
        }

        // Validate amounts and build the balances map.
        let mut balances: Map<Address, i128> = Map::new(&env);
        for (token_addr, amount) in tokens.iter() {
            if amount <= 0 {
                panic_with_error!(&env, WillError::ZeroAmount);
            }
            // Transfer this token from the owner into the contract.
            token::Client::new(&env, &token_addr).transfer(
                &owner,
                &env.current_contract_address(),
                &amount,
            );
            // Accumulate in case the caller somehow duplicated the same token
            // address twice — treat it as an additive top-up rather than
            // silently overwriting.
            let prev = balances.get(token_addr.clone()).unwrap_or(0);
            balances.set(token_addr, prev + amount);
        }

        let will_id = storage::next_will_id(&env);
        let now = env.ledger().timestamp();

        let beneficiaries_count = beneficiaries.len();
        let token_count = balances.len();
        for beneficiary in beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &beneficiary.address, will_id);
        }

        let will = Will {
            id: will_id,
            owner: owner.clone(),
            balances,
            beneficiaries,
            checkin_period_days,
            grace_period_days,
            last_checkin: now,
            trigger_time: None,
            status: WillStatus::Active,
            guardians: guardian_structs,
            guardian_vote_weight: 0,
            guardian_votes: 0,
            guardian_threshold,
            guardian_list_updated_at: now,
            schema_version: CURRENT_SCHEMA_VERSION,
            keeper_bounty_bps: keeper_bounty,
        };
        storage::save_will(&env, &will);
        storage::index_by_owner(&env, &owner, will_id);
        storage::increment_active_will_count(&env);

        record_transition(
            &env,
            will_id,
            WillStatus::Active,
            WillStatus::Active,
            &owner,
            symbol_short!("create"),
        );

        events::will_created(
            &env,
            will_id,
            &owner,
            token_count,
            beneficiaries_count,
            now + checkin_period_days * SECONDS_PER_DAY,
        );

        will_id
    }

    /// Resets the check-in countdown for `will_id`. Must be called by the
    /// will's owner or the designated delegate, and the will must be `Active`.
    pub fn check_in(env: Env, will_id: u64, caller: Address) {
        caller.require_auth();
        let mut will = load_will(&env, will_id);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);
        assert_owner_or_delegate(&env, &will, &caller);

        let now = env.ledger().timestamp();
        will.last_checkin = now;
        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        events::check_in(&env, will_id, &caller, next_deadline);
    }

    /// Sets or replaces the delegate address for `will_id`. Only callable
    /// by the owner while the will is `Active`. Pass `None` to clear.
    pub fn set_delegate(env: Env, will_id: u64, owner: Address, delegate: Option<Address>) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        will.delegate = delegate.clone();
        storage::save_will(&env, &will);

        if let Some(ref addr) = delegate {
            events::delegate_set(&env, will_id, &owner, addr);
        } else {
            events::delegate_cleared(&env, will_id, &owner);
        }
    }

    /// Batch check-in across multiple wills in a single transaction.
    /// All wills must be owned by `owner` and in `Active` status.
    /// Panics if any will ID is invalid, not owned by `owner`, or not `Active`.
    pub fn batch_check_in(env: Env, will_ids: Vec<u64>, owner: Address) {
        owner.require_auth();
        let now = env.ledger().timestamp();
        let count = will_ids.len();

        for will_id in will_ids.iter() {
            let mut will = load_owned(&env, will_id, &owner);
            assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

            will.last_checkin = now;
            let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
            storage::save_will(&env, &will);

            events::check_in(&env, will_id, &owner, next_deadline);
        }

        events::batch_checkin(&env, &owner, &will_ids, count);
    }

    /// Starts the grace period for `will_id` once the check-in deadline has
    /// passed. Callable by anyone: proving a missed deadline requires no
    /// special authorization, which lets any off-chain "keeper" trigger a
    /// stalled will.
    ///
    /// # Panics
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::CheckinNotDue`] if the check-in deadline has not passed yet.
    pub fn trigger_will(env: Env, will_id: u64) {
        let mut will = load_will(&env, will_id);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        let now = env.ledger().timestamp();
        let deadline = will.last_checkin + will.checkin_period_days * SECONDS_PER_DAY;
        if now < deadline {
            panic_with_error!(&env, WillError::CheckinNotDue);
        }

        will.status = WillStatus::Triggered;
        will.trigger_time = Some(now);
        will.trigger_balance = will.balance;
        will.released_basis_points = 0;
        let grace_period_ends = now + will.grace_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        storage::index_triggered_will(&env, will_id);

        record_transition(
            &env,
            will_id,
            WillStatus::Active,
            WillStatus::Triggered,
            &env.current_contract_address(),
            symbol_short!("trigger"),
        );

        events::will_triggered(&env, will_id, grace_period_ends);
    }

    /// Cancels an in-progress trigger during the grace period, proving the
    /// owner is alive, and resets the check-in countdown. Also clears any
    /// guardian votes cast during the cycle being cancelled.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotTriggered`] if the will is not `Triggered`.
    /// - [`WillError::GracePeriodExpired`] if the grace period has already elapsed.
    pub fn emergency_checkin(env: Env, will_id: u64, owner: Address) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(
            &env,
            &will,
            WillStatus::Triggered,
            WillError::WillNotTriggered,
        );

        let trigger_time = will.trigger_time.unwrap_or(0);
        let grace_deadline = trigger_time + will.grace_period_days * SECONDS_PER_DAY;
        let now = env.ledger().timestamp();
        if now > grace_deadline {
            panic_with_error!(&env, WillError::GracePeriodExpired);
        }

        // Clear the vote markers before zeroing the counter: the counter is
        // what tells `reset_guardian_votes` whether there is anything to clear.
        storage::reset_guardian_votes(&env, &will);

        will.status = WillStatus::Active;
        will.trigger_time = None;
        will.last_checkin = now;
        will.guardian_vote_weight = 0;
        storage::save_will(&env, &will);

        storage::unindex_triggered_will(&env, will_id);

        record_transition(
            &env,
            will_id,
            WillStatus::Triggered,
            WillStatus::Active,
            &owner,
            symbol_short!("emerg"),
        );

        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        events::emergency_checkin(&env, will_id, &owner, next_deadline);
    }

    /// Distributes all token balances to beneficiaries proportionally to
    /// their configured percentages. Callable by anyone once the grace
    /// period has fully elapsed.
    ///
    /// If a vesting schedule is configured, instead of releasing everything
    /// at once, this transitions the will to `Vesting` status and records
    /// the start time. Beneficiaries then call `claim_vested` to unlock
    /// their share gradually.
    ///
    /// If no vesting schedule is configured, behaves exactly as before:
    /// full lump-sum release.
    ///
    /// In push mode (the default), tokens are transferred directly to each
    /// beneficiary. In pull mode (`pull_distribution = true`), shares are
    /// stored in claimable-shares storage and beneficiaries must call
    /// `claim_share` to withdraw.
    ///
    /// # Panics
    /// - [`WillError::WillNotTriggered`] if the will is not `Triggered`.
    /// - [`WillError::GracePeriodNotExpired`] if the grace period has not elapsed yet.
    pub fn release_inheritance(env: Env, will_id: u64, caller: Option<Address>) {
        let mut will = load_will(&env, will_id);
        assert_status(
            &env,
            &will,
            WillStatus::Triggered,
            WillError::WillNotTriggered,
        );

        let trigger_time = will.trigger_time.unwrap_or(0);
        let grace_deadline = trigger_time + will.grace_period_days * SECONDS_PER_DAY;
        let now = env.ledger().timestamp();
        if now < grace_deadline {
            panic_with_error!(&env, WillError::GracePeriodNotExpired);
        }

        record_transition(
            &env,
            will_id,
            WillStatus::Triggered,
            WillStatus::Released,
            &env.current_contract_address(),
            symbol_short!("release"),
        );

        distribute(&env, &mut will, &caller);
    }

    /// Cancels the will and refunds every locked token balance to the owner.
    /// Only possible while the will is `Active`, i.e. before it has ever
    /// been triggered by a missed check-in (an owner who is mid-grace-period
    /// must first call `emergency_checkin` to return the will to `Active`).
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    pub fn cancel_will(env: Env, will_id: u64, owner: Address) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        // Snapshot the balances before mutating state (checks-effects-interactions).
        let refund = will.balance;
        let contract_address = env.current_contract_address();
        let token_count = will.balances.len();
        // Capture balances for transfer after state is committed.
        let balances_snapshot = will.balances.clone();

        // --- EFFECTS: mutate state and persist before any external calls ---
        storage::decrement_active_will_count(&env);
        storage::adjust_locked_value(&env, &will.token, -refund);

        will.balance = 0;
        will.balances = Map::new(&env);
        will.status = WillStatus::Cancelled;

        // Prune stale index entries (#70): remove the will from the owner
        // index and from every beneficiary's reverse index so that
        // get_wills_by_owner / get_wills_by_beneficiary no longer return it.
        storage::remove_owner_index(&env, &owner, will_id);
        for beneficiary in will.beneficiaries.iter() {
            storage::remove_beneficiary_index(&env, &beneficiary.address, will_id);
        }

        storage::save_will(&env, &will);

        record_transition(
            &env,
            will_id,
            WillStatus::Active,
            WillStatus::Cancelled,
            &owner,
            symbol_short!("cancel"),
        );

        // --- INTERACTIONS: external token transfers happen after state is settled ---
        for (token_addr, balance) in balances_snapshot.iter() {
            if balance > 0 {
                token::Client::new(&env, &token_addr).transfer(
                    &contract_address,
                    &owner,
                    &balance,
                );
            }
        }

        events::will_cancelled(&env, will_id, &owner, token_count);
    }

    /// Explicitly marks a `Released` will as `Settled`, completing the
    /// archival step separate from the payout moment. Only the owner may
    /// close a will, and only after it has been released.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotReleased`] if the will is not `Released`.
    pub fn close_will(env: Env, will_id: u64, owner: Address) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(
            &env,
            &will,
            WillStatus::Released,
            WillError::WillNotReleased,
        );

        will.status = WillStatus::Settled;
        storage::save_will(&env, &will);

        events::will_closed(&env, will_id, &owner);
    }

    /// Replaces the beneficiary list for `will_id`. Only possible while the
    /// will is `Active`. The new basis points must sum to exactly 10,000.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::TooManyBeneficiaries`] if the new list is empty or too large.
    /// - [`WillError::InvalidPercentages`] if the new basis points do not sum to 10,000.
    /// - [`WillError::DuplicateBeneficiary`] if the same address is supplied twice.
    pub fn update_beneficiaries(
        env: Env,
        will_id: u64,
        owner: Address,
        beneficiaries: Vec<Beneficiary>,
    ) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        if beneficiaries.is_empty() || beneficiaries.len() > MAX_BENEFICIARIES {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }
        assert_valid_percentages(&env, &beneficiaries);

        // Only addresses that actually join or leave the will need their
        // reverse index touched. Unconditionally removing every old address
        // and re-adding every new one costs a storage read and write per
        // address even when the lists are identical — which is the common
        // case, since most updates only re-cut the percentages. Membership is
        // decided against the two lists already in memory, at no storage cost.
        for old in will.beneficiaries.iter() {
            if !names_address(&beneficiaries, &old.address) {
                storage::remove_beneficiary_index(&env, &old.address, will_id);
            }
        }
        for new_beneficiary in beneficiaries.iter() {
            if !names_address(&will.beneficiaries, &new_beneficiary.address) {
                storage::index_by_beneficiary(&env, &new_beneficiary.address, will_id);
            }
        }

        will.beneficiaries = beneficiaries;
        storage::save_will(&env, &will);

        events::beneficiaries_updated(&env, will_id, &owner);
    }

    /// Allows a beneficiary to renounce their inheritance share in advance.
    /// The renouncing beneficiary is removed from the beneficiary list, and
    /// their percentage is redistributed proportionally among the remaining
    /// beneficiaries.
    ///
    /// Only callable by a named beneficiary while the will is in `Active` or
    /// `Triggered` status. After renunciation, the will is saved but status
    /// transitions are not recorded (it's a beneficiary action, not a status change).
    ///
    /// # Parameters
    /// - `will_id`: the will to renounce beneficiary status from
    /// - `beneficiary`: the address renouncing their share; must authorize this call
    ///   and must be named as a beneficiary in the will
    ///
    /// # Panics
    /// - [`WillError::WillNotFound`] if the will does not exist.
    /// - [`WillError::BeneficiaryNotFound`] if `beneficiary` is not named in the will.
    /// - [`WillError::WillNotActive`] if the will is not in `Active` or `Triggered` status.
    /// - [`WillError::InvalidPercentages`] if redistribution would result in invalid
    ///   percentages (shouldn't happen with a single beneficiary, but caught for safety).
    pub fn renounce_beneficiary(env: Env, will_id: u64, beneficiary: Address) {
        beneficiary.require_auth();
        let mut will = load_will(&env, will_id);

        // Allow renunciation only in Active or Triggered status
        if will.status != WillStatus::Active && will.status != WillStatus::Triggered {
            panic_with_error!(&env, WillError::WillNotActive);
        }

        // Find and remove the renouncing beneficiary
        let mut found_index: Option<usize> = None;
        let mut renounced_basis_points: u32 = 0;
        for (index, b) in will.beneficiaries.iter().enumerate() {
            if b.address == beneficiary {
                found_index = Some(index);
                renounced_basis_points = b.basis_points;
                break;
            }
        }

        let index = match found_index {
            Some(i) => i,
            None => panic_with_error!(&env, WillError::BeneficiaryNotFound),
        };

        // Create new beneficiary list without the renouncing beneficiary
        let mut new_beneficiaries: Vec<Beneficiary> = Vec::new(&env);
        for (i, b) in will.beneficiaries.iter().enumerate() {
            if i != index {
                new_beneficiaries.push_back(b);
            }
        }

        // If there are remaining beneficiaries, redistribute the renounced share
        if !new_beneficiaries.is_empty() {
            // Redistribute the renounced basis points proportionally to remaining beneficiaries
            let remaining_basis_points: u32 = new_beneficiaries
                .iter()
                .map(|b| b.basis_points)
                .fold(0u32, |acc, bp| acc.saturating_add(bp));

            if remaining_basis_points > 0 {
                let mut updated_beneficiaries: Vec<Beneficiary> = Vec::new(&env);
                let mut total_redistributed: u32 = 0;

                for (i, beneficiary_entry) in new_beneficiaries.iter().enumerate() {
                    let share_of_renounced = if i as u32 == (new_beneficiaries.len() - 1) as u32 {
                        // Last beneficiary gets any rounding remainder
                        renounced_basis_points - total_redistributed
                    } else {
                        // Proportional share of the renounced percentage
                        let portion = (renounced_basis_points as u128
                            * beneficiary_entry.basis_points as u128)
                            / remaining_basis_points as u128;
                        total_redistributed = total_redistributed.saturating_add(portion as u32);
                        portion as u32
                    };

                    updated_beneficiaries.push_back(Beneficiary {
                        address: beneficiary_entry.address.clone(),
                        basis_points: beneficiary_entry.basis_points.saturating_add(share_of_renounced),
                    });
                }

                will.beneficiaries = updated_beneficiaries;
            } else {
                will.beneficiaries = new_beneficiaries;
            }
        } else {
            // If this was the only beneficiary, the will now has no beneficiaries
            // This is an edge case but we allow it (distribution would fail later)
            will.beneficiaries = new_beneficiaries;
        }

        // Update indexes: remove the renouncing beneficiary from the reverse index
        storage::remove_beneficiary_index(&env, &beneficiary, will_id);

        // Get the owner for event emission (not changed)
        let owner = will.owner.clone();
        storage::save_will(&env, &will);

        events::beneficiary_renounced(&env, will_id, &beneficiary, &owner);
    }

    /// Replaces the guardian list for `will_id`. Only possible while the will
    /// is `Active`. Any votes cast against the previous guardian list are
    /// cleared so every updated list starts a fresh voting cycle. Consent
    /// entries for the old guardians are also cleared.
    ///
    /// Records the current timestamp as `guardian_list_updated_at` so that
    /// [`guardian_trigger`] enforces a cooldown before the new list takes
    /// effect (see [`GUARDIAN_COOLDOWN_DAYS`]).
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::TooManyBeneficiaries`] if more than `MAX_GUARDIANS`
    ///   guardians are supplied.
    pub fn update_guardians(env: Env, will_id: u64, owner: Address, guardians: Vec<Address>) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        assert_valid_guardians(&env, &owner, &guardians);

        let now = env.ledger().timestamp();
        storage::reset_guardian_votes(&env, &will);
        let mut guardian_structs: Vec<Guardian> = Vec::new(&env);
        for addr in guardians.iter() {
            guardian_structs.push_back(Guardian {
                address: addr,
                weight: 1,
            });
        }
        will.guardians = guardian_structs;
        will.guardian_votes = 0;
        will.guardian_list_updated_at = now;
        will.guardian_vote_weight = 0;
        storage::save_will(&env, &will);

        events::guardians_updated(&env, will_id, &owner);
    }

    /// Updates the check-in and/or grace period for an active will.
    /// Only callable by the owner while the will is `Active`.
    ///
    /// # Parameters
    /// - `will_id`: the will to update
    /// - `owner`: the will's owner; must authorize this call
    /// - `checkin_period_days`: new check-in period (optional); if specified,
    ///   must be 1 to `MAX_PERIOD_DAYS`
    /// - `grace_period_days`: new grace period (optional); if specified,
    ///   must be 1 to `MAX_PERIOD_DAYS`
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::InvalidPeriod`] if either period is zero or exceeds
    ///   [`MAX_PERIOD_DAYS`].
    pub fn update_periods(
        env: Env,
        will_id: u64,
        owner: Address,
        checkin_period_days: Option<u64>,
        grace_period_days: Option<u64>,
    ) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        // Update checkin period if provided
        if let Some(new_checkin) = checkin_period_days {
            let valid = 1..=MAX_PERIOD_DAYS;
            if !valid.contains(&new_checkin) {
                panic_with_error!(&env, WillError::InvalidPeriod);
            }
            will.checkin_period_days = new_checkin;
        }

        // Update grace period if provided
        if let Some(new_grace) = grace_period_days {
            let valid = 1..=MAX_PERIOD_DAYS;
            if !valid.contains(&new_grace) {
                panic_with_error!(&env, WillError::InvalidPeriod);
            }
            will.grace_period_days = new_grace;
        }

        let now = env.ledger().timestamp();
        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        events::periods_updated(
            &env,
            will_id,
            &owner,
            will.checkin_period_days,
            will.grace_period_days,
            next_deadline,
        );
    }

    /// Atomically updates multiple will settings (beneficiaries, guardians, and periods)
    /// in a single transaction. Only callable by the owner while the will is `Active`.
    ///
    /// Any unspecified field (passed as `None`) is left unchanged. This allows callers
    /// to update only the settings they need without specifying the others.
    ///
    /// # Parameters
    /// - `will_id`: the will to update
    /// - `owner`: the will's owner; must authorize this call
    /// - `beneficiaries`: new beneficiary list (optional); if specified, must be valid
    /// - `guardians`: new guardian list (optional); if specified, must be valid
    /// - `checkin_period_days`: new check-in period (optional)
    /// - `grace_period_days`: new grace period (optional)
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - Any validation error from the specific update functions.
    pub fn update_will_settings(
        env: Env,
        will_id: u64,
        owner: Address,
        beneficiaries: Option<Vec<Beneficiary>>,
        guardians: Option<Vec<Address>>,
        checkin_period_days: Option<u64>,
        grace_period_days: Option<u64>,
    ) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        let mut updated_fields: Vec<soroban_sdk::Symbol> = Vec::new(&env);

        // Update beneficiaries if provided
        if let Some(new_beneficiaries) = beneficiaries {
            if new_beneficiaries.is_empty() || new_beneficiaries.len() > MAX_BENEFICIARIES {
                panic_with_error!(&env, WillError::TooManyBeneficiaries);
            }
            assert_valid_percentages(&env, &new_beneficiaries);

            // Update reverse indexes
            for old in will.beneficiaries.iter() {
                if !names_address(&new_beneficiaries, &old.address) {
                    storage::remove_beneficiary_index(&env, &old.address, will_id);
                }
            }
            for new_beneficiary in new_beneficiaries.iter() {
                if !names_address(&will.beneficiaries, &new_beneficiary.address) {
                    storage::index_by_beneficiary(&env, &new_beneficiary.address, will_id);
                }
            }

            will.beneficiaries = new_beneficiaries;
            updated_fields.push_back(symbol_short!("benef"));
        }

        // Update guardians if provided
        if let Some(new_guardians) = guardians {
            assert_valid_guardians(&env, &owner, &new_guardians);
            let now = env.ledger().timestamp();
            storage::reset_guardian_votes(&env, &will);
            let mut guardian_structs: Vec<Guardian> = Vec::new(&env);
            for addr in new_guardians.iter() {
                guardian_structs.push_back(Guardian {
                    address: addr,
                    weight: 1,
                });
            }
            will.guardians = guardian_structs;
            will.guardian_votes = 0;
            will.guardian_list_updated_at = now;
            will.guardian_vote_weight = 0;
            updated_fields.push_back(symbol_short!("guard"));
        }

        // Update checkin period if provided
        if let Some(new_checkin) = checkin_period_days {
            let valid = 1..=MAX_PERIOD_DAYS;
            if !valid.contains(&new_checkin) {
                panic_with_error!(&env, WillError::InvalidPeriod);
            }
            will.checkin_period_days = new_checkin;
            updated_fields.push_back(symbol_short!("checkin"));
        }

        // Update grace period if provided
        if let Some(new_grace) = grace_period_days {
            let valid = 1..=MAX_PERIOD_DAYS;
            if !valid.contains(&new_grace) {
                panic_with_error!(&env, WillError::InvalidPeriod);
            }
            will.grace_period_days = new_grace;
            updated_fields.push_back(symbol_short!("grace"));
        }

        // Save the will with all updates applied
        storage::save_will(&env, &will);

        // Emit consolidated event with the list of updated fields
        events::will_settings_updated(&env, will_id, &owner, &updated_fields);
    }

    /// Adds `amount` of a specific `token` to an existing will's locked
    /// balance. Only possible while the will is `Active`. The token does not
    /// need to have been part of the original `create_will` call — new tokens
    /// can be added via `top_up`.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::ZeroAmount`] if `amount` is not positive.
    pub fn top_up(env: Env, will_id: u64, owner: Address, token: Address, amount: i128) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        if amount <= 0 {
            panic_with_error!(&env, WillError::ZeroAmount);
        }

        // Snapshot values needed after state mutation (checks-effects-interactions).
        let prev = will.balances.get(token.clone()).unwrap_or(0);
        let new_balance = prev + amount;

        // --- EFFECTS: update state and persist before the external transfer ---
        will.balances.set(token.clone(), new_balance);
        storage::save_will(&env, &will);

        // --- INTERACTIONS: external token transfer after state is committed ---
        token::Client::new(&env, &token).transfer(
            &owner,
            &env.current_contract_address(),
            &amount,
        );

        events::top_up(&env, will_id, &owner, &token, amount, new_balance);
    }

    /// Returns the contract version as a `u32` encoded semver value:
    /// `major * 1_000_000 + minor * 1_000 + patch`.
    ///
    /// SDKs and apps can call this to detect version mismatches before
    /// submitting transactions that depend on specific contract behaviour.
    pub fn get_contract_version(_env: Env) -> u32 {
        CONTRACT_VERSION
    }

    /// Returns the full on-chain state of `will_id`.
    ///
    /// # Panics
    /// - [`WillError::WillNotFound`] if no will exists with this id.
    pub fn get_will(env: Env, will_id: u64) -> Will {
        load_will(&env, will_id)
    }

    /// Returns just the lifecycle status of `will_id`, without loading or
    /// transmitting the rest of the `Will` struct (beneficiaries, guardians,
    /// balances, etc.).
    ///
    /// Intended for polling use cases — e.g. a dashboard checking the status
    /// of many wills — where the caller only needs the status and calling
    /// [`Self::get_will`] would mean deserializing and transmitting data it
    /// throws away.
    ///
    /// # Panics
    /// - [`WillError::WillNotFound`] if no will exists with this id.
    pub fn get_will_status(env: Env, will_id: u64) -> WillStatus {
        load_will(&env, will_id).status
    }

    /// Returns the number of seconds until `will_id`'s next relevant
    /// deadline, or `None` if the will's current status has no deadline.
    ///
    /// The deadline depends on status:
    /// - `Active`: seconds until the check-in deadline
    ///   (`last_checkin + checkin_period_days`).
    /// - `Triggered`: seconds until the grace period expires
    ///   (`trigger_time + grace_period_days`).
    /// - `Released`, `Cancelled`, `Settled`: no deadline applies; returns
    ///   `None`.
    ///
    /// The returned value is negative when the deadline has already passed
    /// (e.g. an `Active` will whose check-in deadline elapsed but which has
    /// not yet been `trigger_will`-ed, or a `Triggered` will whose grace
    /// period has expired but has not yet been released) — callers should
    /// treat any non-positive value as "actionable now" rather than treating
    /// only `None` as the past-due signal.
    ///
    /// Like [`Self::get_will_status`], this avoids loading and transmitting
    /// the full `Will` struct for simple polling use cases.
    ///
    /// # Panics
    /// - [`WillError::WillNotFound`] if no will exists with this id.
    pub fn get_time_until_deadline(env: Env, will_id: u64) -> Option<i64> {
        let will = load_will(&env, will_id);
        let now = env.ledger().timestamp() as i64;

        match will.status {
            WillStatus::Active => {
                let deadline =
                    will.last_checkin as i64 + (will.checkin_period_days * SECONDS_PER_DAY) as i64;
                Some(deadline - now)
            }
            WillStatus::Triggered => {
                let trigger_time = will.trigger_time.unwrap_or(will.last_checkin) as i64;
                let deadline = trigger_time + (will.grace_period_days * SECONDS_PER_DAY) as i64;
                Some(deadline - now)
            }
            WillStatus::Released | WillStatus::Cancelled | WillStatus::Settled => None,
        }
    }

    /// Returns aggregate protocol statistics for all wills currently tracked on-chain.
    pub fn get_protocol_stats(env: Env) -> ProtocolStats {
        storage::get_protocol_stats(&env)
    }

    /// Returns the list of will ids currently in `Triggered` status.
    ///
    /// This is the on-chain index that lets keeper bots and monitoring tools
    /// efficiently discover wills that are past their check-in deadline and
    /// within their grace period, without having to replay every
    /// `will_triggered` event off-chain.
    pub fn get_triggered_wills(env: Env) -> Vec<u64> {
        storage::get_triggered_wills(&env)
    }

    /// Returns the full state of every will owned by `owner`.
    pub fn get_wills_by_owner(env: Env, owner: Address) -> Vec<Will> {
        let ids = storage::get_owner_wills(&env, &owner);
        let page = storage::paginate_ids(&env, &ids, cursor, limit);
        let mut wills = Vec::new(&env);
        for id in page.iter() {
            if let Ok(will) = storage::load_will(&env, id) {
                wills.push_back(will);
            }
        }
        wills
    }

    /// Returns a page of wills that `beneficiary` is named in.
    ///
    /// Supports bounded pagination to avoid hitting Soroban resource limits
    /// for addresses with many wills.
    ///
    /// # Parameters
    /// - `beneficiary`: the address to query wills for.
    /// - `cursor`: optional will id to paginate after (exclusive). Pass `None`
    ///   or `0` for the first page.
    /// - `limit`: maximum number of wills to return. Capped at
    ///   [`storage::MAX_PAGE_SIZE`].

    /// Returns the full state of every will owned by `owner` with the given `status`.
    pub fn get_wills_by_owner_and_status(
        env: Env,
        owner: Address,
        status: WillStatus,
    ) -> Vec<Will> {
        let ids = storage::get_owner_wills(&env, &owner);
        let mut wills = Vec::new(&env);
        for id in ids.iter() {
            if let Ok(will) = storage::load_will(&env, id) {
                if will.status == status {
                    wills.push_back(will);
                }
            }
        }
        wills
    }

    /// Returns the full state of every will `beneficiary` is named in.
    pub fn get_wills_by_beneficiary(env: Env, beneficiary: Address) -> Vec<Will> {
        let ids = storage::get_beneficiary_wills(&env, &beneficiary);
        let mut wills = Vec::new(&env);
        for id in ids.iter() {
            if let Ok(will) = storage::load_will(&env, id) {
                wills.push_back(will);
            }
        }
        wills
    }

    /// Casts a guardian vote to force an early release of `will_id`, for use
    /// when the owner is known to be incapacitated. Once
    /// `guardian_threshold` distinct guardians have voted, all balances are
    /// immediately distributed to beneficiaries, bypassing the check-in and
    /// grace-period flow entirely.
    ///
    /// Enforces a cooldown after a guardian-list change: if the current
    /// guardian list was updated less than [`GUARDIAN_COOLDOWN_DAYS`] days ago,
    /// the vote is rejected with [`WillError::GuardianCooldownActive`].
    ///
    /// # Parameters
    /// - `reason`: the reason the guardian is casting the vote.
    ///
    /// # Panics
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::NotGuardian`] if `guardian` is not one of the will's guardians.
    /// - [`WillError::AlreadyVoted`] if `guardian` already voted in this cycle.
    /// - [`WillError::GuardianCooldownActive`] if the guardian-list cooldown has not elapsed.
    pub fn guardian_trigger(env: Env, will_id: u64, guardian: Address, reason: GuardianVoteReason) {
        guardian.require_auth();
        let mut will = load_will(&env, will_id);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        // Enforce guardian-list cooldown.
        let now = env.ledger().timestamp();
        let cooldown_seconds = GUARDIAN_COOLDOWN_DAYS * SECONDS_PER_DAY;
        let cooldown_ends = will.guardian_list_updated_at + cooldown_seconds;
        if now < cooldown_ends {
            panic_with_error!(&env, WillError::GuardianCooldownActive);
        }

        let weight = match will.guardians.iter().find(|g| g.address == guardian) {
            Some(g) => g.weight,
            None => panic_with_error!(&env, WillError::NotGuardian),
        };

        let expiry_days = will.grace_period_days;
        if storage::has_guardian_voted(&env, will_id, &guardian, now, expiry_days) {
            panic_with_error!(&env, WillError::AlreadyVoted);
        }

        storage::set_guardian_voted(&env, will_id, &guardian, now, reason);
        will.guardian_vote_weight += weight;
        will.guardian_votes += 1;
        storage::save_will(&env, &will);

        events::guardian_voted(&env, will_id, &guardian, weight, will.guardian_vote_weight);

        if will.guardian_votes >= will.guardian_threshold {
            record_transition(
                &env,
                will_id,
                WillStatus::Active,
                WillStatus::Released,
                &guardian,
                symbol_short!("gtrigr"),
            );
            distribute(&env, &mut will, &None);
        }
    }

    // ── #21: Will cloning / templates ────────────────────────────────────

    /// Clones an existing will's configuration into a new will with fresh
    /// token balances.
    ///
    /// Copies beneficiaries, guardian list, check-in period, and grace period
    /// from the source will. The new will gets a fresh balance (funded by the
    /// `tokens` parameter), a new id, and starts with `Active` status and a
    /// fresh check-in deadline.
    ///
    /// The source will must be `Active` or `Triggered` (any non-destroyed will).
    /// The owner must authorize this call.
    ///
    /// # Parameters
    /// - `source_will_id`: the id of the will to clone configuration from.
    /// - `owner`: the address creating the new will.
    /// - `tokens`: token balances to lock in the new will (same format as
    ///   [`create_will`]).
    ///
    /// # Returns
    /// The newly allocated will id.
    ///
    /// # Panics
    /// - [`WillError::WillNotFound`] if the source will does not exist.
    /// - [`WillError::ZeroAmount`] if any token amount is not positive.
    /// - [`WillError::TooManyBeneficiaries`] if the token list is empty or too large.
    #[allow(clippy::too_many_arguments)]
    pub fn clone_will(
        env: Env,
        source_will_id: u64,
        owner: Address,
        tokens: Vec<(Address, i128)>,
    ) -> u64 {
        owner.require_auth();

        if tokens.is_empty() || tokens.len() > MAX_TOKENS {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }

        let source = load_will(&env, source_will_id);

        // Build balances map and transfer tokens from the owner.
        let mut balances: Map<Address, i128> = Map::new(&env);
        for (token_addr, amount) in tokens.iter() {
            if amount <= 0 {
                panic_with_error!(&env, WillError::ZeroAmount);
            }
            token::Client::new(&env, &token_addr).transfer(
                &owner,
                &env.current_contract_address(),
                &amount,
            );
            let prev = balances.get(token_addr.clone()).unwrap_or(0);
            balances.set(token_addr, prev + amount);
        }

        let will_id = storage::next_will_id(&env);
        let now = env.ledger().timestamp();
        let token_count = balances.len();
        let beneficiaries_count = source.beneficiaries.len();

        for beneficiary in source.beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &beneficiary.address, will_id);
        }

        let will = Will {
            id: will_id,
            owner: owner.clone(),
            balances,
            beneficiaries: source.beneficiaries.clone(),
            checkin_period_days: source.checkin_period_days,
            grace_period_days: source.grace_period_days,
            last_checkin: now,
            trigger_time: None,
            status: WillStatus::Active,
            guardians: source.guardians.clone(),
            guardian_vote_weight: 0,
            guardian_votes: 0,
            guardian_threshold: source.guardian_threshold,
            guardian_list_updated_at: now,
            schema_version: CURRENT_SCHEMA_VERSION,
            keeper_bounty_bps: source.keeper_bounty_bps,
        };
        storage::save_will(&env, &will);
        storage::index_by_owner(&env, &owner, will_id);

        events::will_created(
            &env,
            will_id,
            &owner,
            token_count,
            beneficiaries_count,
            now + source.checkin_period_days * SECONDS_PER_DAY,
        );
        events::will_cloned(&env, source_will_id, will_id, &owner);

        will_id
    }

    // ── #19: Batch will creation ─────────────────────────────────────────

    /// Creates multiple wills in a single transaction.
    ///
    /// Each entry in `will_specs` is a tuple of:
    /// - `tokens`: `(token_address, amount)` pairs to lock.
    /// - `beneficiaries`: beneficiary list with basis-point shares.
    /// - `checkin_period_days`: check-in period in days.
    /// - `grace_period_days`: grace period in days.
    /// - `guardians`: guardian address list.
    ///
    /// The owner must authorize the entire call. All wills are created under
    /// the same `owner`.
    ///
    /// # Returns
    /// A `Vec<u64>` of newly allocated will ids, one per spec.
    ///
    /// # Panics
    /// - [`WillError::TooManyBeneficiaries`] if the batch is empty or exceeds
    ///   [`BATCH_MAX`], or if any individual spec violates beneficiary/guardian/token caps.
    /// - Any error that [`create_will`] would panic with for an individual spec.
    #[allow(clippy::too_many_arguments)]
    pub fn batch_create_wills(
        env: Env,
        owner: Address,
        will_specs: Vec<(
            Vec<(Address, i128)>,
            Vec<Beneficiary>,
            u64,
            u64,
            Vec<Address>,
            u32,
        )>,
    ) -> Vec<u64> {
        owner.require_auth();

        if will_specs.is_empty() || will_specs.len() > BATCH_MAX {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }

        let mut ids = Vec::new(&env);
        for spec in will_specs.iter() {
            let (
                tokens,
                beneficiaries,
                checkin_period_days,
                grace_period_days,
                guardians,
                guardian_threshold,
            ) = spec;

            // Inline the validation + creation logic (mirrors create_will)
            // to avoid re-authorizing per will.
            if tokens.is_empty() || tokens.len() > MAX_TOKENS {
                panic_with_error!(&env, WillError::TooManyBeneficiaries);
            }
            if beneficiaries.is_empty() || beneficiaries.len() > MAX_BENEFICIARIES {
                panic_with_error!(&env, WillError::TooManyBeneficiaries);
            }
            assert_valid_percentages(&env, &beneficiaries);
            assert_valid_guardians(&env, &owner, &guardians);
            assert_valid_periods(&env, checkin_period_days, grace_period_days);
            if !guardians.is_empty() {
                let threshold_range = 1..=guardians.len() as u32;
                if !threshold_range.contains(guardian_threshold) {
                    panic_with_error!(&env, WillError::InvalidGuardianThreshold);
                }
            }

            let mut balances: Map<Address, i128> = Map::new(&env);
            for (token_addr, amount) in tokens.iter() {
                if amount <= 0 {
                    panic_with_error!(&env, WillError::ZeroAmount);
                }
                token::Client::new(&env, &token_addr).transfer(
                    &owner,
                    &env.current_contract_address(),
                    &amount,
                );
                let prev = balances.get(token_addr.clone()).unwrap_or(0);
                balances.set(token_addr, prev + amount);
            }

            let mut guardian_structs: Vec<Guardian> = Vec::new(&env);
            for addr in guardians.iter() {
                guardian_structs.push_back(Guardian {
                    address: addr,
                    weight: 1,
                });
            }

            let will_id = storage::next_will_id(&env);
            let now = env.ledger().timestamp();
            let beneficiaries_count = beneficiaries.len();
            let token_count = balances.len();

            for beneficiary in beneficiaries.iter() {
                storage::index_by_beneficiary(&env, &beneficiary.address, will_id);
            }

            let will = Will {
                id: will_id,
                owner: owner.clone(),
                balances,
                beneficiaries,
                checkin_period_days,
                grace_period_days,
                last_checkin: now,
                trigger_time: None,
                status: WillStatus::Active,
                guardians: guardian_structs,
                guardian_vote_weight: 0,
                guardian_votes: 0,
                guardian_threshold: *guardian_threshold,
                guardian_list_updated_at: now,
                schema_version: CURRENT_SCHEMA_VERSION,
                keeper_bounty_bps: 0,
            };
            storage::save_will(&env, &will);
            storage::index_by_owner(&env, &owner, will_id);

            events::will_created(
                &env,
                will_id,
                &owner,
                token_count,
                beneficiaries_count,
                now + checkin_period_days * SECONDS_PER_DAY,
            );

            ids.push_back(will_id);
        }

        events::batch_created(&env, &owner, &ids);
        ids
    /// Migrates a will to the latest schema version. The owner must authorize
    /// this call. This is an owner-initiated per-will migration that allows
    /// users to opt-in to new contract versions without being forced to do so.
    ///
    /// # Current behavior (v0 → v1)
    /// Sets the schema_version field to 1. Future versions will implement
    /// data transformations here.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotFound`] if the will does not exist.
    pub fn migrate_will(env: Env, will_id: u64, owner: Address) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);

        let old_version = will.schema_version;

        // Check if already on current version
        if old_version >= CURRENT_SCHEMA_VERSION {
            return;
        }

        // Apply version-specific migrations in sequence
        will.schema_version = CURRENT_SCHEMA_VERSION;

        storage::save_will(&env, &will);
        events::will_migrated(&env, will_id, &owner, old_version, CURRENT_SCHEMA_VERSION);
    }

    /// Merges two active wills owned by the same address into a single will.
    /// 
    /// The merge policy is:
    /// - The surviving will (will_id_a) receives the combined balance.
    /// - Beneficiaries from both wills are merged, with percentages recalculated
    ///   proportionally based on the combined balance. If a beneficiary appears
    ///   in both wills, their percentages are summed first, then recalculated.
    /// - Guardians from both wills are combined into a single list (up to MAX_GUARDIANS).
    /// - Check-in period: use the minimum (most conservative).
    /// - Grace period: use the maximum (most conservative).
    /// - The consumed will (will_id_b) is marked as Cancelled with zero balance.
    ///
    /// # Parameters
    /// - `owner`: the owner of both wills; must authorize this call.
    /// - `will_id_a`: the will that survives and receives the merged state.
    /// - `will_id_b`: the will that is consumed (marked Cancelled).
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own both wills.
    /// - [`WillError::WillNotBothActive`] if either will is not in `Active` status.
    /// - [`WillError::SameWillId`] if `will_id_a` equals `will_id_b`.
    /// - [`WillError::MergeWouldExceedLimits`] if merging would exceed MAX_BENEFICIARIES or MAX_GUARDIANS limits.
    /// - [`WillError::InvalidPercentages`] if recalculating percentages fails.
    pub fn merge_wills(
        env: Env,
        owner: Address,
        will_id_a: u64,
        will_id_b: u64,
    ) {
        owner.require_auth();

        if will_id_a == will_id_b {
            panic_with_error!(&env, WillError::SameWillId);
        }

        let mut will_a = load_owned(&env, will_id_a, &owner);
        let mut will_b = load_owned(&env, will_id_b, &owner);

        assert_status(&env, &will_a, WillStatus::Active, WillError::WillNotBothActive);
        assert_status(&env, &will_b, WillStatus::Active, WillError::WillNotBothActive);

        // Merge beneficiaries with proportional recalculation
        let merged_beneficiaries = merge_beneficiaries(&env, &will_a, &will_b);

        if merged_beneficiaries.len() > MAX_BENEFICIARIES {
            panic_with_error!(&env, WillError::MergeWouldExceedLimits);
        }

        // Merge guardians (unique)
        let mut merged_guardians = will_a.guardians.clone();
        for guardian in will_b.guardians.iter() {
            if !merged_guardians.contains(&guardian) {
                merged_guardians.push_back(guardian);
            }
        }

        if merged_guardians.len() > MAX_GUARDIANS {
            panic_with_error!(&env, WillError::MergeWouldExceedLimits);
        }

        // Merge parameters: use minimum check-in period, maximum grace period
        let merged_checkin_period = if will_a.checkin_period_days < will_b.checkin_period_days {
            will_a.checkin_period_days
        } else {
            will_b.checkin_period_days
        };

        let merged_grace_period = if will_a.grace_period_days > will_b.grace_period_days {
            will_a.grace_period_days
        } else {
            will_b.grace_period_days
        };

        // Combine balances
        let combined_balance = will_a.balance + will_b.balance;

        // Update will_a with merged state
        will_a.beneficiaries = merged_beneficiaries;
        will_a.guardians = merged_guardians;
        will_a.checkin_period_days = merged_checkin_period;
        will_a.grace_period_days = merged_grace_period;
        will_a.balance = combined_balance;
        will_a.guardian_votes = 0;

        // Remove old beneficiary indexes for will_b
        for beneficiary in will_b.beneficiaries.iter() {
            storage::remove_beneficiary_index(&env, &beneficiary.address, will_id_b);
        }

        // Mark will_b as cancelled with zero balance
        will_b.balance = 0;
        will_b.status = WillStatus::Cancelled;

        // Save both wills
        storage::save_will(&env, &will_a);
        storage::save_will(&env, &will_b);

        // Update beneficiary indexes for will_a
        for beneficiary in will_a.beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &beneficiary.address, will_id_a);
        }

        events::wills_merged(&env, will_id_a, will_id_b, &owner, combined_balance);
    }

    /// Returns the full audit trail for `will_id`, recording every status
    /// transition since creation.
    pub fn get_will_history(env: Env, will_id: u64) -> Vec<WillStatusTransition> {
        storage::get_history(&env, will_id)
    }

    /// Archives a Released or Cancelled will, removing it from active
    /// storage and indexes so it no longer appears in owner/beneficiary
    /// queries. The archived will data will eventually be garbage-collected
    /// by Soroban's state archival system.
    ///
    /// Callable by anyone: once a will is settled it can be archived to
    /// reduce ongoing storage costs.
    ///
    /// # Panics
    /// - [`WillError::WillNotFound`] if no will exists with this id.
    /// - [`WillError::WillNotSettled`] if the will is not `Released` or `Cancelled`.
    pub fn archive_will(env: Env, will_id: u64) {
        let will = load_will(&env, will_id);
        if will.status != WillStatus::Released && will.status != WillStatus::Cancelled {
            panic_with_error!(&env, WillError::WillNotSettled);
        }

        let archived_will = will.clone();
        storage::archive_will(&env, &will);

        events::will_archived(&env, will_id, &archived_will.owner);
    }
}

// ── Private helpers ─────────────────────────────────────────────────────────

/// Loads a will by id, panicking with [`WillError::WillNotFound`] if it does not exist.
fn load_will(env: &Env, will_id: u64) -> Will {
    match storage::load_will(env, will_id) {
        Ok(will) => will,
        Err(e) => panic_with_error!(env, e),
    }
}

/// Loads a will by id and asserts `owner` is its owner.
fn load_owned(env: &Env, will_id: u64, owner: &Address) -> Will {
    let will = load_will(env, will_id);
    if &will.owner != owner {
        panic_with_error!(env, WillError::NotOwner);
    }
    will
}

/// Asserts a will is in the `expected` status, panicking with `err` otherwise.
fn assert_status(env: &Env, will: &Will, expected: WillStatus, err: WillError) {
    if will.status != expected {
        panic_with_error!(env, err);
    }
}

/// Returns whether `beneficiaries` names `address`.
///
/// Operates on an in-memory list, so callers can decide reverse-index
/// membership without touching storage.
fn names_address(beneficiaries: &Vec<Beneficiary>, address: &Address) -> bool {
    beneficiaries
        .iter()
        .any(|beneficiary| &beneficiary.address == address)
}

/// Asserts beneficiary basis points sum to exactly 10,000 and no beneficiary has zero percentage.
///
/// The per-entry upper bound is not enforced here (shares are `u32` and
/// `MAX_BENEFICIARIES` is small enough that overflow is not a concern), but
/// the exact-sum invariant is critical: it guarantees that every token
/// balance is fully distributed with no dust left behind.
///
/// Each beneficiary must have a non-zero basis point allocation; zero-percentage
/// entries waste a slot in the beneficiary list and indicate a client-side error.
///
/// The same address may not appear more than once: the beneficiary index only
/// stores one entry per address, so a repeated address silently drops one of
/// the split percentages rather than actually splitting the share, which is
/// almost certainly a client-side mistake rather than intentional.
fn assert_valid_percentages(env: &Env, beneficiaries: &Vec<Beneficiary>) {
    let mut total: u32 = 0;
    for i in 0..beneficiaries.len() {
        let beneficiary = beneficiaries.get_unchecked(i);
        if beneficiary.basis_points == 0 {
            panic_with_error!(env, WillError::InvalidPercentages);
        }
        total += beneficiary.basis_points;
        for j in (i + 1)..beneficiaries.len() {
            if beneficiary.address == beneficiaries.get_unchecked(j).address {
                panic_with_error!(env, WillError::DuplicateBeneficiary);
            }
        }
    }
    if total != 10_000 {
        panic_with_error!(env, WillError::InvalidPercentages);
    }
}

/// Transfers funds using either native XLM or token contract depending on
/// `is_native`. When native, uses `env.transfer()`; otherwise uses the
/// standard token client.
fn transfer_funds(
    env: &Env,
    is_native: bool,
    token_address: &Address,
    from: &Address,
    to: &Address,
    amount: &i128,
) {
    if is_native {
        env.transfer(from, to, amount);
    } else {
        token::Client::new(env, token_address).transfer(from, to, amount);
    }
}

/// Returns the balance of `address` for the asset identified by the will.
/// For native XLM this uses `env.balance()`, for token contracts it uses the
/// token client's `balance` method.
fn balance_of(env: &Env, is_native: bool, token_address: &Address, address: &Address) -> i128 {
    if is_native {
        env.balance(address)
    } else {
        token::Client::new(env, token_address).balance(address)
    }
}

/// Asserts a guardian list is no longer than [`MAX_GUARDIANS`] and contains no
/// repeated address. Also validates that the owner is not in the guardian list.
///
/// Duplicates matter because [`WillContract::guardian_trigger`] counts each
/// address at most once. A list such as `[g, g]` looks like a working 2-of-2
/// quorum but can only ever reach a single vote, silently leaving the will with
/// a guardian override that can never fire.
///
/// The owner cannot be a guardian since guardians are meant to act when the
/// owner is incapacitated or known to be dead.
fn assert_valid_guardians(env: &Env, owner: &Address, guardians: &Vec<Address>) {
    if guardians.len() > MAX_GUARDIANS {
        panic_with_error!(env, WillError::TooManyBeneficiaries);
    }
    for i in 0..guardians.len() {
        let guardian = guardians.get_unchecked(i);
        if guardian == owner {
            panic_with_error!(env, WillError::OwnerCannotBeGuardian);
        }
        for j in (i + 1)..guardians.len() {
            if guardian == guardians.get_unchecked(j) {
                panic_with_error!(env, WillError::DuplicateGuardian);
            }
        }
    }
}

/// Asserts both periods are at least one day and at most [`MAX_PERIOD_DAYS`].
///
/// The upper bound keeps `days * SECONDS_PER_DAY` well inside `u64`. The lower
/// bound rules out a zero-day period, which would make a will triggerable (or
/// releasable) in the very ledger it was created in, defeating the check-in
/// mechanism entirely.
fn assert_valid_periods(env: &Env, checkin_period_days: u64, grace_period_days: u64) {
    let valid = 1..=MAX_PERIOD_DAYS;
    if !valid.contains(&checkin_period_days) || !valid.contains(&grace_period_days) {
        panic_with_error!(env, WillError::InvalidPeriod);
    }
}

/// Distributes all token balances across `will.beneficiaries` proportionally
/// to their basis-point shares, transfers the shares out of the contract,
/// clears the balances map, marks the will `Released`, and publishes the
/// `InheritanceReleased` event.
///
/// # Rounding Behavior
///
/// Each token's distribution is calculated as: `share = balance * (basis_points / 10_000)`.
/// Integer division truncates toward zero, which may result in zero shares for
/// beneficiaries with very small calculated amounts. For example, distributing 9 units
/// equally among 10 beneficiaries (900 basis points each) gives each person 0.9 units,
/// which truncates to 0.
///
/// To ensure no dust is left behind, any rounding remainder is paid to the final
/// beneficiary in the list. This guarantees the full balance of every token is
/// always distributed across beneficiaries.
///
/// **Note:** Callers should ensure that the will's balance is sufficient to give
/// each beneficiary at least 1 unit of their share. Extremely small balances relative
/// to beneficiary counts can result in most recipients getting zero after rounding.
/// Consider validating a minimum will amount at creation time (see issue #37).
/// Splits `will.balance` across `will.beneficiaries` proportionally to their
/// percentages, transfers the shares out of the contract, marks the will
/// `Released`, and publishes the `InheritanceReleased` event with a full
/// For each token in `will.balances`, splits the balance across
/// `will.beneficiaries` proportionally to their basis-point shares, transfers
/// the shares out of the contract, clears the balances map, marks the will
/// `Released`, and publishes the `InheritanceReleased` event. Any rounding
/// remainder from integer division is paid to the final beneficiary so the
/// full balance of every token is always distributed with no dust left behind.
///
/// Follows checks-effects-interactions ordering: all per-beneficiary share
/// amounts are computed from the pre-mutation balances, then all state is
/// committed (status, balances, indexes), and only then are the external
/// token transfers executed.
fn distribute(env: &Env, will: &mut Will, keeper: &Option<Address>) {
    let contract_address = env.current_contract_address();
    let count = will.beneficiaries.len();
    let token_count = will.balances.len();

    // --- COMPUTE: calculate every share from the current (pre-mutation) balances ---
    // Calculate keeper bounty if applicable (not paid to owner, only to other keepers)
    let mut bounty_amount: i128 = 0;
    let should_pay_bounty = keeper
        .as_ref()
        .map(|k| k != &will.owner && will.keeper_bounty_bps > 0)
        .unwrap_or(false);

    // Build a Vec of (token_addr, Vec<(beneficiary_addr, share)>) so we can
    // commit all state before any external call fires.
    let mut transfer_plan: Vec<(Address, Vec<(Address, i128)>)> = Vec::new(env);

    for (token_addr, total) in will.balances.iter() {
        if total == 0 {
            continue;
        }

        // Calculate bounty from first token's balance if applicable
        if should_pay_bounty && bounty_amount == 0 {
            bounty_amount = (total * (will.keeper_bounty_bps as i128)) / 10_000;
        }

        let mut shares: Vec<(Address, i128)> = Vec::new(env);
        let mut remaining = total;
        for (index, beneficiary) in will.beneficiaries.iter().enumerate() {
            let share = if index as u32 == count - 1 {
                remaining
            } else {
                let portion = total * (beneficiary.basis_points as i128) / 10_000;
                remaining -= portion;
                portion
            };
            shares.push_back((beneficiary.address.clone(), share));
        }
        transfer_plan.push_back((token_addr, shares));
    }

    // --- EFFECTS: mutate and persist all state before any external call ---
    storage::decrement_active_will_count(env);

    will.balance = 0;
    will.balances = Map::new(env);
    will.status = WillStatus::Released;

    // Prune stale index entries (#71): remove the released will from the
    // owner index and from every beneficiary's reverse index.
    storage::remove_owner_index(env, &will.owner, will.id);
    for beneficiary in will.beneficiaries.iter() {
        storage::remove_beneficiary_index(env, &beneficiary.address, will.id);
    }

    storage::unindex_triggered_will(env, will.id);
    storage::save_will(env, will);

    // --- INTERACTIONS: external token transfers execute after state is settled ---
    for (token_addr, shares) in transfer_plan.iter() {
        let token_client = token::Client::new(env, &token_addr);
        for (beneficiary_addr, share) in shares.iter() {
            if share > 0 {
                token_client.transfer(&contract_address, &beneficiary_addr, &share);
            }
        }

        // Pay keeper bounty from first token if applicable
        if should_pay_bounty && bounty_amount > 0 {
            if let Some(keeper_addr) = keeper {
                token_client.transfer(&contract_address, keeper_addr, &bounty_amount);
                events::keeper_bounty_paid(env, will.id, keeper_addr, bounty_amount);
            }
            bounty_amount = 0; // Only pay once
        }
    }

    events::inheritance_released(env, will.id, token_count, count);
}

/// Merges beneficiaries from two wills, recalculating percentages proportionally
/// based on the combined balance. If a beneficiary appears in both wills, their
/// percentages are summed before recalculation.
fn merge_beneficiaries(env: &Env, will_a: &Will, will_b: &Will) -> Vec<Beneficiary> {
    let total_balance = will_a.balance + will_b.balance;
    let mut beneficiary_shares: Vec<(Address, i128)> = Vec::new(env);

    // Add beneficiaries from will_a with proportional balance
    for beneficiary in will_a.beneficiaries.iter() {
        let share = (will_a.balance as i128) * (beneficiary.percentage as i128) / 100;
        beneficiary_shares.push_back((beneficiary.address.clone(), share));
    }

    // Merge beneficiaries from will_b, combining shares if they already exist
    for beneficiary_b in will_b.beneficiaries.iter() {
        let share_b = (will_b.balance as i128) * (beneficiary_b.percentage as i128) / 100;
        let mut found = false;
        let mut updated_shares: Vec<(Address, i128)> = Vec::new(env);
        for (addr, share) in beneficiary_shares.iter() {
            if addr == beneficiary_b.address {
                updated_shares.push_back((addr, share + share_b));
                found = true;
            } else {
                updated_shares.push_back((addr, share));
            }
        }
        if found {
            beneficiary_shares = updated_shares;
        } else {
            beneficiary_shares.push_back((beneficiary_b.address.clone(), share_b));
        }
    }

    // Recalculate percentages from combined shares
    let mut merged_beneficiaries: Vec<Beneficiary> = Vec::new(env);
    let mut total_percentage: u32 = 0;

    for (addr, share) in beneficiary_shares.iter() {
        let percentage = if total_balance > 0 {
            ((share * 100) / total_balance) as u32
        } else {
            0
        };
        if percentage > 0 {
            merged_beneficiaries.push_back(Beneficiary {
                address: addr,
                percentage,
            });
            total_percentage += percentage;
        }
    }

    // Handle rounding: assign remainder to the last beneficiary
    if total_percentage < 100 && merged_beneficiaries.len() > 0 {
        let remainder = 100 - total_percentage;
        let last_index = merged_beneficiaries.len() - 1;
        let mut last_beneficiary = merged_beneficiaries.get(last_index).unwrap();
        last_beneficiary.percentage += remainder;
        merged_beneficiaries.set(last_index, last_beneficiary);
    }

    merged_beneficiaries
}

/// Records a status transition in `will_id`'s on-chain audit trail.
fn record_transition(
    env: &Env,
    will_id: u64,
    from_status: WillStatus,
    to_status: WillStatus,
    actor: &Address,
    action: soroban_sdk::Symbol,
) {
    let transition = WillStatusTransition {
        will_id,
        from_status,
        to_status,
        timestamp: env.ledger().timestamp(),
        actor: actor.clone(),
        action,
    };
    storage::append_history(env, will_id, &transition);
}

/// Releases `amount` from the will proportionally across beneficiaries and
/// deducts it from `will.balance`. Does NOT change the will's status or
/// persist it — the caller is responsible for saving.
fn distribute_tier(env: &Env, will: &mut Will, amount: i128) {
    let token_client = token::Client::new(env, &will.token);
    let contract_address = env.current_contract_address();
    let count = will.beneficiaries.len();

    let mut remaining = amount;
    for (index, beneficiary) in will.beneficiaries.iter().enumerate() {
        let share = if index as u32 == count - 1 {
            remaining
        } else {
            let portion = amount * (beneficiary.basis_points as i128) / 10_000;
            remaining -= portion;
            portion
        };
        if share > 0 {
            token_client.transfer(&contract_address, &beneficiary.address, &share);
        }
    }

    will.balance -= amount;
}
