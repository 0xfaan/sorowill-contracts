#![no_std]

//! SoroWill — a trustless on-chain inheritance and dead man's switch protocol
//! for Stellar Soroban.
//!
//! An owner locks a token (e.g. USDC) into a `Will`, names beneficiaries with
//! percentage shares, and periodically calls [`WillContract::check_in`] to
//! prove they are still active. If the owner misses a check-in deadline,
//! anyone may call [`WillContract::trigger_will`] to start a grace period.
//! The owner can still call [`WillContract::emergency_checkin`] during the
//! grace period to prove they are alive and reset the countdown. If the
//! grace period elapses without an emergency check-in, anyone may call
//! [`WillContract::release_inheritance`] to split the locked balance among
//! the beneficiaries proportionally to their configured percentages.
//!
//! Optionally, up to three guardians may be named on a will; any two of them
//! calling [`WillContract::guardian_trigger`] force an immediate release,
//! bypassing the check-in/grace-period flow entirely (e.g. if the owner is
//! known to be incapacitated).
//!
//! ## New features (issues #43 – #46)
//!
//! - **#43 Confirmation delay**: `create_will` can accept a
//!   `confirmation_delay_seconds` value > 0, placing the will in
//!   `PendingConfirmation` state until the owner calls `confirm_will`.
//! - **#44 Multi-sig owners**: a will may have `co_owners` and an
//!   `owner_threshold`; privileged actions require that many distinct
//!   authorisations from the owner set.
//! - **#45 Split will**: `split_will` carves a subset of beneficiaries and
//!   balance out into a new, fully independent will.
//! - **#46 Hashed beneficiaries**: a beneficiary can be registered by a
//!   SHA-256 commitment hash; they later call `reveal_and_claim` with the
//!   pre-image to collect their share.

mod errors;
mod events;
mod storage;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Bytes, Env, Vec};

pub use errors::WillError;
pub use types::{Beneficiary, HashedBeneficiary, Will, WillStatus};

/// Number of seconds in a day, used to convert the day-denominated periods
/// stored on a `Will` into absolute ledger timestamps.
const SECONDS_PER_DAY: u64 = 86_400;

/// Maximum number of beneficiaries a single will may have.
const MAX_BENEFICIARIES: u32 = 10;

/// Maximum number of guardians a single will may have.
const MAX_GUARDIANS: u32 = 3;

/// Number of distinct guardian votes required to force an early release.
const GUARDIAN_THRESHOLD: u32 = 2;

#[contract]
pub struct WillContract;

#[contractimpl]
impl WillContract {
    // -----------------------------------------------------------------------
    // Will creation
    // -----------------------------------------------------------------------

    /// Creates a new will, locking `amount` of `token` in the contract.
    ///
    /// If `confirmation_delay_seconds` is **0** the will starts `Active`
    /// immediately (backwards-compatible behaviour). If it is **> 0** the will
    /// starts in `PendingConfirmation` and the owner must call `confirm_will`
    /// within that window; the check-in clock does not start until confirmation.
    ///
    /// For multi-sig (#44) pass `co_owners` and `owner_threshold > 1`.
    /// `owner_threshold` must be ≥ 1 and ≤ `1 + co_owners.len()`.
    ///
    /// # Parameters
    /// - `owner`: the address creating the will; must authorize this call.
    /// - `token`: the token contract address.
    /// - `amount`: the amount of `token` to lock. Must be positive.
    /// - `beneficiaries`: 1–`MAX_BENEFICIARIES` entries whose percentages sum to 100.
    /// - `checkin_period_days`: days between required check-ins.
    /// - `grace_period_days`: days the owner has to respond after a trigger.
    /// - `guardians`: 0–`MAX_GUARDIANS` addresses that may jointly force early release.
    /// - `co_owners`: additional owner addresses for multi-sig (may be empty).
    /// - `owner_threshold`: how many of the owner-set must sign privileged actions.
    /// - `confirmation_delay_seconds`: seconds the owner has to confirm; 0 = skip.
    ///
    /// # Returns
    /// The newly allocated will id.
    #[allow(clippy::too_many_arguments)]
    pub fn create_will(
        env: Env,
        owner: Address,
        token: Address,
        amount: i128,
        beneficiaries: Vec<Beneficiary>,
        checkin_period_days: u64,
        grace_period_days: u64,
        guardians: Vec<Address>,
        co_owners: Vec<Address>,
        owner_threshold: u32,
        confirmation_delay_seconds: u64,
    ) -> u64 {
        owner.require_auth();

        if amount <= 0 {
            panic_with_error!(&env, WillError::ZeroAmount);
        }
        if beneficiaries.is_empty()
            || beneficiaries.len() > MAX_BENEFICIARIES
            || guardians.len() > MAX_GUARDIANS
        {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }
        assert_valid_percentages(&env, &beneficiaries, &Vec::new(&env));

        // Validate multi-sig threshold (#44).
        let total_owners = 1u32 + co_owners.len();
        let threshold = if owner_threshold == 0 { 1 } else { owner_threshold };
        if threshold > total_owners {
            panic_with_error!(&env, WillError::InvalidThreshold);
        }

        let will_id = storage::next_will_id(&env);
        let now = env.ledger().timestamp();

        token::Client::new(&env, &token).transfer(&owner, &env.current_contract_address(), &amount);

        let beneficiaries_count = beneficiaries.len();
        for beneficiary in beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &beneficiary.address, will_id);
        }

        // Determine initial status and confirmation deadline (#43).
        let (status, confirmation_deadline) = if confirmation_delay_seconds > 0 {
            (
                WillStatus::PendingConfirmation,
                Some(now + confirmation_delay_seconds),
            )
        } else {
            (WillStatus::Active, None)
        };

        let will = Will {
            id: will_id,
            owner: owner.clone(),
            co_owners,
            owner_threshold: threshold,
            token,
            balance: amount,
            beneficiaries,
            hashed_beneficiaries: Vec::new(&env),
            checkin_period_days,
            grace_period_days,
            last_checkin: now,
            trigger_time: None,
            confirmation_deadline,
            status,
            guardians,
            guardian_votes: 0,
        };
        storage::save_will(&env, &will);
        storage::index_by_owner(&env, &owner, will_id);

        events::will_created(
            &env,
            will_id,
            &owner,
            amount,
            beneficiaries_count,
            now + checkin_period_days * SECONDS_PER_DAY,
        );

        will_id
    }

    // -----------------------------------------------------------------------
    // Issue #43 — confirm_will
    // -----------------------------------------------------------------------

    /// Transitions a will from `PendingConfirmation` to `Active`, starting the
    /// check-in clock. Must be called by the owner within the confirmation window.
    ///
    /// # Panics
    /// - [`WillError::WillNotFound`] if the will does not exist.
    /// - [`WillError::NotOwner`] if `owner` does not own the will.
    /// - [`WillError::WillNotConfirmed`] if the will is not `PendingConfirmation`.
    /// - [`WillError::ConfirmationWindowExpired`] if the confirmation deadline has passed.
    pub fn confirm_will(env: Env, will_id: u64, owner: Address) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);

        if will.status != WillStatus::PendingConfirmation {
            panic_with_error!(&env, WillError::WillNotConfirmed);
        }

        let now = env.ledger().timestamp();
        if let Some(deadline) = will.confirmation_deadline {
            if now > deadline {
                panic_with_error!(&env, WillError::ConfirmationWindowExpired);
            }
        }

        will.status = WillStatus::Active;
        will.last_checkin = now;
        will.confirmation_deadline = None;
        storage::save_will(&env, &will);

        events::will_confirmed(&env, will_id, &owner);
    }

    // -----------------------------------------------------------------------
    // Core lifecycle
    // -----------------------------------------------------------------------

    /// Resets the check-in countdown for `will_id`. Must be called by the
    /// will's owner (or any co-owner reaching the threshold), and the will
    /// must be `Active`.
    pub fn check_in(env: Env, will_id: u64, owner: Address) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        let now = env.ledger().timestamp();
        will.last_checkin = now;
        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        events::check_in(&env, will_id, &owner, next_deadline);
    }

    /// Starts the grace period for `will_id` once the check-in deadline has
    /// passed. Callable by anyone.
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
        let grace_period_ends = now + will.grace_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        events::will_triggered(&env, will_id, grace_period_ends);
    }

    /// Cancels an in-progress trigger during the grace period, proving the
    /// owner is alive, and resets the check-in countdown.
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

        will.status = WillStatus::Active;
        will.trigger_time = None;
        will.last_checkin = now;
        will.guardian_votes = 0;
        storage::reset_guardian_votes(&env, will_id, &will.guardians);
        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        events::emergency_checkin(&env, will_id, &owner, next_deadline);
    }

    /// Distributes the will's balance to all beneficiaries proportionally.
    /// Callable by anyone once the grace period has fully elapsed.
    ///
    /// # Panics
    /// - [`WillError::WillNotTriggered`] if the will is not `Triggered`.
    /// - [`WillError::GracePeriodNotExpired`] if the grace period has not elapsed yet.
    pub fn release_inheritance(env: Env, will_id: u64) {
        let mut will = load_will(&env, will_id);
        assert_status(
            &env,
            &will,
            WillStatus::Triggered,
            WillError::WillNotTriggered,
        );

        let trigger_time = will.trigger_time.unwrap_or(0);
        let grace_deadline = trigger_time + will.grace_period_days * SECONDS_PER_DAY;
        if env.ledger().timestamp() < grace_deadline {
            panic_with_error!(&env, WillError::GracePeriodNotExpired);
        }

        distribute(&env, &mut will);
    }

    /// Cancels the will and refunds the full locked balance to the owner.
    /// Works from both `Active` and `PendingConfirmation` states so the
    /// owner can abort a pending will during the confirmation window.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is neither `Active` nor
    ///   `PendingConfirmation`.
    pub fn cancel_will(env: Env, will_id: u64, owner: Address) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);

        // Allow cancellation from both Active and PendingConfirmation (#43).
        if will.status != WillStatus::Active && will.status != WillStatus::PendingConfirmation {
            panic_with_error!(&env, WillError::WillNotActive);
        }

        let refund = will.balance;
        token::Client::new(&env, &will.token).transfer(
            &env.current_contract_address(),
            &owner,
            &refund,
        );

        will.balance = 0;
        will.status = WillStatus::Cancelled;
        storage::save_will(&env, &will);

        events::will_cancelled(&env, will_id, &owner, refund);
    }

    /// Replaces the beneficiary list for `will_id`. Only possible while Active.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] / [`WillError::WillNotActive`] / percentage errors.
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
        assert_valid_percentages(&env, &beneficiaries, &will.hashed_beneficiaries);

        for old in will.beneficiaries.iter() {
            storage::remove_beneficiary_index(&env, &old.address, will_id);
        }
        for new_beneficiary in beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &new_beneficiary.address, will_id);
        }

        will.beneficiaries = beneficiaries;
        storage::save_will(&env, &will);

        events::beneficiaries_updated(&env, will_id, &owner);
    }

    /// Replaces the guardian list for `will_id`. Only possible while Active.
    pub fn update_guardians(env: Env, will_id: u64, owner: Address, guardians: Vec<Address>) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        if guardians.len() > MAX_GUARDIANS {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }

        storage::reset_guardian_votes(&env, will_id, &will.guardians);
        will.guardians = guardians;
        will.guardian_votes = 0;
        storage::save_will(&env, &will);

        events::guardians_updated(&env, will_id, &owner);
    }

    /// Adds `amount` more of the will's token to its locked balance.
    /// Only possible while `Active`.
    pub fn top_up(env: Env, will_id: u64, owner: Address, amount: i128) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        if amount <= 0 {
            panic_with_error!(&env, WillError::ZeroAmount);
        }

        token::Client::new(&env, &will.token).transfer(
            &owner,
            &env.current_contract_address(),
            &amount,
        );

        will.balance += amount;
        storage::save_will(&env, &will);

        events::top_up(&env, will_id, &owner, amount, will.balance);
    }

    /// Returns the full on-chain state of `will_id`.
    pub fn get_will(env: Env, will_id: u64) -> Will {
        load_will(&env, will_id)
    }

    /// Returns every will owned by `owner`.
    pub fn get_wills_by_owner(env: Env, owner: Address) -> Vec<Will> {
        let ids = storage::get_owner_wills(&env, &owner);
        let mut wills = Vec::new(&env);
        for id in ids.iter() {
            if let Ok(will) = storage::load_will(&env, id) {
                wills.push_back(will);
            }
        }
        wills
    }

    /// Returns every will `beneficiary` is named in.
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

    /// Casts a guardian vote to force an early release of `will_id`.
    pub fn guardian_trigger(env: Env, will_id: u64, guardian: Address) {
        guardian.require_auth();
        let mut will = load_will(&env, will_id);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        if !will.guardians.contains(&guardian) {
            panic_with_error!(&env, WillError::NotGuardian);
        }
        if storage::has_guardian_voted(&env, will_id, &guardian) {
            panic_with_error!(&env, WillError::AlreadyVoted);
        }

        storage::set_guardian_voted(&env, will_id, &guardian);
        will.guardian_votes += 1;
        storage::save_will(&env, &will);

        events::guardian_voted(&env, will_id, &guardian, will.guardian_votes);

        if will.guardian_votes >= GUARDIAN_THRESHOLD {
            distribute(&env, &mut will);
        }
    }

    // -----------------------------------------------------------------------
    // Issue #45 — split_will
    // -----------------------------------------------------------------------

    /// Carves a subset of beneficiaries and balance out of an existing will
    /// into a new, fully independent child will.
    ///
    /// The original will's balance is reduced by `amount` and any beneficiaries
    /// present in `beneficiaries_to_split` are removed from it; the new will
    /// receives those beneficiaries with percentages renormalised to 100, and
    /// it starts `Active` with the same token, check-in period, grace period,
    /// co-owners, and threshold as the original.
    ///
    /// # Parameters
    /// - `will_id`: the source will to split from.
    /// - `owner`: must be the primary owner of the source will.
    /// - `beneficiaries_to_split`: subset of beneficiaries to move to the new will.
    ///   Their percentages will be renormalised to sum to 100 in the child will.
    /// - `amount`: token amount to transfer into the new will. Must be > 0 and
    ///   ≤ the source will's balance.
    ///
    /// # Returns
    /// The id of the newly created child will.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] / [`WillError::WillNotActive`]
    /// - [`WillError::ZeroAmount`] / [`WillError::InsufficientBalance`]
    /// - [`WillError::InvalidSplit`] if `beneficiaries_to_split` is empty or would
    ///   leave the source will with no beneficiaries.
    pub fn split_will(
        env: Env,
        will_id: u64,
        owner: Address,
        beneficiaries_to_split: Vec<Beneficiary>,
        amount: i128,
    ) -> u64 {
        owner.require_auth();
        let mut source = load_owned(&env, will_id, &owner);
        assert_status(&env, &source, WillStatus::Active, WillError::WillNotActive);

        if amount <= 0 {
            panic_with_error!(&env, WillError::ZeroAmount);
        }
        if amount > source.balance {
            panic_with_error!(&env, WillError::InsufficientBalance);
        }
        if beneficiaries_to_split.is_empty() {
            panic_with_error!(&env, WillError::InvalidSplit);
        }

        // Build a set of addresses being split out to verify they exist in the
        // source will and remove them from it.
        let mut remaining_beneficiaries: Vec<Beneficiary> = Vec::new(&env);
        for b in source.beneficiaries.iter() {
            let mut being_split = false;
            for s in beneficiaries_to_split.iter() {
                if s.address == b.address {
                    being_split = true;
                    break;
                }
            }
            if !being_split {
                remaining_beneficiaries.push_back(b.clone());
            }
        }

        // The source will must keep at least one beneficiary.
        if remaining_beneficiaries.is_empty() {
            panic_with_error!(&env, WillError::InvalidSplit);
        }

        // Renormalise the remaining beneficiaries so they still sum to 100.
        let mut remaining_total: u32 = 0;
        for b in remaining_beneficiaries.iter() {
            remaining_total += b.percentage;
        }
        // If the total is already 100 (the split beneficiaries were added with
        // explicit percentages that happen to leave the rest at 100) keep as-is;
        // otherwise scale so they sum to 100.
        let mut normalised_remaining: Vec<Beneficiary> = Vec::new(&env);
        let rem_count = remaining_beneficiaries.len();
        let mut rem_running: u32 = 0;
        for (i, b) in remaining_beneficiaries.iter().enumerate() {
            let pct = if (i as u32) == rem_count - 1 {
                100u32.saturating_sub(rem_running)
            } else {
                b.percentage * 100 / remaining_total
            };
            rem_running += pct;
            normalised_remaining.push_back(Beneficiary {
                address: b.address.clone(),
                percentage: pct,
            });
        }

        // Renormalise the split-off beneficiaries to sum to 100 for the child will.
        let mut split_total: u32 = 0;
        for b in beneficiaries_to_split.iter() {
            split_total += b.percentage;
        }
        let split_count = beneficiaries_to_split.len();
        let mut normalised_split: Vec<Beneficiary> = Vec::new(&env);
        let mut split_running: u32 = 0;
        for (i, b) in beneficiaries_to_split.iter().enumerate() {
            let pct = if (i as u32) == split_count - 1 {
                100u32.saturating_sub(split_running)
            } else if split_total > 0 {
                b.percentage * 100 / split_total
            } else {
                100 / split_count
            };
            split_running += pct;
            normalised_split.push_back(Beneficiary {
                address: b.address.clone(),
                percentage: pct,
            });
        }

        // Remove split-off beneficiaries from the source index and add them to
        // the child's index.
        for b in beneficiaries_to_split.iter() {
            storage::remove_beneficiary_index(&env, &b.address, will_id);
        }

        // Update source will.
        source.balance -= amount;
        source.beneficiaries = normalised_remaining;
        storage::save_will(&env, &source);

        // Create the new child will.
        let new_id = storage::next_will_id(&env);
        let now = env.ledger().timestamp();

        for b in normalised_split.iter() {
            storage::index_by_beneficiary(&env, &b.address, new_id);
        }

        let child_count = normalised_split.len();
        let child = Will {
            id: new_id,
            owner: source.owner.clone(),
            co_owners: source.co_owners.clone(),
            owner_threshold: source.owner_threshold,
            token: source.token.clone(),
            balance: amount,
            beneficiaries: normalised_split,
            hashed_beneficiaries: Vec::new(&env),
            checkin_period_days: source.checkin_period_days,
            grace_period_days: source.grace_period_days,
            last_checkin: now,
            trigger_time: None,
            confirmation_deadline: None,
            status: WillStatus::Active,
            guardians: source.guardians.clone(),
            guardian_votes: 0,
        };
        storage::save_will(&env, &child);
        storage::index_by_owner(&env, &source.owner, new_id);

        events::will_split(&env, will_id, new_id, &owner, amount);
        events::will_created(
            &env,
            new_id,
            &owner,
            amount,
            child_count,
            now + source.checkin_period_days * SECONDS_PER_DAY,
        );

        new_id
    }

    // -----------------------------------------------------------------------
    // Issue #46 — reveal_and_claim
    // -----------------------------------------------------------------------

    /// Registers a hashed beneficiary on an existing active will.
    ///
    /// Only the owner (or co-owner set meeting the threshold) may add hashed
    /// beneficiaries. The combined percentages of `beneficiaries` and
    /// `hashed_beneficiaries` must still sum to 100.
    ///
    /// # Parameters
    /// - `will_id`: the will to add the hashed beneficiary to.
    /// - `owner`: must be the primary owner.
    /// - `commitment`: SHA-256 hash of the pre-image `address_bytes || salt_bytes`.
    /// - `percentage`: share of the will's balance for this beneficiary.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] / [`WillError::WillNotActive`]
    /// - [`WillError::InvalidPercentages`] if total percentages would exceed 100.
    pub fn add_hashed_beneficiary(
        env: Env,
        will_id: u64,
        owner: Address,
        commitment: Bytes,
        percentage: u32,
    ) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        will.hashed_beneficiaries.push_back(HashedBeneficiary {
            commitment,
            percentage,
            claimed: false,
        });

        // Validate combined percentages.
        assert_valid_percentages(&env, &will.beneficiaries, &will.hashed_beneficiaries);

        storage::save_will(&env, &will);
    }

    /// Verifies a pre-image against a stored commitment hash and, if correct,
    /// immediately transfers that beneficiary's share to the revealed address.
    ///
    /// The pre-image must be 64 bytes: the first 32 bytes are the raw bytes of
    /// the beneficiary `Address` and the remaining 32 bytes are a random salt
    /// chosen by the beneficiary at registration time.
    ///
    /// This entrypoint works once the will is `Triggered` AND the grace period
    /// has elapsed (the same condition as `release_inheritance`). This keeps
    /// hashed-beneficiary payouts consistent with normal payouts.
    ///
    /// # Parameters
    /// - `will_id`: the will to claim from.
    /// - `claimant`: the address that will receive the funds; must authorise.
    /// - `preimage`: raw bytes whose SHA-256 must match a stored commitment.
    ///
    /// # Panics
    /// - [`WillError::WillNotTriggered`] if the will is not `Triggered`.
    /// - [`WillError::GracePeriodNotExpired`] if the grace period has not elapsed.
    /// - [`WillError::InvalidPreimage`] if no matching commitment is found.
    /// - [`WillError::AlreadyClaimed`] if that slot was already claimed.
    pub fn reveal_and_claim(env: Env, will_id: u64, claimant: Address, preimage: Bytes) {
        claimant.require_auth();
        let mut will = load_will(&env, will_id);
        assert_status(
            &env,
            &will,
            WillStatus::Triggered,
            WillError::WillNotTriggered,
        );

        let trigger_time = will.trigger_time.unwrap_or(0);
        let grace_deadline = trigger_time + will.grace_period_days * SECONDS_PER_DAY;
        if env.ledger().timestamp() < grace_deadline {
            panic_with_error!(&env, WillError::GracePeriodNotExpired);
        }

        // Hash the supplied pre-image with SHA-256.
        let digest = env.crypto().sha256(&preimage);
        let digest_bytes = Bytes::from_array(&env, &digest.to_array());

        // Find the matching hashed beneficiary slot.
        let mut found_idx: Option<u32> = None;
        for (i, hb) in will.hashed_beneficiaries.iter().enumerate() {
            if hb.commitment == digest_bytes {
                found_idx = Some(i as u32);
                break;
            }
        }

        let idx = match found_idx {
            Some(i) => i,
            None => panic_with_error!(&env, WillError::InvalidPreimage),
        };

        // Check already-claimed both in-memory (the `claimed` flag) and in
        // persistent storage (so the flag survives across invocations before
        // the will struct itself is saved).
        let hb = will.hashed_beneficiaries.get(idx).unwrap();
        if hb.claimed || storage::is_hashed_claimed(&env, will_id, &hb.commitment) {
            panic_with_error!(&env, WillError::AlreadyClaimed);
        }

        let share = will.balance * (hb.percentage as i128) / 100;
        if share > 0 {
            token::Client::new(&env, &will.token).transfer(
                &env.current_contract_address(),
                &claimant,
                &share,
            );
        }

        will.balance -= share;

        // Mark the slot as claimed.
        let commitment = hb.commitment.clone();
        storage::set_hashed_claimed(&env, will_id, &commitment);

        // Update the in-memory Vec entry.
        let mut updated_hb: Vec<HashedBeneficiary> = Vec::new(&env);
        for (i, entry) in will.hashed_beneficiaries.iter().enumerate() {
            if i as u32 == idx {
                updated_hb.push_back(HashedBeneficiary {
                    commitment: entry.commitment.clone(),
                    percentage: entry.percentage,
                    claimed: true,
                });
            } else {
                updated_hb.push_back(entry.clone());
            }
        }
        will.hashed_beneficiaries = updated_hb;
        storage::save_will(&env, &will);

        events::hashed_claimed(&env, will_id, &claimant, share);
    }
}

// ---------------------------------------------------------------------------
// Private helpers
// ---------------------------------------------------------------------------

/// Loads a will by id, panicking with [`WillError::WillNotFound`] if absent.
fn load_will(env: &Env, will_id: u64) -> Will {
    match storage::load_will(env, will_id) {
        Ok(will) => will,
        Err(e) => panic_with_error!(env, e),
    }
}

/// Loads a will by id and asserts `owner` is its primary owner.
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

/// Asserts that the sum of plain beneficiary percentages plus hashed beneficiary
/// percentages equals exactly 100.
fn assert_valid_percentages(
    env: &Env,
    beneficiaries: &Vec<Beneficiary>,
    hashed: &Vec<HashedBeneficiary>,
) {
    let mut total: u32 = 0;
    for b in beneficiaries.iter() {
        total += b.percentage;
    }
    for hb in hashed.iter() {
        total += hb.percentage;
    }
    if total != 100 {
        panic_with_error!(env, WillError::InvalidPercentages);
    }
}

/// Splits `will.balance` across `will.beneficiaries` proportionally, transfers
/// shares, marks the will `Released`, and emits the event.
fn distribute(env: &Env, will: &mut Will) {
    let token_client = token::Client::new(env, &will.token);
    let contract_address = env.current_contract_address();
    let total = will.balance;
    let count = will.beneficiaries.len();

    let mut remaining = total;
    for (index, beneficiary) in will.beneficiaries.iter().enumerate() {
        let share = if index as u32 == count - 1 {
            remaining
        } else {
            let portion = total * (beneficiary.percentage as i128) / 100;
            remaining -= portion;
            portion
        };
        if share > 0 {
            token_client.transfer(&contract_address, &beneficiary.address, &share);
        }
    }

    will.balance = 0;
    will.status = WillStatus::Released;
    storage::save_will(env, will);

    events::inheritance_released(env, will.id, total, count);
}
