//! Resource-cost profile for every public entry point.
//!
//! This is a measurement harness, not a correctness test — [`test`] covers
//! behaviour. Its job is to make the ledger cost of each
//! entry point visible, so that a change which quietly doubles the storage
//! traffic of a hot path shows up in review instead of on a fee invoice.
//!
//! Run it with the output visible:
//!
//! ```text
//! cargo test -p will --lib profile -- --nocapture
//! ```
//!
//! # Reading the numbers
//!
//! Figures come from `Env::cost_estimate()`, which reports the resources the
//! host metered during the last top-level invocation, plus that invocation's
//! fee under a snapshot of Stellar pubnet rates.
//!
//! Two caveats apply, and both are why these numbers are useful for comparing
//! a change against its baseline rather than for predicting an exact bill:
//!
//! - The contract is registered natively, not as Wasm. Everything the host
//!   charges for reading, instantiating and running the Wasm module is
//!   therefore missing, which understates `instructions` in particular.
//!   Ledger-entry counts and byte sizes — the quantities this profile exists
//!   to police — are unaffected.
//! - Resource estimation is approximate by design; the SDK points at RPC
//!   simulation for exact submission resources.
//!
//! `read_entries` counts entries read but *not* modified, so an entry point's
//! total ledger footprint is `read_entries + write_entries`.

#![cfg(test)]

// The contract crate is `no_std`. Collecting rows and printing a table needs
// an allocator and stdout, which only this test-only module wants, so `std` is
// pulled in here rather than by relaxing the crate-level attribute.
extern crate std;

use std::{println, vec::Vec as StdVec};

use soroban_sdk::{
    testutils::{storage::Persistent as _, Address as _, Ledger},
    token::StellarAssetClient,
    vec, Address, Env, Vec,
};

use crate::{Beneficiary, WillContract, WillContractClient};

const DAY: u64 = 86_400;

/// Beneficiary count used for the profiled will.
///
/// Half of `MAX_BENEFICIARIES`, so the recorded entry size reflects a
/// realistic will rather than the cheapest or the most expensive one possible.
const PROFILE_BENEFICIARIES: u32 = 5;

/// One measured invocation.
struct Row {
    name: &'static str,
    instructions: i64,
    read_entries: u32,
    write_entries: u32,
    read_bytes: u32,
    write_bytes: u32,
    rent_ledger_bytes: i64,
    fee_stroops: i64,
}

/// Collects one [`Row`] per profiled entry point and renders them as a table.
struct Report {
    rows: StdVec<Row>,
}

impl Report {
    fn new() -> Self {
        Self {
            rows: StdVec::new(),
        }
    }

    /// Records the resources metered for the most recent invocation on `env`.
    ///
    /// Must be called immediately after the call being profiled: the host only
    /// retains the resources of the last top-level invocation, so any
    /// intervening contract call — including a token transfer or a `get_will`
    /// used to set up an assertion — would be measured instead.
    fn record(&mut self, env: &Env, name: &'static str) {
        let resources = env.cost_estimate().resources();
        let fee = env.cost_estimate().fee();

        self.rows.push(Row {
            name,
            instructions: resources.instructions,
            read_entries: resources.read_entries,
            write_entries: resources.write_entries,
            read_bytes: resources.read_bytes,
            write_bytes: resources.write_bytes,
            rent_ledger_bytes: resources.persistent_rent_ledger_bytes,
            fee_stroops: fee.total,
        });
    }

    /// Returns the row recorded under `name`, panicking if it is missing so a
    /// renamed scenario fails loudly instead of silently dropping its
    /// assertions.
    fn row(&self, name: &str) -> &Row {
        self.rows
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("no profiled scenario named `{name}`"))
    }

    /// Prints the profile as a Markdown table, ready to paste into a PR.
    fn print(&self) {
        println!(
            "\nSoroWill resource profile ({PROFILE_BENEFICIARIES} beneficiaries, 2 guardians)\n"
        );
        println!(
            "| entry point | instructions | read entries | write entries | read bytes | write bytes | rent ledger-bytes | fee (stroops) |"
        );
        println!("| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |");
        for row in &self.rows {
            println!(
                "| {} | {} | {} | {} | {} | {} | {} | {} |",
                row.name,
                row.instructions,
                row.read_entries,
                row.write_entries,
                row.read_bytes,
                row.write_bytes,
                row.rent_ledger_bytes,
                row.fee_stroops,
            );
        }
        println!();
    }
}

/// A profiling scenario: a fresh environment with a funded owner and a
/// registered contract.
struct Fixture<'a> {
    env: Env,
    client: WillContractClient<'a>,
    owner: Address,
    token: Address,
}

fn fixture<'a>() -> Fixture<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_700_000_000);

    let owner = Address::generate(&env);
    let sac = env.register_stellar_asset_contract_v2(owner.clone());
    let token = sac.address();
    StellarAssetClient::new(&env, &token).mint(&owner, &1_000_000_000_000);

    let contract_id = env.register(WillContract, ());
    let client = WillContractClient::new(&env, &contract_id);

    Fixture {
        env,
        client,
        owner,
        token,
    }
}

impl Fixture<'_> {
    /// Creates a will with `PROFILE_BENEFICIARIES` beneficiaries and the given
    /// guardians, returning its id and beneficiary list.
    fn create(&self, guardians: &Vec<Address>) -> (u64, Vec<Beneficiary>) {
        let list = beneficiaries(&self.env, PROFILE_BENEFICIARIES);
        let will_id = self.client.create_will(
            &self.owner,
            &self.token,
            &1_000_000,
            &list,
            &90,
            &7,
            guardians,
        );
        (will_id, list)
    }

    fn advance(&self, seconds: u64) {
        self.env.ledger().with_mut(|ledger| {
            ledger.timestamp += seconds;
        });
    }

    /// Advances the ledger sequence without moving the clock, ageing every
    /// stored entry towards expiry.
    ///
    /// Rent is only charged when an entry's remaining lifetime has fallen
    /// below `LIFETIME_THRESHOLD`; until then `extend_ttl` is a no-op and the
    /// rent columns read zero. Ageing the sequence is what makes the rent a
    /// will actually pays over its lifetime visible.
    fn age(&self, ledgers: u32) {
        self.env.ledger().with_mut(|ledger| {
            ledger.sequence_number += ledgers;
        });
    }
}

/// Builds `count` beneficiaries with fresh addresses whose shares sum to 100,
/// any remainder going to the last entry.
fn beneficiaries(env: &Env, count: u32) -> Vec<Beneficiary> {
    let mut list = Vec::new(env);
    let even = 100 / count;
    let mut allocated = 0;
    for index in 0..count {
        let percentage = if index == count - 1 {
            100 - allocated
        } else {
            even
        };
        allocated += percentage;
        list.push_back(Beneficiary {
            address: Address::generate(env),
            percentage,
        });
    }
    list
}

/// Returns `beneficiaries` with the shares re-cut but every address kept, the
/// shape of a beneficiary update that only changes who gets how much.
fn reshare(env: &Env, beneficiaries: &Vec<Beneficiary>) -> Vec<Beneficiary> {
    let count = beneficiaries.len();
    let mut list = Vec::new(env);
    let even = 100 / count;
    let mut allocated = 0;
    for (index, beneficiary) in beneficiaries.iter().enumerate() {
        // Shift a point from the first share to the last so the list is
        // genuinely different from the one already stored.
        let percentage = if index as u32 == count - 1 {
            100 - allocated
        } else if index == 0 {
            even - 1
        } else {
            even
        };
        allocated += percentage;
        list.push_back(Beneficiary {
            address: beneficiary.address.clone(),
            percentage,
        });
    }
    list
}

fn two_guardians(env: &Env) -> Vec<Address> {
    vec![env, Address::generate(env), Address::generate(env)]
}

#[test]
fn profile_public_entry_points() {
    let mut report = Report::new();

    profile_lifecycle(&mut report);
    profile_updates(&mut report);
    profile_guardians(&mut report);
    profile_queries(&mut report);
    let (active_ttl, terminal_ttl) = profile_under_rent_pressure(&mut report);

    report.print();
    assert_footprints(&report);
    assert_rent_renewal(active_ttl, terminal_ttl);
}

/// `create_will`, `check_in`, `get_will`, `top_up`, and the
/// trigger/release path.
fn profile_lifecycle(report: &mut Report) {
    let f = fixture();
    let guardians = two_guardians(&f.env);
    let (will_id, _) = f.create(&guardians);
    report.record(&f.env, "create_will");

    f.client.check_in(&will_id, &f.owner);
    report.record(&f.env, "check_in");

    f.client.get_will(&will_id);
    report.record(&f.env, "get_will");

    f.client.top_up(&will_id, &f.owner, &500_000);
    report.record(&f.env, "top_up");

    f.advance(91 * DAY);
    f.client.trigger_will(&will_id);
    report.record(&f.env, "trigger_will");

    // No guardian has voted, so the vote-clearing loop has nothing to remove.
    f.client.emergency_checkin(&will_id, &f.owner);
    report.record(&f.env, "emergency_checkin (no votes cast)");

    f.advance(91 * DAY);
    f.client.trigger_will(&will_id);
    f.advance(8 * DAY);
    f.client.release_inheritance(&will_id);
    report.record(&f.env, "release_inheritance");

    // `cancel_will` needs a will that was never released, so use a second one.
    let cancel = fixture();
    let (cancel_id, _) = cancel.create(&Vec::new(&cancel.env));
    cancel.client.cancel_will(&cancel_id, &cancel.owner);
    report.record(&cancel.env, "cancel_will");
}

/// The two shapes of `update_beneficiaries`, plus `update_guardians`.
fn profile_updates(report: &mut Report) {
    let f = fixture();
    let guardians = two_guardians(&f.env);
    let (will_id, original) = f.create(&guardians);

    // The common case: the same people, different shares. No address enters or
    // leaves the will, so no reverse index needs rewriting.
    f.client
        .update_beneficiaries(&will_id, &f.owner, &reshare(&f.env, &original));
    report.record(&f.env, "update_beneficiaries (shares only)");

    // The worst case: every beneficiary replaced, so every reverse index on
    // both the old and the new list has to be rewritten.
    let replacement = beneficiaries(&f.env, PROFILE_BENEFICIARIES);
    f.client
        .update_beneficiaries(&will_id, &f.owner, &replacement);
    report.record(&f.env, "update_beneficiaries (full replacement)");

    // No guardian has voted, so there are no vote markers to remove.
    f.client
        .update_guardians(&will_id, &f.owner, &two_guardians(&f.env));
    report.record(&f.env, "update_guardians (no votes cast)");
}

/// `guardian_trigger`, both below and at the release threshold.
fn profile_guardians(report: &mut Report) {
    let f = fixture();
    let guardians = two_guardians(&f.env);
    let (will_id, _) = f.create(&guardians);

    let first = guardians.get_unchecked(0);
    let second = guardians.get_unchecked(1);

    f.client.guardian_trigger(&will_id, &first);
    report.record(&f.env, "guardian_trigger (below threshold)");

    // The second vote reaches quorum and releases in the same invocation.
    f.client.guardian_trigger(&will_id, &second);
    report.record(&f.env, "guardian_trigger (reaches threshold)");

    // Clearing vote markers on a will that has votes to clear.
    let g = fixture();
    let g_guardians = two_guardians(&g.env);
    let (g_will_id, _) = g.create(&g_guardians);
    g.client
        .guardian_trigger(&g_will_id, &g_guardians.get_unchecked(0));
    g.client
        .update_guardians(&g_will_id, &g.owner, &two_guardians(&g.env));
    report.record(&g.env, "update_guardians (clearing a vote)");
}

/// The index-backed read-only queries.
fn profile_queries(report: &mut Report) {
    let f = fixture();
    let (_, list) = f.create(&Vec::new(&f.env));

    f.client.get_wills_by_owner(&f.owner);
    report.record(&f.env, "get_wills_by_owner (1 will)");

    f.client
        .get_wills_by_beneficiary(&list.get_unchecked(0).address);
    report.record(&f.env, "get_wills_by_beneficiary (1 will)");
}

/// The same calls once the will's entry has aged far enough to need its rent
/// topped up.
///
/// Every other scenario runs against entries written moments earlier, whose
/// remaining lifetime still exceeds the extension threshold — so `extend_ttl`
/// does nothing and rent never shows up. These rows show what a will costs on
/// the roughly monthly invocation that does have to pay.
/// Returns `(ttl after an aged check-in, ttl after an aged cancellation)`.
fn profile_under_rent_pressure(report: &mut Report) -> (u32, u32) {
    // An active will tops its own rent up when the threshold is crossed.
    let active = fixture();
    let (active_id, _) = active.create(&Vec::new(&active.env));
    age_until_rent_due(&active);
    active.client.check_in(&active_id, &active.owner);
    report.record(&active.env, "check_in (rent due)");
    let active_ttl = will_ttl(&active, active_id);

    // A terminal one does not. This needs its own fixture: had the will been
    // checked in first, its lifetime would already be back above the
    // threshold and the cancellation would look free either way.
    let terminal = fixture();
    let (terminal_id, _) = terminal.create(&Vec::new(&terminal.env));
    age_until_rent_due(&terminal);
    terminal.client.cancel_will(&terminal_id, &terminal.owner);
    report.record(&terminal.env, "cancel_will (rent due)");
    let terminal_ttl = will_ttl(&terminal, terminal_id);

    (active_ttl, terminal_ttl)
}

/// Ages the ledger until the will's entry is close enough to expiry that the
/// next `extend_ttl` will actually charge rent.
///
/// The will renews itself for 60 days, so the jump has to clear the 30-day
/// extension threshold. The Stellar Asset Contract only renews balances for 30
/// days, so a single jump that long archives the token balances first and
/// every transfer afterwards fails. The jump is therefore taken in two halves
/// with the balances refreshed in between — a mint touches only the token's
/// entries, leaving the will's own lifetime ticking down throughout.
fn age_until_rent_due(f: &Fixture<'_>) {
    const HALF: u32 = 400_000;

    keep_contracts_alive(f);
    f.age(HALF);
    refresh_token_balances(f);
    f.age(HALF);
}

/// Renews the token balances of the owner and of the contract holding the
/// locked funds, by minting a token unit to each.
fn refresh_token_balances(f: &Fixture<'_>) {
    let minter = StellarAssetClient::new(&f.env, &f.token);
    minter.mint(&f.owner, &1);
    minter.mint(&f.client.address, &1);
}

/// Renews the will and token contracts' instance entries so they survive a
/// long ledger jump.
///
/// A contract instance is written once at deployment with a short default
/// lifetime, and neither contract renews its own. Ageing the ledger far enough
/// to put a *will* under rent pressure would otherwise archive the instances
/// first and fail every later call — an artefact of the jump, not of anything
/// the scenario is measuring.
fn keep_contracts_alive(f: &Fixture<'_>) {
    // Longer than the whole jump, so one renewal up front is enough.
    const INSTANCE_TTL: u32 = 2_000_000;

    for contract in [&f.client.address, &f.token] {
        f.env.as_contract(contract, || {
            f.env
                .storage()
                .instance()
                .extend_ttl(INSTANCE_TTL, INSTANCE_TTL);
        });
    }
}

/// Reads the remaining lifetime, in ledgers, of a will's storage entry.
///
/// The rent columns cannot answer whether a *will* was renewed, because token
/// transfers put their own entries in the same total. Reading the entry's TTL
/// back addresses the question directly.
fn will_ttl(f: &Fixture<'_>, will_id: u64) -> u32 {
    f.env.as_contract(&f.client.address, || {
        f.env
            .storage()
            .persistent()
            .get_ttl(&crate::storage::DataKey::Will(will_id))
    })
}

/// Locks in the ledger footprint of each entry point.
///
/// Entry counts are asserted rather than instruction counts: they are the
/// quantity this pass set out to control, and unlike CPU figures they do not
/// drift with SDK patch releases, so a failure here means the contract's
/// storage behaviour actually changed.
fn assert_footprints(report: &Report) {
    // `check_in` is the baseline for "touched the will and nothing else": one
    // will entry written, plus the contract's own instance entry. Several
    // assertions below are stated against it rather than against a literal, so
    // they keep describing the same property if the SDK's accounting shifts.
    let will_only = report.row("check_in").write_entries;

    assert_eq!(
        report.row("get_will").write_entries,
        0,
        "get_will must not write to the ledger"
    );

    // Re-cutting shares between the same heirs moves nobody in or out of the
    // will, so no reverse index needs rewriting and the cost matches a plain
    // check-in. Without the membership test in `update_beneficiaries` this is
    // one extra read and write per beneficiary.
    assert_eq!(
        report
            .row("update_beneficiaries (shares only)")
            .write_entries,
        will_only,
        "a shares-only beneficiary update must write no more than a check-in"
    );
    assert!(
        report
            .row("update_beneficiaries (shares only)")
            .write_entries
            < report
                .row("update_beneficiaries (full replacement)")
                .write_entries,
        "a shares-only update must write fewer entries than a full replacement"
    );

    // Clearing guardian votes is skipped when the counter says there are none,
    // so both of these cost a check-in rather than a check-in plus one removal
    // per guardian.
    assert_eq!(
        report
            .row("emergency_checkin (no votes cast)")
            .write_entries,
        will_only,
        "an emergency check-in with no votes cast must not touch vote markers"
    );
    assert_eq!(
        report.row("update_guardians (no votes cast)").write_entries,
        will_only,
        "a guardian update with no votes cast must not touch vote markers"
    );
    assert!(
        report
            .row("update_guardians (clearing a vote)")
            .write_entries
            > report.row("update_guardians (no votes cast)").write_entries,
        "clearing a real vote must cost more than skipping the clear"
    );

    // Sanity check on the rent-pressure scenario itself: if ageing the ledger
    // stopped costing rent, the rows below it would prove nothing.
    assert!(
        report.row("check_in (rent due)").rent_ledger_bytes > 0,
        "the rent-pressure scenario must actually leave the will owing rent"
    );
}

/// Checks that rent is renewed for a will that can still be used, and not for
/// one that cannot.
fn assert_rent_renewal(active_ttl: u32, terminal_ttl: u32) {
    assert!(
        terminal_ttl < active_ttl,
        "a cancelled will had its lifetime extended ({terminal_ttl} ledgers) as if it were \
         still active ({active_ttl} ledgers); terminal wills should stop paying rent"
    );
}
