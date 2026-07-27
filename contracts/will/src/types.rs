use soroban_sdk::{contracttype, Address, Vec};

/// A single beneficiary entry: an address and the share of the will's balance
/// it is entitled to receive when the inheritance is released, expressed in
/// basis points (1 bp = 0.01 %).
///
/// `basis_points` across all beneficiaries of a will must sum to exactly
/// 10,000 (i.e. 100 %).
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Beneficiary {
    pub address: Address,
    pub basis_points: u32,
}

/// A single grace-period tier that defines a partial payout milestone.
///
/// `day_offset` is the number of seconds after `trigger_time` at which this
/// tier's payout becomes claimable. `basis_points` is the percentage of the
/// original locked balance (at trigger time) to release at this tier.
///
/// Tiers must be sorted by ascending `day_offset` and their `basis_points`
/// must sum to exactly 10,000.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GraceTier {
    /// Seconds after `trigger_time` when this tier becomes claimable.
    pub day_offset: u64,
    /// Percentage of the original locked balance to release, in basis points.
    pub basis_points: u32,
}

/// Reason code attached to a guardian vote for transparency and dispute review.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardianVoteReason {
    /// The owner is known to be incapacitated.
    Incapacitated = 0,
    /// The owner is unreachable.
    Unreachable = 1,
    /// The owner is known to be deceased.
    Deceased = 2,
    /// Other reason (free-text context stored off-chain).
    Other = 3,
}

/// Lifecycle state of a will.
///
/// ```text
/// Active --(missed check-in)--> Triggered --(grace period expires)--> Released
///   |                               |
///   |--(cancel_will)--> Cancelled   |--(emergency_checkin)--> Active
/// ```
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WillStatus {
    /// The will is funded and the owner is checking in on schedule.
    Active,
    /// The owner missed a check-in deadline; the grace period is running.
    Triggered,
    /// The grace period expired (or guardians reached quorum) and funds were
    /// distributed to beneficiaries.
    Released,
    /// The owner cancelled the will and withdrew the remaining balance.
    Cancelled,
}

/// The full on-chain state of a single will.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Will {
    /// Unique, monotonically increasing identifier for this will.
    pub id: u64,
    /// The address that created and funds the will.
    pub owner: Address,
    /// The token contract (e.g. a USDC Stellar Asset Contract) held by the will.
    pub token: Address,
    /// The amount of `token` currently locked in the will, in the token's base units.
    pub balance: i128,
    /// The beneficiaries and their basis-point shares. Always sums to 10,000.
    pub beneficiaries: Vec<Beneficiary>,
    /// How many days the owner may go without checking in before the will
    /// can be triggered.
    pub checkin_period_days: u64,
    /// How many days after being triggered the owner has to prove they are
    /// alive (via `emergency_checkin`) before inheritance can be released.
    pub grace_period_days: u64,
    /// Unix timestamp (seconds) of the owner's last check-in.
    pub last_checkin: u64,
    /// Unix timestamp (seconds) at which the will was triggered, if any.
    pub trigger_time: Option<u64>,
    /// Current lifecycle state of the will.
    pub status: WillStatus,
    /// Optional guardian addresses (up to 3) who may force an early release
    /// via a 2-of-N vote using `guardian_trigger`.
    pub guardians: Vec<Address>,
    /// Number of distinct guardians who have voted to trigger the current
    /// guardian-release cycle.
    pub guardian_votes: u32,
    /// Number of days after a guardian vote is cast before it expires and
    /// no longer counts toward quorum. Defaults to `grace_period_days` if 0.
    pub guardian_vote_expiry_days: u64,
    /// Multi-tier grace period configuration. When empty, the will falls back
    /// to the legacy single-release behaviour (full payout at `grace_period_days`).
    /// When populated, tiers define partial payout milestones during the grace period.
    pub grace_tiers: Vec<GraceTier>,
    /// Cumulative basis points already released via grace-tier payouts.
    /// Starts at 0 and increases as tiers are released. When it reaches
    /// 10,000 the will transitions to `Released`.
    pub released_basis_points: u32,
    /// Balance locked at the moment the will was triggered, used to compute
    /// per-tier payouts without being affected by prior tier releases.
    /// Only meaningful while `status == Triggered`.
    pub trigger_balance: i128,
}
