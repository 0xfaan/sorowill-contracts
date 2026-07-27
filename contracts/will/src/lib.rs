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
//! Optionally, up to three guardians (with tier distinction: primary or
//! backup) may be named on a will; any two of them calling
//! [`WillContract::guardian_trigger`] force an immediate release.
//!
//! The owner may designate a delegate to check in on their behalf, perform
//! partial early releases to a subset of beneficiaries while the will
//! remains active, and optionally configure a vesting schedule so that the
//! inheritance unlocks gradually instead of in a single lump sum.

mod errors;
mod events;
mod storage;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Vec};

pub use errors::WillError;
pub use types::{Beneficiary, GuardianEntry, GuardianTier, VestingSchedule, Will, WillStatus};

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
    /// Creates a new will, locking `amount` of `token` in the contract.
    ///
    /// # Parameters
    /// - `owner`: the address creating the will; must authorize this call.
    /// - `token`: the token contract address (e.g. a USDC Stellar Asset Contract).
    /// - `amount`: the amount of `token` to lock, in the token's base units. Must be positive.
    /// - `beneficiaries`: 1 to `MAX_BENEFICIARIES` entries whose basis points sum to exactly 10,000.
    /// - `checkin_period_days`: how many days the owner may go without checking in.
    /// - `grace_period_days`: how many days after being triggered the owner has to prove they are alive.
    /// - `guardians`: 0 to `MAX_GUARDIANS` guardian entries (address + tier).
    /// - `delegate`: optional delegate address that may check in on the owner's behalf.
    /// - `vesting_duration_days`: if set, the inheritance unlocks linearly over this many days
    ///   after the grace period expires, instead of being released as a lump sum.
    ///
    /// # Returns
    /// The newly allocated will id.
    ///
    /// # Panics
    /// - [`WillError::ZeroAmount`] if `amount` is not positive.
    /// - [`WillError::TooManyBeneficiaries`] if the beneficiary list is empty or too large,
    ///   or if too many guardians are supplied.
    /// - [`WillError::InvalidPercentages`] if beneficiary basis points do not sum to 10,000.
    #[allow(clippy::too_many_arguments)]
    pub fn create_will(
        env: Env,
        owner: Address,
        token: Address,
        amount: i128,
        beneficiaries: Vec<Beneficiary>,
        checkin_period_days: u64,
        grace_period_days: u64,
        guardians: Vec<GuardianEntry>,
        delegate: Option<Address>,
        vesting_duration_days: Option<u64>,
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
        assert_valid_percentages(&env, &beneficiaries);

        let will_id = storage::next_will_id(&env);
        let now = env.ledger().timestamp();

        token::Client::new(&env, &token).transfer(&owner, &env.current_contract_address(), &amount);

        let beneficiaries_count = beneficiaries.len();
        for beneficiary in beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &beneficiary.address, will_id);
        }

        let vesting = vesting_duration_days.map(|days| VestingSchedule {
            start_time: 0, // set when grace period expires
            duration_seconds: days * SECONDS_PER_DAY,
            released_amount: 0,
        });

        let will = Will {
            id: will_id,
            owner: owner.clone(),
            token,
            balance: amount,
            beneficiaries,
            checkin_period_days,
            grace_period_days,
            last_checkin: now,
            trigger_time: None,
            status: WillStatus::Active,
            guardians,
            guardian_votes: 0,
            delegate,
            vesting,
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
        let grace_period_ends = now + will.grace_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

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

        will.status = WillStatus::Active;
        will.trigger_time = None;
        will.last_checkin = now;
        will.guardian_votes = 0;
        storage::reset_guardian_votes(&env, will_id, &will.guardians);
        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        events::emergency_checkin(&env, will_id, &owner, next_deadline);
    }

    /// Distributes the will's balance to all beneficiaries proportionally to
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
        let now = env.ledger().timestamp();
        if now < grace_deadline {
            panic_with_error!(&env, WillError::GracePeriodNotExpired);
        }

        if will.vesting.is_some() {
            // Start vesting: record the start time and transition to Vesting.
            let vesting = will.vesting.as_mut().unwrap();
            vesting.start_time = now;
            will.status = WillStatus::Vesting;
            will.trigger_time = None;
            storage::save_will(&env, &will);

            events::vesting_started(&env, will_id, now, vesting.duration_seconds);
        } else {
            distribute(&env, &mut will);
        }
    }

    /// Claims the vested portion of the will's balance for the caller.
    /// The caller must be one of the will's beneficiaries. The releasable
    /// amount is calculated linearly based on elapsed time since vesting
    /// started, proportionally to the beneficiary's basis-point share.
    ///
    /// Callable by anyone once the will is in `Vesting` status. If the
    /// full amount has vested, this completes the release and marks the
    /// will `Released`.
    ///
    /// # Panics
    /// - [`WillError::WillNotActive`] if the will is not in `Vesting` status.
    /// - [`WillError::NothingVested`] if no time has elapsed since vesting started.
    /// - [`WillError::FullyReleased`] if the balance is already zero.
    /// - [`WillError::InvalidReleaseBeneficiaries`] if the caller is not a beneficiary.
    pub fn claim_vested(env: Env, will_id: u64, claimer: Address) {
        claimer.require_auth();
        let mut will = load_will(&env, will_id);
        assert_status(&env, &will, WillStatus::Vesting, WillError::WillNotActive);

        if will.balance <= 0 {
            panic_with_error!(&env, WillError::FullyReleased);
        }

        let vesting = will.vesting.as_ref().unwrap();
        let now = env.ledger().timestamp();
        if now <= vesting.start_time {
            panic_with_error!(&env, WillError::NothingVested);
        }

        let elapsed = now - vesting.start_time;
        let total_amount = will.balance + vesting.released_amount;
        let duration = vesting.duration_seconds;

        let claimer_share = find_beneficiary_share(&env, &will, &claimer);

        let vested_total = if elapsed >= duration {
            total_amount
        } else {
            total_amount * (elapsed as i128) / (duration as i128)
        };

        let claimable = vested_total - vesting.released_amount;
        let claimer_amount = claimable * (claimer_share as i128) / 10_000;

        if claimer_amount <= 0 {
            panic_with_error!(&env, WillError::NothingVested);
        }

        let token_client = token::Client::new(&env, &will.token);
        token_client.transfer(
            &env.current_contract_address(),
            &claimer,
            &claimer_amount,
        );

        will.balance -= claimer_amount;
        let vesting = will.vesting.as_mut().unwrap();
        vesting.released_amount += claimer_amount;

        if will.balance <= 0 {
            will.balance = 0;
            will.status = WillStatus::Released;
        }
        storage::save_will(&env, &will);

        events::vested_claim(&env, will_id, &claimer, claimer_amount, will.balance);
    }

    /// Performs a partial early release: the owner proactively distributes a
    /// fraction of the locked balance to a specified subset of beneficiaries
    /// while the will remains `Active`. Each selected beneficiary receives
    /// their proportionate share of `amount` based on their basis points
    /// relative to the sum of the selected beneficiaries' basis points.
    ///
    /// # Parameters
    /// - `will_id`: the will to partially release from.
    /// - `owner`: the will's owner; must authorize this call.
    /// - `amount`: the total amount to release. Must be positive and ≤ will.balance.
    /// - `beneficiary_addresses`: the addresses of the subset of beneficiaries
    ///   who should receive this early release. Must be a non-empty subset of
    ///   the will's beneficiary list.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::ZeroPartialRelease`] if `amount` is not positive.
    /// - [`WillError::InsufficientBalance`] if `amount` exceeds the will's balance.
    /// - [`WillError::InvalidReleaseBeneficiaries`] if no valid beneficiary addresses are supplied.
    pub fn partial_release(
        env: Env,
        will_id: u64,
        owner: Address,
        amount: i128,
        beneficiary_addresses: Vec<Address>,
    ) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        if amount <= 0 {
            panic_with_error!(&env, WillError::ZeroPartialRelease);
        }
        if amount > will.balance {
            panic_with_error!(&env, WillError::InsufficientBalance);
        }
        if beneficiary_addresses.is_empty() {
            panic_with_error!(&env, WillError::InvalidReleaseBeneficiaries);
        }

        // Validate that all supplied addresses are actual beneficiaries.
        let mut selected_bp_sum: u32 = 0;
        for addr in beneficiary_addresses.iter() {
            let bp = find_beneficiary_share(&env, &will, &addr);
            selected_bp_sum += bp;
        }
        if selected_bp_sum == 0 {
            panic_with_error!(&env, WillError::InvalidReleaseBeneficiaries);
        }

        let token_client = token::Client::new(&env, &will.token);
        let contract_address = env.current_contract_address();
        let mut distributed: i128 = 0;
        let selected_count = beneficiary_addresses.len();

        for (index, addr) in beneficiary_addresses.iter().enumerate() {
            let bp = find_beneficiary_share(&env, &will, &addr);
            let share = if index as u32 == selected_count - 1 {
                amount - distributed
            } else {
                amount * (bp as i128) / (selected_bp_sum as i128)
            };
            distributed += share;
            if share > 0 {
                token_client.transfer(&contract_address, &addr, &share);
            }
        }

        will.balance -= amount;
        storage::save_will(&env, &will);

        events::partial_release(
            &env,
            will_id,
            amount,
            selected_count,
            will.balance,
        );
    }

    /// Cancels the will and refunds the full locked balance to the owner.
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

    /// Replaces the beneficiary list for `will_id`. Only possible while the
    /// will is `Active`. The new basis points must sum to exactly 10,000.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::TooManyBeneficiaries`] if the new list is empty or too large.
    /// - [`WillError::InvalidPercentages`] if the new basis points do not sum to 10,000.
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

    /// Replaces the guardian list for `will_id`. Only possible while the will
    /// is `Active`. Any votes cast against the previous guardian list are
    /// cleared so every updated list starts a fresh voting cycle.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::TooManyBeneficiaries`] if more than `MAX_GUARDIANS`
    ///   guardians are supplied.
    pub fn update_guardians(
        env: Env,
        will_id: u64,
        owner: Address,
        guardians: Vec<GuardianEntry>,
    ) {
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

    /// Adds `amount` more of the will's token to its locked balance. Only
    /// possible while the will is `Active`.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::ZeroAmount`] if `amount` is not positive.
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
    ///
    /// # Panics
    /// - [`WillError::WillNotFound`] if no will exists with this id.
    pub fn get_will(env: Env, will_id: u64) -> Will {
        load_will(&env, will_id)
    }

    /// Returns the full state of every will owned by `owner`.
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
    /// when the owner is known to be incapacitated.
    ///
    /// **Tier logic**: Primary guardians count immediately toward the
    /// [`GUARDIAN_THRESHOLD`]. Backup guardians may only vote if no primary
    /// guardians exist in the will's guardian list. Once the threshold is
    /// reached, the balance is distributed (or vesting begins, if configured).
    ///
    /// # Panics
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::NotGuardian`] if `guardian` is not one of the will's guardians.
    /// - [`WillError::AlreadyVoted`] if `guardian` already voted in this cycle.
    /// - [`WillError::BackupGuardianUnavailable`] if a backup guardian tries to vote
    ///   while primary guardians are present.
    pub fn guardian_trigger(env: Env, will_id: u64, guardian: Address) {
        guardian.require_auth();
        let mut will = load_will(&env, will_id);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        let entry = find_guardian_entry(&env, &will, &guardian);

        // Enforce tier rules: backup guardians cannot vote while primaries exist.
        if entry.tier == GuardianTier::Backup && has_primary_guardians(&will) {
            panic_with_error!(&env, WillError::BackupGuardianUnavailable);
        }

        if storage::has_guardian_voted(&env, will_id, &guardian) {
            panic_with_error!(&env, WillError::AlreadyVoted);
        }

        storage::set_guardian_voted(&env, will_id, &guardian);
        will.guardian_votes += 1;
        storage::save_will(&env, &will);

        events::guardian_voted(&env, will_id, &guardian, will.guardian_votes);

        if will.guardian_votes >= GUARDIAN_THRESHOLD {
            if will.vesting.is_some() {
                // Start vesting instead of lump-sum release.
                let now = env.ledger().timestamp();
                let vesting = will.vesting.as_mut().unwrap();
                vesting.start_time = now;
                will.status = WillStatus::Vesting;
                will.trigger_time = None;
                will.guardian_votes = 0;
                storage::reset_guardian_votes(&env, will_id, &will.guardians);
                storage::save_will(&env, &will);
                events::vesting_started(&env, will_id, now, vesting.duration_seconds);
            } else {
                distribute(&env, &mut will);
            }
        }
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

/// Asserts that `caller` is either the will's owner or its designated delegate.
fn assert_owner_or_delegate(env: &Env, will: &Will, caller: &Address) {
    if &will.owner == caller {
        return;
    }
    match &will.delegate {
        Some(delegate) if delegate == caller => return,
        _ => {}
    }
    panic_with_error!(env, WillError::NotOwnerOrDelegate);
}

/// Asserts beneficiary basis points sum to exactly 10,000.
fn assert_valid_percentages(env: &Env, beneficiaries: &Vec<Beneficiary>) {
    let mut total: u32 = 0;
    for beneficiary in beneficiaries.iter() {
        total += beneficiary.basis_points;
    }
    if total != 10_000 {
        panic_with_error!(env, WillError::InvalidPercentages);
    }
}

/// Returns the basis-point share of `address` in the will's beneficiary list.
/// Panics if `address` is not a beneficiary.
fn find_beneficiary_share(env: &Env, will: &Will, address: &Address) -> u32 {
    for b in will.beneficiaries.iter() {
        if b.address == *address {
            return b.basis_points;
        }
    }
    panic_with_error!(env, WillError::InvalidReleaseBeneficiaries);
}

/// Finds the `GuardianEntry` for `address` in the will's guardian list.
/// Panics if not found.
fn find_guardian_entry(env: &Env, will: &Will, address: &Address) -> GuardianEntry {
    for entry in will.guardians.iter() {
        if entry.address == *address {
            return entry;
        }
    }
    panic_with_error!(env, WillError::NotGuardian);
}

/// Returns `true` if the will has at least one primary guardian.
fn has_primary_guardians(will: &Will) -> bool {
    for entry in will.guardians.iter() {
        if entry.tier == GuardianTier::Primary {
            return true;
        }
    }
    false
}

/// Splits `will.balance` across `will.beneficiaries` proportionally to their
/// basis-point shares, transfers the shares out of the contract, marks the
/// will `Released`, and publishes the `InheritanceReleased` event. Any
/// rounding remainder from integer division is paid to the final beneficiary.
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
            let portion = total * (beneficiary.basis_points as i128) / 10_000;
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
