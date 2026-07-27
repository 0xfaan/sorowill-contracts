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

/// Consent status for a named guardian.
///
/// A guardian must explicitly accept before they can cast a `guardian_trigger`
/// vote. The owner may also reject a guardian's acceptance.
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GuardianConsent {
    /// Guardian has been named but has not yet responded.
    Pending,
    /// Guardian has accepted the role and may vote.
    Accepted,
    /// Guardian has declined the role.
    Rejected,
}

/// A beneficiary's claimable share in a pull-based distribution.
///
/// Stored in persistent storage keyed by `(will_id, beneficiary_address)`.
/// When the will enters `Released` status with `pull_distribution = true`,
/// `distribute` computes each beneficiary's share and stores a `ClaimableShare`
/// with `total` set to the share amount and `claimed` set to `0`. When the
/// beneficiary calls `claim_share`, `claimed` is set to `total` and the tokens
/// are transferred out of the contract.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimableShare {
    /// The total amount the beneficiary is entitled to claim.
    pub total: i128,
    /// The amount already claimed (0 or `total`).
    pub claimed: i128,
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
    /// When `true`, `distribute` stores claimable shares instead of pushing
    /// tokens directly. Beneficiaries must call `claim_share` to withdraw.
    pub pull_distribution: bool,
    /// Optional fallback beneficiary that receives shares when a direct
    /// transfer fails. If `None`, failed transfers keep funds in the contract
    /// as claimable shares.
    pub fallback_beneficiary: Option<Address>,
}
