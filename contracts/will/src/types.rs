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

/// Tier of a guardian, determining voting priority.
///
/// Primary guardians count immediately toward the release threshold.
/// Backup guardians may only vote if no primary guardians exist on the
/// will (i.e. all guardians have been replaced with backups, or the
/// original list contained only backups).
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardianTier {
    /// Counts immediately toward the guardian-release threshold.
    Primary,
    /// Counts only when no primary guardians are present.
    Backup,
}

/// A guardian entry pairing an address with its tier.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GuardianEntry {
    pub address: Address,
    pub tier: GuardianTier,
}

/// Optional vesting schedule attached to a will. When present, the
/// inheritance is not released in a single lump sum. Instead, funds
/// unlock linearly from `start_time` over `duration_seconds`. The
/// `released_amount` tracks how much has already been claimed.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VestingSchedule {
    /// Unix timestamp (seconds) at which vesting begins.
    /// For will-triggered vesting this is set to the grace-period expiry.
    pub start_time: u64,
    /// Total duration of the vesting window, in seconds.
    pub duration_seconds: u64,
    /// Amount of `token` already claimed and transferred out.
    pub released_amount: i128,
}

/// Lifecycle state of a will.
///
/// ```text
/// Active --(missed check-in)--> Triggered --(grace period expires)--> Released
///   |                               |
///   |--(cancel_will)--> Cancelled   |--(emergency_checkin)--> Active
///   |--(partial_release)--> Active  (balance reduced, subset paid)
/// ```
///
/// When a vesting schedule is configured, the lifecycle after grace-period
/// expiry is:
///
/// ```text
/// Triggered --(grace period expires + vesting configured)--> Vesting
/// Vesting --(claim_vested)--> Vesting  (partial unlock)
/// Vesting --(final claim)--> Released
/// ```
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WillStatus {
    /// The will is funded and the owner is checking in on schedule.
    Active,
    /// The owner missed a check-in deadline; the grace period is running.
    Triggered,
    /// The grace period expired (or guardians reached quorum) and funds were
    /// distributed to beneficiaries (lump sum or final vested claim).
    Released,
    /// The owner cancelled the will and withdrew the remaining balance.
    Cancelled,
    /// A vesting schedule is active; funds unlock gradually over time.
    Vesting,
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
    /// Guardians (up to 3) with tier distinction who may force an early
    /// release via a 2-of-N vote using `guardian_trigger`.
    pub guardians: Vec<GuardianEntry>,
    /// Number of distinct guardians who have voted to trigger the current
    /// guardian-release cycle.
    pub guardian_votes: u32,
    /// Optional delegate address that may check in on the owner's behalf.
    pub delegate: Option<Address>,
    /// Optional vesting schedule. When present, `release_inheritance` does
    /// not distribute everything at once; funds unlock linearly over the
    /// configured duration and beneficiaries claim via `claim_vested`.
    pub vesting: Option<VestingSchedule>,
}
