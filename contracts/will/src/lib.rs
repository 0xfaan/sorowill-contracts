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

use soroban_sdk::{contract, contractimpl, panic_with_error, symbol_short, token, Address, Env, Vec};

pub use errors::WillError;
pub use types::{Beneficiary, Will, WillStatus, WillStatusTransition};
use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Map, Vec};

pub use errors::WillError;
pub use types::{Beneficiary, Guardian, Will, WillStatus};
pub use types::{Beneficiary, ProtocolStats, Will, WillStatus};

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
    /// - `beneficiaries`: 1 to `MAX_BENEFICIARIES` entries whose basis points sum to exactly 10,000.
    /// - `checkin_period_days`: how many days the owner may go without checking in.
    /// - `grace_period_days`: how many days after being triggered the owner has to prove they are alive.
    /// - `guardians`: 0 to `MAX_GUARDIANS` guardians that may jointly force an early release.
    ///   Each guardian has a vote weight; the accumulated weight must reach
    ///   `GUARDIAN_THRESHOLD` to trigger.
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
    /// - [`WillError::InvalidPercentages`] if beneficiary basis points do not sum to 10,000.
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
        guardians: Vec<Guardian>,
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

        transfer_funds(&env, is_native, &token, &owner, &env.current_contract_address(), &amount);

        let beneficiaries_count = beneficiaries.len();
        let token_count = balances.len();
        for beneficiary in beneficiaries.iter() {
            storage::index_by_beneficiary(&env, &beneficiary.address, will_id);
        }

        for guardian in guardians.iter() {
            storage::set_guardian_consent(&env, will_id, &guardian, &GuardianConsent::Pending);
        }

        if let Some(ref fb) = fallback_beneficiary {
            storage::set_fallback_beneficiary(&env, will_id, fb);
        }

        let will = Will {
            id: will_id,
            owner: owner.clone(),
            token: token.clone(),
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
            guardian_vote_weight: 0,
            guardian_votes: 0,
            guardian_list_updated_at: now,
            schema_version: CURRENT_SCHEMA_VERSION,
        };
        storage::save_will(&env, &will);
        storage::index_by_owner(&env, &owner, will_id);
        storage::increment_active_will_count(&env);
        storage::adjust_locked_value(&env, &token, amount);

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
        storage::reset_guardian_votes(&env, will_id, &will.guardians);
        will.guardian_votes = 0;
        let next_deadline = now + will.checkin_period_days * SECONDS_PER_DAY;
        storage::save_will(&env, &will);

        record_transition(
            &env,
            will_id,
            WillStatus::Triggered,
            WillStatus::Active,
            &owner,
            symbol_short!("emerg"),
        );

        events::emergency_checkin(&env, will_id, &owner, next_deadline);
    }

    /// Distributes all token balances to beneficiaries proportionally to
    /// their configured percentages. Callable by anyone once the grace
    /// period has fully elapsed. Any rounding remainder from integer
    /// division is paid to the final beneficiary so the full balance is
    /// always distributed with no dust left behind.
    ///
    /// In push mode (the default), tokens are transferred directly to each
    /// beneficiary. In pull mode (`pull_distribution = true`), shares are
    /// stored in claimable-shares storage and beneficiaries must call
    /// `claim_share` to withdraw.
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

        record_transition(
            &env,
            will_id,
            WillStatus::Triggered,
            WillStatus::Released,
            &env.current_contract_address(),
            symbol_short!("release"),
        );

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

        storage::decrement_active_will_count(&env);
        storage::adjust_locked_value(&env, &will.token, -refund);

        will.balance = 0;
        will.balances = Map::new(&env);
        will.status = WillStatus::Cancelled;
        storage::save_will(&env, &will);

        record_transition(
            &env,
            will_id,
            WillStatus::Active,
            WillStatus::Cancelled,
            &owner,
            symbol_short!("cancel"),
        );

        events::will_cancelled(&env, will_id, &owner, refund);
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
    pub fn update_guardians(env: Env, will_id: u64, owner: Address, guardians: Vec<Guardian>) {
    /// - [`WillError::DuplicateGuardian`] if the same guardian is supplied twice.
    pub fn update_guardians(env: Env, will_id: u64, owner: Address, guardians: Vec<Address>) {
        owner.require_auth();
        let mut will = load_owned(&env, will_id, &owner);
        assert_status(&env, &will, WillStatus::Active, WillError::WillNotActive);

        assert_valid_guardians(&env, &guardians);

        let now = env.ledger().timestamp();
        storage::reset_guardian_votes(&env, &will);
        will.guardians = guardians;
        will.guardian_votes = 0;
        will.guardian_list_updated_at = now;
        will.guardian_vote_weight = 0;
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

        will.balance += amount;
        storage::adjust_locked_value(&env, &will.token, amount);
        let prev = will.balances.get(token.clone()).unwrap_or(0);
        let new_balance = prev + amount;
        will.balances.set(token.clone(), new_balance);
        storage::save_will(&env, &will);

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

    /// Returns aggregate protocol statistics for all wills currently tracked on-chain.
    pub fn get_protocol_stats(env: Env) -> ProtocolStats {
        storage::get_protocol_stats(&env)
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
    pub fn get_wills_by_beneficiary(
        env: Env,
        beneficiary: Address,
        cursor: Option<u64>,
        limit: u32,
    ) -> Vec<Will> {
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
        let page = storage::paginate_ids(&env, &ids, cursor, limit);
        let mut wills = Vec::new(&env);
        for id in page.iter() {
            if let Ok(will) = storage::load_will(&env, id) {
                wills.push_back(will);
            }
        }
        wills
    }

    /// Casts a guardian vote to force an early release of `will_id`, for use
    /// when the owner is known to be incapacitated. Each guardian's configured
    /// weight is added to the accumulated vote weight; once it reaches
    /// [`GUARDIAN_THRESHOLD`], the balance is immediately distributed to
    /// beneficiaries, bypassing the check-in and grace-period flow entirely.
    /// when the owner is known to be incapacitated. Once
    /// [`GUARDIAN_THRESHOLD`] distinct guardians have voted, all balances are
    /// immediately distributed to beneficiaries, bypassing the check-in and
    /// grace-period flow entirely.
    ///
    /// Enforces a cooldown after a guardian-list change: if the current
    /// guardian list was updated less than [`GUARDIAN_COOLDOWN_DAYS`] days ago,
    /// the vote is rejected with [`WillError::GuardianCooldownActive`].
    ///
    /// # Panics
    /// - [`WillError::WillNotActive`] if the will is not `Active`.
    /// - [`WillError::NotGuardian`] if `guardian` is not one of the will's guardians.
    /// - [`WillError::AlreadyVoted`] if `guardian` already voted in this cycle.
    /// - [`WillError::GuardianCooldownActive`] if the guardian-list cooldown has not elapsed.
    pub fn guardian_trigger(env: Env, will_id: u64, guardian: Address) {
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

        if !will.guardians.contains(&guardian) {
            panic_with_error!(&env, WillError::NotGuardian);
        }
        let weight = match will.guardians.iter().find(|g| g.address == guardian) {
            Some(g) => g.weight,
            None => panic_with_error!(&env, WillError::NotGuardian),
        };
        if storage::has_guardian_voted(&env, will_id, &guardian) {
            panic_with_error!(&env, WillError::AlreadyVoted);
        }

        storage::set_guardian_voted(&env, will_id, &guardian);
        will.guardian_vote_weight += weight;
        storage::save_will(&env, &will);
        will.guardian_votes += 1;

        events::guardian_voted(&env, will_id, &guardian, weight, will.guardian_vote_weight);

        if will.guardian_vote_weight >= GUARDIAN_THRESHOLD {
        // `distribute` persists the will itself. Saving here as well would
        // write the whole entry twice — and extend its TTL twice — in the one
        // invocation that reaches quorum.
        if will.guardian_votes >= GUARDIAN_THRESHOLD {
            record_transition(
                &env,
                will_id,
                WillStatus::Active,
                WillStatus::Released,
                &guardian,
                symbol_short!("gtrigr"),
            );
            distribute(&env, &mut will);
        } else {
            storage::save_will(&env, &will);
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
            guardian_votes: 0,
            guardian_list_updated_at: now,
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
            assert_valid_guardians(&env, &guardians);
            assert_valid_periods(&env, checkin_period_days, grace_period_days);

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
                guardians,
                guardian_votes: 0,
                guardian_list_updated_at: now,
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

/// Asserts beneficiary basis points sum to exactly 10,000.
///
/// The per-entry upper bound is not enforced here (shares are `u32` and
/// `MAX_BENEFICIARIES` is small enough that overflow is not a concern), but
/// the exact-sum invariant is critical: it guarantees that every token
/// balance is fully distributed with no dust left behind.
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
/// `Released`, and publishes the `InheritanceReleased` event with a full
/// per-beneficiary breakdown. Any rounding remainder from integer division
/// is paid to the final beneficiary.
/// For each token in `will.balances`, splits the balance across
/// `will.beneficiaries` proportionally to their basis-point shares, transfers
/// the shares out of the contract, clears the balances map, marks the will
/// `Released`, and publishes the `InheritanceReleased` event. Any rounding
/// remainder from integer division is paid to the final beneficiary so the
/// full balance of every token is always distributed with no dust left behind.
fn distribute(env: &Env, will: &mut Will) {
    let contract_address = env.current_contract_address();
    let count = will.beneficiaries.len();
    let guardian_triggered = will.status == WillStatus::Active;
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
                let portion = total * (beneficiary.basis_points as i128) / 10_000;
                remaining -= portion;
                portion
            };
            if share > 0 {
                token_client.transfer(&contract_address, &beneficiary.address, &share);
            }
    let mut remaining = total;
    let mut breakdown: Vec<(Address, u32, i128)> = Vec::new(env);
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
        breakdown.push_back((beneficiary.address.clone(), beneficiary.percentage, share));
    }

    storage::decrement_active_will_count(env);
    storage::adjust_locked_value(env, &will.token, -total);

    will.balance = 0;
    will.balances = Map::new(env);
    will.status = WillStatus::Released;
    storage::save_will(env, will);

    events::inheritance_released(env, will.id, total, &breakdown, guardian_triggered);
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
