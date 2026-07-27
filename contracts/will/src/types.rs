use soroban_sdk::{contracttype, Address, Symbol, Vec};

/// A single beneficiary entry: an address and the percentage of the will's
/// balance it is entitled to receive when the inheritance is released.
///
/// Percentages across all beneficiaries of a will must sum to exactly 100.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Beneficiary {
    pub address: Address,
    pub percentage: u32,
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
    /// The beneficiaries and their percentage shares. Always sums to 100.
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
}

/// A single entry in a will's on-chain audit trail, recording one status
/// transition. The full history of a will can be reconstructed by reading
/// all entries in insertion order.
#[contracttype]
#[derive(Clone, Debug)]
pub struct WillStatusTransition {
    /// The will this transition belongs to.
    pub will_id: u64,
    /// The status before the transition.
    pub from_status: WillStatus,
    /// The status after the transition.
    pub to_status: WillStatus,
    /// Unix timestamp (seconds) when the transition occurred.
    pub timestamp: u64,
    /// The address that initiated the transition, or the contract address
    /// for transitions triggered by anyone (e.g. `trigger_will`,
    /// `release_inheritance`).
    pub actor: Address,
    /// A short label describing what caused the transition
    /// (e.g. "create", "checkin", "trigger", "release").
    pub action: Symbol,
}
