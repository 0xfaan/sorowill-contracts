use soroban_sdk::{
    contract, contractimpl, contracttype, token, Address, Env, String, Vec,
};

const MAX_KEEPER_BOUNTY_BPS: u32 = 100;

#[derive(Clone)]
#[contracttype]
pub struct Will {
    pub owner: Address,
    pub beneficiaries: Vec<Beneficiary>,
    pub inactivity_period: u64,
    pub last_check_in: u64,
    pub triggered: bool,
    pub release_time: u64,
    pub keeper_bounty_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub struct Beneficiary {
    pub address: Address,
    pub share_bps: u32,
}

#[derive(Clone)]
#[contracttype]
pub enum DataKey {
    Will,
}

#[derive(Clone)]
#[contracttype]
pub struct WillTriggeredEvent {
    pub owner: Address,
    pub triggered_by: Address,
    pub release_time: u64,
    pub keeper_bounty_paid: i128,
}

#[derive(Clone)]
#[contracttype]
pub struct InheritanceReleasedEvent {
    pub owner: Address,
    pub released_by: Address,
    pub total_amount: i128,
    pub keeper_bounty_paid: i128,
}

#[contract]
pub struct WillContract;

#[contractimpl]
impl WillContract {
    pub fn create_will(
        env: Env,
        owner: Address,
        beneficiaries: Vec<Beneficiary>,
        inactivity_period: u64,
        keeper_bounty_bps: u32,
    ) {
        owner.require_auth();

        if keeper_bounty_bps > MAX_KEEPER_BOUNTY_BPS {
            panic!("keeper_bounty_bps exceeds maximum of {}", MAX_KEEPER_BOUNTY_BPS);
        }

        let mut total_share: u32 = 0;
        for beneficiary in beneficiaries.iter() {
            total_share += beneficiary.share_bps;
        }
        if total_share != 10000 {
            panic!("beneficiary shares must sum to 10000 bps (100%)");
        }

        let will = Will {
            owner: owner.clone(),
            beneficiaries,
            inactivity_period,
            last_check_in: env.ledger().timestamp(),
            triggered: false,
            release_time: 0,
            keeper_bounty_bps,
        };

        env.storage().instance().set(&DataKey::Will, &will);
    }

    pub fn check_in(env: Env) {
        let mut will: Will = env.storage().instance().get(&DataKey::Will).unwrap();
        will.owner.require_auth();

        if will.triggered {
            panic!("will already triggered");
        }

        will.last_check_in = env.ledger().timestamp();
        env.storage().instance().set(&DataKey::Will, &will);
    }

    pub fn trigger_will(env: Env, token_address: Address) {
        let mut will: Will = env.storage().instance().get(&DataKey::Will).unwrap();

        if will.triggered {
            panic!("will already triggered");
        }

        let current_time = env.ledger().timestamp();
        if current_time < will.last_check_in + will.inactivity_period {
            panic!("inactivity period not yet elapsed");
        }

        will.triggered = true;
        will.release_time = current_time + 604800;
        env.storage().instance().set(&DataKey::Will, &will);

        let invoker = env.invoker();
        let mut keeper_bounty_paid: i128 = 0;

        if will.keeper_bounty_bps > 0 && invoker != will.owner {
            let token_client = token::Client::new(&env, &token_address);
            let balance = token_client.balance(&env.current_contract_address());
            keeper_bounty_paid = (balance * will.keeper_bounty_bps as i128) / 10000;
            if keeper_bounty_paid > 0 {
                token_client.transfer(&env.current_contract_address(), &invoker, &keeper_bounty_paid);
            }
        }

        env.events().publish(
            (String::from_str(&env, "will_triggered"),),
            WillTriggeredEvent {
                owner: will.owner.clone(),
                triggered_by: invoker,
                release_time: will.release_time,
                keeper_bounty_paid,
            },
        );
    }

    pub fn release_inheritance(env: Env, token_address: Address) {
        let will: Will = env.storage().instance().get(&DataKey::Will).unwrap();

        if !will.triggered {
            panic!("will not triggered");
        }

        let current_time = env.ledger().timestamp();
        if current_time < will.release_time {
            panic!("release time not yet reached");
        }

        let token_client = token::Client::new(&env, &token_address);
        let balance = token_client.balance(&env.current_contract_address());
        let invoker = env.invoker();
        let mut keeper_bounty_paid: i128 = 0;

        if will.keeper_bounty_bps > 0 && invoker != will.owner {
            keeper_bounty_paid = (balance * will.keeper_bounty_bps as i128) / 10000;
            if keeper_bounty_paid > 0 {
                token_client.transfer(&env.current_contract_address(), &invoker, &keeper_bounty_paid);
            }
        }

        let remaining_balance = balance - keeper_bounty_paid;

        for beneficiary in will.beneficiaries.iter() {
            let amount = (remaining_balance * beneficiary.share_bps as i128) / 10000;
            if amount > 0 {
                token_client.transfer(&env.current_contract_address(), &beneficiary.address, &amount);
            }
        }

        env.events().publish(
            (String::from_str(&env, "inheritance_released"),),
            InheritanceReleasedEvent {
                owner: will.owner.clone(),
                released_by: invoker,
                total_amount: balance,
                keeper_bounty_paid,
            },
        );

        env.storage().instance().remove(&DataKey::Will);
    }

    pub fn get_will(env: Env) -> Will {
        env.storage().instance().get(&DataKey::Will).unwrap()
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use soroban_sdk::{testutils::Address as _, token, Address, Env};

    fn create_token_contract<'a>(env: &Env, admin: &Address) -> (token::Client<'a>, token::StellarAssetClient<'a>) {
        let contract_address = env.register_stellar_asset_contract(admin.clone());
        (
            token::Client::new(env, &contract_address),
            token::StellarAssetClient::new(env, &contract_address),
        )
    }

    #[test]
    fn test_keeper_bounty_on_trigger() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let keeper = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let (token_client, token_admin) = create_token_contract(&env, &owner);
        token_admin.mint(&contract_id, &10000);

        let beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );

        client.create_will(&owner, &beneficiaries, &100, &50);

        env.ledger().with_mut(|li| li.timestamp = 200);

        env.mock_auths(&[]);
        env.set_auths(&[]);
        client.trigger_will(&token_client.address);

        let keeper_balance = token_client.balance(&keeper);
        assert_eq!(keeper_balance, 50);

        let contract_balance = token_client.balance(&contract_id);
        assert_eq!(contract_balance, 9950);
    }

    #[test]
    fn test_keeper_bounty_on_release() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let keeper = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let (token_client, token_admin) = create_token_contract(&env, &owner);
        token_admin.mint(&contract_id, &10000);

        let beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );

        client.create_will(&owner, &beneficiaries, &100, &100);

        env.ledger().with_mut(|li| li.timestamp = 200);
        client.trigger_will(&token_client.address);

        env.ledger().with_mut(|li| li.timestamp = 200 + 604800);

        env.mock_auths(&[]);
        env.set_auths(&[]);
        client.release_inheritance(&token_client.address);

        let keeper_balance = token_client.balance(&keeper);
        assert_eq!(keeper_balance, 100);

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
                guardian_cancel_vote_weight: 0,
                guardian_cancel_votes: 0,
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
                &will.beneficiaries,
                now + checkin_period_days * SECONDS_PER_DAY,
            );

            ids.push_back(will_id);
        }

        events::batch_created(&env, &owner, &ids);
        ids
    }

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

    #[test]
    fn test_zero_bounty_default() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let keeper = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let (token_client, token_admin) = create_token_contract(&env, &owner);
        token_admin.mint(&contract_id, &10000);

        let beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );

        client.create_will(&owner, &beneficiaries, &100, &0);

        env.ledger().with_mut(|li| li.timestamp = 200);
        client.trigger_will(&token_client.address);

        env.ledger().with_mut(|li| li.timestamp = 200 + 604800);
        client.release_inheritance(&token_client.address);

        let keeper_balance = token_client.balance(&keeper);
        assert_eq!(keeper_balance, 0);

        let beneficiary_balance = token_client.balance(&beneficiary);
        assert_eq!(beneficiary_balance, 10000);
    }

    #[test]
    #[should_panic(expected = "keeper_bounty_bps exceeds maximum of 100")]
    fn test_bounty_cap_enforced() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );

        client.create_will(&owner, &beneficiaries, &100, &101);
    }

    #[test]
    fn test_no_bounty_when_owner_triggers() {
        let env = Env::default();
        env.mock_all_auths();

        let owner = Address::generate(&env);
        let beneficiary = Address::generate(&env);
        let contract_id = env.register_contract(None, WillContract);
        let client = WillContractClient::new(&env, &contract_id);

        let (token_client, token_admin) = create_token_contract(&env, &owner);
        token_admin.mint(&contract_id, &10000);

        let beneficiaries = Vec::from_array(
            &env,
            [Beneficiary {
                address: beneficiary.clone(),
                share_bps: 10000,
            }],
        );

        client.create_will(&owner, &beneficiaries, &100, &50);

        env.ledger().with_mut(|li| li.timestamp = 200);
        client.trigger_will(&token_client.address);

        let owner_balance = token_client.balance(&owner);
        assert_eq!(owner_balance, 0);

        let contract_balance = token_client.balance(&contract_id);
        assert_eq!(contract_balance, 10000);
    }

    if fixed_total > will_balance {
        panic_with_error!(env, WillError::FixedAmountExceedsBalance);
    }
    if has_percentage {
        if percentage_total != 10_000 {
            panic_with_error!(env, WillError::InvalidPercentages);
        }
    } else if fixed_total != will_balance {
        panic_with_error!(env, WillError::FixedAmountExceedsBalance);
    }
}

/// Adds `value` into `total`, panicking with `InvalidPercentages` on overflow
/// instead of aborting — a `u32` overflow here would otherwise be reachable
/// with adversarial basis-point inputs.
fn total_checked_add(total: &mut u32, value: u32, env: &Env) {
    *total = match total.checked_add(value) {
        Some(sum) => sum,
        None => panic_with_error!(env, WillError::InvalidPercentages),
    };
}

/// Sums every token balance in `balances` into a single `i128`, saturating
/// rather than overflowing. Used to validate `Allocation::FixedAmount`
/// entries against "the will's balance" in the simplified single-balance
/// sense described in the `Allocation` docs: a fixed amount is available
/// against the combined value locked across all of a will's tokens.
fn total_balance(balances: &Map<Address, i128>) -> i128 {
    let mut total: i128 = 0;
    for (_, amount) in balances.iter() {
        total = total.saturating_add(amount);
    }
    total
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
/// Calculates `floor(total * basis_points / 10_000)` without ever forming
/// the potentially overflowing `total * basis_points` intermediate. The
/// workspace release profile enables overflow checks, but this decomposition
/// also makes the calculation safe independently of that compiler setting.
fn proportional_share(total: i128, basis_points: u32) -> i128 {
    const BASIS_POINTS_TOTAL: i128 = 10_000;

    let whole = total / BASIS_POINTS_TOTAL;
    let remainder = total % BASIS_POINTS_TOTAL;
    whole * basis_points as i128
        + remainder * basis_points as i128 / BASIS_POINTS_TOTAL
}

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
            bounty_amount = proportional_share(total, will.keeper_bounty_bps);
        }

        let mut shares: Vec<(Address, i128)> = Vec::new(env);

        // Fixed-amount beneficiaries are paid first, capped at what is
        // actually available so a misconfigured/under-funded token never
        // aborts the whole distribution.
        let mut remaining = total;
        for (index, beneficiary) in will.beneficiaries.iter().enumerate() {
            let share = if index as u32 == count - 1 {
                remaining
            } else {
                let portion = proportional_share(total, beneficiary.basis_points);
                remaining -= portion;
                portion
            };
            shares.push_back((beneficiary.address.clone(), share));
        }

        // Whatever remains is split among percentage-based beneficiaries,
        // proportionally to their basis points; the final one absorbs the
        // rounding remainder so no dust is left behind.
        let mut percentage_count: u32 = 0;
        for beneficiary in will.beneficiaries.iter() {
            if let Allocation::Percentage(_) = beneficiary.allocation {
                percentage_count += 1;
            }
        }
        let mut percentage_index: u32 = 0;
        let mut percentage_remaining = remaining;
        for beneficiary in will.beneficiaries.iter() {
            if let Allocation::Percentage(bp) = beneficiary.allocation {
                percentage_index += 1;
                let share = if percentage_index == percentage_count {
                    percentage_remaining
                } else {
                    let portion = remaining * (bp as i128) / 10_000;
                    percentage_remaining -= portion;
                    portion
                };
                shares.push_back((beneficiary.address.clone(), share));
            }
        }

        transfer_plan.push_back((token_addr, shares));
    }

    // --- EFFECTS: mutate and persist all state before any external call ---
    storage::decrement_active_will_count(env);

    will.balance = 0;
    will.balances = Map::new(env);
    will.status = WillStatus::Released;

    // Clean up any guardian-vote entries that were cast in the current cycle.
    // When a guardian quorum triggers distribute() directly (via
    // guardian_trigger), the GuardianVote persistent-storage entries recorded
    // for that cycle are never touched by emergency_checkin (which only runs
    // on Triggered wills). Without this call those entries become permanent,
    // unreachable dead storage that the contract keeps paying TTL-bump rent
    // on for the lifetime of the instance. Clearing them here mirrors what
    // emergency_checkin already does for the Active→Triggered→Active path.
    storage::reset_guardian_votes(env, will);
    // Zero the in-memory counter so the saved Released will has a consistent
    // state (no votes, no vote records).
    will.guardian_votes = 0;
    will.guardian_vote_weight = 0;

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

/// Releases `amount` from the will proportionally across percentage-based
/// beneficiaries and deducts it from `will.balance`. Does NOT change the
/// will's status or persist it — the caller is responsible for saving.
///
/// Fixed-amount beneficiaries are intentionally excluded from tiered partial
/// releases: a `FixedAmount` promise is only meaningful once, against the
/// final full release, so paying a fraction of it early at each grace-tier
/// milestone would either shortchange or double-pay them. They are always
/// paid in full by `distribute` at the final release instead.
fn distribute_tier(env: &Env, will: &mut Will, amount: i128) {
    let token_client = token::Client::new(env, &will.token);
    let contract_address = env.current_contract_address();

    let mut percentage_count: u32 = 0;
    for beneficiary in will.beneficiaries.iter() {
        if let Allocation::Percentage(_) = beneficiary.allocation {
            percentage_count += 1;
        }
    }

    let mut percentage_index: u32 = 0;
    let mut remaining = amount;
    for beneficiary in will.beneficiaries.iter() {
        if let Allocation::Percentage(bp) = beneficiary.allocation {
            percentage_index += 1;
            let share = if percentage_index == percentage_count {
                remaining
            } else {
                let portion = amount * (bp as i128) / 10_000;
                remaining -= portion;
                portion
            };
            if share > 0 {
                token_client.transfer(&contract_address, &beneficiary.address, &share);
            }
    for (index, beneficiary) in will.beneficiaries.iter().enumerate() {
        let share = if index as u32 == count - 1 {
            remaining
        } else {
            let portion = proportional_share(amount, beneficiary.basis_points);
            remaining -= portion;
            portion
        };
        if share > 0 {
            token_client.transfer(&contract_address, &beneficiary.address, &share);
        }
    }

    will.balance -= amount;
}
