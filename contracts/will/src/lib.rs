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
//! known to be incapacitated).

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

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Map, Vec};

pub use errors::WillError;
pub use types::{Beneficiary, Will, WillStatus};

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

#[contract]
pub struct WillContract;

#[contractimpl]
impl WillContract {
    /// Creates a new will, locking one or more token balances in the contract.
    ///
    /// # Parameters
    /// - `owner`: the address creating the will; must authorize this call.
    /// - `tokens`: a list of `(token_address, amount)` pairs to lock. Each
    ///   token address must be unique, each amount must be positive, and the
    ///   list must contain between 1 and `MAX_TOKENS` entries.
    /// - `beneficiaries`: 1 to `MAX_BENEFICIARIES` entries whose percentages sum to exactly 100.
    /// - `token`: the token contract address (e.g. a USDC Stellar Asset Contract).
    /// - `amount`: the amount of `token` to lock, in the token's base units. Must be positive.
    /// - `beneficiaries`: 1 to `MAX_BENEFICIARIES` entries, each with a share of
    ///   1 to 100, whose percentages sum to exactly 100.
    /// - `checkin_period_days`: how many days the owner may go without checking
    ///   in; 1 to `MAX_PERIOD_DAYS`.
    /// - `grace_period_days`: how many days after being triggered the owner has
    ///   to prove they are alive; 1 to `MAX_PERIOD_DAYS`.
    /// - `guardians`: 0 to `MAX_GUARDIANS` distinct addresses that may jointly
    ///   force an early release.
    /// - `beneficiaries`: 1 to `MAX_BENEFICIARIES` entries whose basis points sum to exactly 10,000.
    /// - `checkin_period_days`: how many days the owner may go without checking in.
    /// - `grace_period_days`: how many days after being triggered the owner has to prove they are alive.
    /// - `guardians`: 0 to `MAX_GUARDIANS` addresses that may jointly force an early release.
    /// - `is_native`: whether the asset is native XLM. When `true`, transfers use
    ///   `env.transfer()` instead of the token client; the `token` address is still
    ///   stored but the native path is used for all balance movements.
    ///
    /// # Returns
    /// The newly allocated will id.
    ///
    /// # Panics
    /// - [`WillError::ZeroAmount`] if any token amount is not positive.
    /// - [`WillError::TooManyBeneficiaries`] if the beneficiary/guardian/token lists are
    ///   empty or exceed their respective caps.
    /// - [`WillError::InvalidPercentages`] if beneficiary percentages do not sum to 100.
    /// - [`WillError::ZeroAmount`] if `amount` is not positive.
    /// - [`WillError::TooManyBeneficiaries`] if the beneficiary list is empty or too large,
    ///   or if too many guardians are supplied.
    /// - [`WillError::InvalidPercentages`] if any share is outside `1..=100`, or if
    ///   the shares do not sum to 100.
    /// - [`WillError::DuplicateGuardian`] if the same guardian is supplied twice.
    /// - [`WillError::InvalidPeriod`] if either period is zero or exceeds
    ///   [`MAX_PERIOD_DAYS`].
    /// - [`WillError::InvalidPercentages`] if beneficiary basis points do not sum to 10,000.
    #[allow(clippy::too_many_arguments)]
    pub fn create_will(
        env: Env,
        owner: Address,
        tokens: Vec<(Address, i128)>,
        beneficiaries: Vec<Beneficiary>,
        checkin_period_days: u64,
        grace_period_days: u64,
        guardians: Vec<Address>,
        is_native: bool,
    ) -> u64 {
        owner.require_auth();

        if tokens.is_empty() || tokens.len() > MAX_TOKENS {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }
        if beneficiaries.is_empty() || beneficiaries.len() > MAX_BENEFICIARIES {
            panic_with_error!(&env, WillError::TooManyBeneficiaries);
        }
        assert_valid_percentages(&env, &beneficiaries);
        assert_valid_guardians(&env, &guardians);
        assert_valid_periods(&env, checkin_period_days, grace_period_days);

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

        transfer_funds(&env, is_native, &token, &owner, &env.current_contract_address(), &amount);

        let beneficiaries_count = beneficiaries.len();
        let token_count = balances.len();
        for beneficiary in beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &beneficiary.address, will_id);
        }

        let will = Will {
            id: will_id,
            owner: owner.clone(),
            token,
            is_native,
            balance: amount,
            balances,
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
            token_count,
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

        // Clear the vote markers before zeroing the counter: the counter is
        // what tells `reset_guardian_votes` whether there is anything to clear.
        storage::reset_guardian_votes(&env, &will);

        will.status = WillStatus::Active;
        will.trigger_time = None;
        will.last_checkin = now;
        will.guardian_votes = 0;
        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        events::emergency_checkin(&env, will_id, &owner, next_deadline);
    }

    /// Distributes all token balances to beneficiaries proportionally to
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

        let refund = will.balance;
        transfer_funds(
            &env,
            will.is_native,
            &will.token,
            &env.current_contract_address(),
            &owner,
            &refund,
        );
        let contract_address = env.current_contract_address();
        let token_count = will.balances.len();

        // Refund every token balance back to the owner.
        for (token_addr, balance) in will.balances.iter() {
            if balance > 0 {
                token::Client::new(&env, &token_addr).transfer(
                    &contract_address,
                    &owner,
                    &balance,
                );
            }
        }

        will.balances = Map::new(&env);
        will.status = WillStatus::Cancelled;
        storage::save_will(&env, &will);

        events::will_cancelled(&env, will_id, &owner, token_count);
    }

    /// Replaces the beneficiary list for `will_id`. Only possible while the
    /// will is `Active`. The new basis points must sum to exactly 10,000.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::TooManyBeneficiaries`] if the new list is empty or too large.
    /// - [`WillError::InvalidPercentages`] if any new share is outside `1..=100`,
    ///   or if the new shares do not sum to 100.
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

    /// Replaces the guardian list for `will_id`. Only possible while the will
    /// is `Active`. Any votes cast against the previous guardian list are
    /// cleared so every updated list starts a fresh voting cycle.
    ///
    /// # Panics
    /// - [`WillError::NotOwner`] if `owner` does not own `will_id`.
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::TooManyBeneficiaries`] if more than `MAX_GUARDIANS`
    ///   guardians are supplied.
    /// - [`WillError::DuplicateGuardian`] if the same guardian is supplied twice.
    pub fn update_guardians(env: Env, will_id: u64, owner: Address, guardians: Vec<Address>) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        assert_valid_guardians(&env, &guardians);

        storage::reset_guardian_votes(&env, &will);
        will.guardians = guardians;
        will.guardian_votes = 0;
        storage::save_will(&env, &will);

        events::guardians_updated(&env, will_id, &owner);
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

        transfer_funds(
            &env,
            will.is_native,
            &will.token,
        token::Client::new(&env, &token).transfer(
            &owner,
            &env.current_contract_address(),
            &amount,
        );

        let prev = will.balances.get(token.clone()).unwrap_or(0);
        let new_balance = prev + amount;
        will.balances.set(token.clone(), new_balance);
        storage::save_will(&env, &will);

        events::top_up(&env, will_id, &owner, &token, amount, new_balance);
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
    /// [`GUARDIAN_THRESHOLD`] distinct guardians have voted, all balances are
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

        events::guardian_voted(&env, will_id, &guardian, will.guardian_votes);

        // `distribute` persists the will itself. Saving here as well would
        // write the whole entry twice — and extend its TTL twice — in the one
        // invocation that reaches quorum.
        if will.guardian_votes >= GUARDIAN_THRESHOLD {
            distribute(&env, &mut will);
        } else {
            storage::save_will(&env, &will);
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

/// Returns whether `beneficiaries` names `address`.
///
/// Operates on an in-memory list, so callers can decide reverse-index
/// membership without touching storage.
fn names_address(beneficiaries: &Vec<Beneficiary>, address: &Address) -> bool {
    beneficiaries
        .iter()
        .any(|beneficiary| &beneficiary.address == address)
}

/// Asserts beneficiary percentages sum to exactly 100.
/// Asserts every beneficiary share is in `1..=100` and that the shares sum to
/// exactly 100.
///
/// The per-share bound is not merely cosmetic: without it a caller could pass
/// shares near `u32::MAX` and overflow the running total, which panics under
/// `overflow-checks` instead of returning [`WillError::InvalidPercentages`].
/// With every share capped at 100 and the caller-side cap of
/// [`MAX_BENEFICIARIES`] entries, the total cannot exceed 1000.
///
/// A zero share is rejected too: such a beneficiary is recorded and indexed on
/// the will but would receive nothing on release.
fn assert_valid_percentages(env: &Env, beneficiaries: &Vec<Beneficiary>) {
    let mut total: u32 = 0;
    for beneficiary in beneficiaries.iter() {
        if !(1..=100).contains(&beneficiary.percentage) {
            panic_with_error!(env, WillError::InvalidPercentages);
        }
        total += beneficiary.percentage;
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
/// repeated address.
///
/// Duplicates matter because [`WillContract::guardian_trigger`] counts each
/// address at most once. A list such as `[g, g]` looks like a working 2-of-2
/// quorum but can only ever reach a single vote, silently leaving the will with
/// a guardian override that can never fire.
fn assert_valid_guardians(env: &Env, guardians: &Vec<Address>) {
    if guardians.len() > MAX_GUARDIANS {
        panic_with_error!(env, WillError::TooManyBeneficiaries);
    }
    for i in 0..guardians.len() {
        let guardian = guardians.get_unchecked(i);
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

/// Splits `will.balance` across `will.beneficiaries` proportionally to their
/// percentages, transfers the shares out of the contract, marks the will
/// For each token in `will.balances`, splits the balance across
/// `will.beneficiaries` proportionally to their percentages, transfers the
/// shares out of the contract, clears the balances map, marks the will
/// `Released`, and publishes the `InheritanceReleased` event. Any rounding
/// remainder from integer division is paid to the final beneficiary so the
/// full balance of every token is always distributed with no dust left behind.
/// Splits `will.balance` across `will.beneficiaries` proportionally to their
/// basis-point shares, transfers the shares out of the contract, marks the
/// will `Released`, and publishes the `InheritanceReleased` event. Any
/// rounding remainder from integer division is paid to the final beneficiary.
fn distribute(env: &Env, will: &mut Will) {
    let contract_address = env.current_contract_address();
    let count = will.beneficiaries.len();
    let token_count = will.balances.len();

    for (token_addr, total) in will.balances.iter() {
        if total == 0 {
            continue;
        }
        let token_client = token::Client::new(env, &token_addr);
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
            transfer_funds(
                env,
                will.is_native,
                &will.token,
                &contract_address,
                &beneficiary.address,
                &share,
            );
        }
    }

    will.balances = Map::new(env);
    will.status = WillStatus::Released;
    storage::save_will(env, will);

    events::inheritance_released(env, will.id, token_count, count);
}
