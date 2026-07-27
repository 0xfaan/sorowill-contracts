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
//! known to be incapacitated). Guardian votes expire after a configurable
//! window so stale votes cannot combine with fresh ones.
//!
//! Grace periods may optionally be split into multiple tiers, each releasing
//! a configurable percentage of the balance at a different time offset.

mod errors;
mod events;
mod storage;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Vec};

pub use errors::WillError;
pub use types::{Beneficiary, GraceTier, GuardianVoteReason, Will, WillStatus};

/// Number of seconds in a day, used to convert the day-denominated periods
/// stored on a `Will` into absolute ledger timestamps.
const SECONDS_PER_DAY: u64 = 86_400;

/// Maximum number of beneficiaries a single will may have.
const MAX_BENEFICIARIES: u32 = 10;

/// Maximum number of guardians a single will may have.
const MAX_GUARDIANS: u32 = 3;

/// Number of distinct guardian votes required to force an early release.
const GUARDIAN_THRESHOLD: u32 = 2;

/// Maximum number of grace-period tiers a single will may have.
const MAX_GRACE_TIERS: u32 = 10;

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
    /// - `guardians`: 0 to `MAX_GUARDIANS` addresses that may jointly force an early release.
    /// - `guardian_vote_expiry_days`: how many days a guardian vote remains valid. Pass 0 to default
    ///   to `grace_period_days`.
    /// - `grace_tiers`: optional multi-tier payout schedule. When empty the legacy single-release
    ///   behaviour applies. When provided, basis points must sum to 10,000 and day offsets must be
    ///   strictly ascending and all ≤ `grace_period_days * 86400`.
    ///
    /// # Returns
    /// The newly allocated will id.
    ///
    /// # Panics
    /// - [`WillError::ZeroAmount`] if `amount` is not positive.
    /// - [`WillError::TooManyBeneficiaries`] if the beneficiary list is empty or too large,
    ///   or if too many guardians are supplied.
    /// - [`WillError::InvalidPercentages`] if beneficiary basis points do not sum to 10,000.
    /// - [`WillError::InvalidGraceTiers`] if grace tiers are malformed.
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
        guardian_vote_expiry_days: u32,
        grace_tiers: Vec<GraceTier>,
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

        let effective_expiry = if guardian_vote_expiry_days == 0 {
            grace_period_days
        } else {
            guardian_vote_expiry_days as u64
        };

        if !grace_tiers.is_empty() {
            validate_grace_tiers(&env, &grace_tiers, grace_period_days);
        }

        let will_id = storage::next_will_id(&env);
        let now = env.ledger().timestamp();

        token::Client::new(&env, &token).transfer(&owner, &env.current_contract_address(), &amount);

        let beneficiaries_count = beneficiaries.len();
        for beneficiary in beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &beneficiary.address, will_id);
        }

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
            guardian_vote_expiry_days: effective_expiry,
            grace_tiers,
            released_basis_points: 0,
            trigger_balance: 0,
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
    /// will's owner, and the will must be `Active`.
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
        will.released_basis_points = 0;
        will.trigger_balance = 0;
        storage::reset_guardian_votes(&env, will_id, &will.guardians);
        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        events::emergency_checkin(&env, will_id, &owner, next_deadline);
    }

    /// Distributes the will's balance to all beneficiaries proportionally to
    /// their configured percentages. Callable by anyone once the grace
    /// period has fully elapsed. Any rounding remainder from integer
    /// division is paid to the final beneficiary so the full balance is
    /// always distributed with no dust left behind.
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

    /// Releases the payout for a specific grace-period tier. Callable by
    /// anyone once the tier's time offset (relative to `trigger_time`) has
    /// been reached. Tiers must be released in order (tier 0, then 1, etc.)
    /// and each tier can only be released once.
    ///
    /// For a will without grace tiers this function panics with
    /// [`WillError::InvalidGraceTiers`]. For a will whose tiers have all
    /// been released, this panics with [`WillError::TierAlreadyReleased`].
    ///
    /// # Panics
    /// - [`WillError::WillNotTriggered`] if the will is not `Triggered`.
    /// - [`WillError::InvalidGraceTiers`] if the will has no grace tiers.
    /// - [`WillError::InvalidTierIndex`] if the tier index is out of range.
    /// - [`WillError::TierAlreadyReleased`] if this tier was already released.
    /// - [`WillError::GracePeriodNotExpired`] if the tier's time offset has not yet been reached.
    pub fn release_tier(env: Env, will_id: u64, tier_index: u32) {
        let mut will = load_will(&env, will_id);
        assert_status(
            &env,
            &will,
            WillStatus::Triggered,
            WillError::WillNotTriggered,
        );

        if will.grace_tiers.is_empty() {
            panic_with_error!(&env, WillError::InvalidGraceTiers);
        }
        if tier_index >= will.grace_tiers.len() {
            panic_with_error!(&env, WillError::InvalidTierIndex);
        }
        if tier_index < will.released_basis_points {
            panic_with_error!(&env, WillError::TierAlreadyReleased);
        }

        let trigger_time = will.trigger_time.unwrap_or(0);
        let tier = will.grace_tiers.get(tier_index).unwrap();
        let tier_deadline = trigger_time + tier.day_offset;
        let now = env.ledger().timestamp();
        if now < tier_deadline {
            panic_with_error!(&env, WillError::GracePeriodNotExpired);
        }

        let amount = will.trigger_balance * (tier.basis_points as i128) / 10_000;
        if amount > will.balance {
            panic_with_error!(&env, WillError::InsufficientBalance);
        }

        distribute_tier(&env, &mut will, amount);

        will.released_basis_points = tier_index + 1;

        if will.released_basis_points >= will.grace_tiers.len() {
            will.status = WillStatus::Released;
        }
        storage::save_will(&env, &will);

        events::tier_released(
            &env,
            will_id,
            tier_index,
            amount,
            will.released_basis_points,
        );

        if will.status == WillStatus::Released {
            events::inheritance_released(&env, will_id, will.trigger_balance, will.beneficiaries.len());
        }
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
    /// when the owner is known to be incapacitated. Once
    /// [`GUARDIAN_THRESHOLD`] distinct non-expired guardians have voted, the
    /// balance is immediately distributed to beneficiaries, bypassing the
    /// check-in and grace-period flow entirely.
    ///
    /// Votes expire after `guardian_vote_expiry_days` (configured at will
    /// creation). Expired votes are silently skipped when counting quorum,
    /// preventing a stale vote from combining with a new one.
    ///
    /// # Parameters
    /// - `will_id`: the will to vote on.
    /// - `guardian`: the voting guardian's address; must authorize this call.
    /// - `reason`: an optional reason code for the vote (e.g. `Incapacitated`).
    ///
    /// # Panics
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::NotGuardian`] if `guardian` is not one of the will's guardians.
    /// - [`WillError::AlreadyVoted`] if `guardian` already has a non-expired vote.
    pub fn guardian_trigger(env: Env, will_id: u64, guardian: Address, reason: GuardianVoteReason) {
        guardian.require_auth();
        let mut will = load_will(&env, will_id);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        if !will.guardians.contains(&guardian) {
            panic_with_error!(&env, WillError::NotGuardian);
        }

        let now = env.ledger().timestamp();
        if storage::has_guardian_voted(&env, will_id, &guardian, now, will.guardian_vote_expiry_days)
        {
            panic_with_error!(&env, WillError::AlreadyVoted);
        }

        storage::set_guardian_voted(&env, will_id, &guardian, now, reason);

        let valid_votes = storage::count_valid_guardian_votes(
            &env,
            will_id,
            &will.guardians,
            now,
            will.guardian_vote_expiry_days,
        );
        will.guardian_votes = valid_votes;
        storage::save_will(&env, &will);

        events::guardian_voted(&env, will_id, &guardian, valid_votes, reason);

        if valid_votes >= GUARDIAN_THRESHOLD {
            distribute(&env, &mut will);
        }
    }
}

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

/// Validates that grace tiers are well-formed:
/// - Non-empty
/// - Sorted by strictly ascending `day_offset`
/// - `day_offset` values all within the grace period
/// - Basis points sum to exactly 10,000
fn validate_grace_tiers(env: &Env, tiers: &Vec<GraceTier>, grace_period_days: u64) {
    if tiers.is_empty() || tiers.len() > MAX_GRACE_TIERS {
        panic_with_error!(env, WillError::InvalidGraceTiers);
    }

    let grace_period_secs = grace_period_days * SECONDS_PER_DAY;
    let mut total_bp: u32 = 0;
    let mut prev_offset: u64 = 0;

    for (i, tier) in tiers.iter().enumerate() {
        if tier.day_offset == 0 || tier.day_offset > grace_period_secs {
            panic_with_error!(env, WillError::InvalidGraceTiers);
        }
        if tier.basis_points == 0 || tier.basis_points > 10_000 {
            panic_with_error!(env, WillError::InvalidGraceTiers);
        }
        if i > 0 && tier.day_offset <= prev_offset {
            panic_with_error!(env, WillError::InvalidGraceTiers);
        }
        total_bp += tier.basis_points;
        prev_offset = tier.day_offset;
    }

    if total_bp != 10_000 {
        panic_with_error!(env, WillError::InvalidGraceTiers);
    }
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
