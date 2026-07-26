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

mod errors;
mod events;
mod storage;
mod types;

#[cfg(test)]
mod test;

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Vec};

pub use errors::WillError;
pub use types::{Beneficiary, Will, WillStatus};

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
    /// - `beneficiaries`: 1 to `MAX_BENEFICIARIES` entries whose percentages sum to exactly 100.
    /// - `checkin_period_days`: how many days the owner may go without checking in.
    /// - `grace_period_days`: how many days after being triggered the owner has to prove they are alive.
    /// - `guardians`: 0 to `MAX_GUARDIANS` addresses that may jointly force an early release.
    ///
    /// # Returns
    /// The newly allocated will id.
    ///
    /// # Panics
    /// - [`WillError::ZeroAmount`] if `amount` is not positive.
    /// - [`WillError::TooManyBeneficiaries`] if the beneficiary list is empty or too large,
    ///   or if too many guardians are supplied.
    /// - [`WillError::InvalidPercentages`] if beneficiary percentages do not sum to 100.
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
    /// will is `Active`. The new percentages must sum to exactly 100.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::TooManyBeneficiaries`] if the new list is empty or too large.
    /// - [`WillError::InvalidPercentages`] if the new percentages do not sum to 100.
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
    /// [`GUARDIAN_THRESHOLD`] distinct guardians have voted, the balance is
    /// immediately distributed to beneficiaries, bypassing the check-in and
    /// grace-period flow entirely.
    ///
    /// # Panics
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::NotGuardian`] if `guardian` is not one of the will's guardians.
    /// - [`WillError::AlreadyVoted`] if `guardian` already voted in this cycle.
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

/// Asserts beneficiary percentages sum to exactly 100.
fn assert_valid_percentages(env: &Env, beneficiaries: &Vec<Beneficiary>) {
    let mut total: u32 = 0;
    for beneficiary in beneficiaries.iter() {
        total += beneficiary.percentage;
    }
    if total != 100 {
        panic_with_error!(env, WillError::InvalidPercentages);
    }
}

/// Splits `will.balance` across `will.beneficiaries` proportionally to their
/// percentages, transfers the shares out of the contract, marks the will
/// `Released`, and publishes the `InheritanceReleased` event. Any rounding
/// remainder from integer division is paid to the final beneficiary.
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
