use soroban_sdk::{contracttype, Address, Bytes, Vec};

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

/// A privacy-preserving beneficiary entry (issue #46).
///
/// Instead of a raw address the owner stores a SHA-256 commitment hash of the
/// pre-image `<address_bytes> || <salt_bytes>`. At claim time the beneficiary
/// calls `reveal_and_claim` with the pre-image; the contract verifies the hash
/// matches and pays out to the revealed address.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HashedBeneficiary {
    /// SHA-256 hash of the pre-image (address bytes concatenated with salt).
    pub commitment: Bytes,
    /// Percentage of the will's balance this beneficiary receives.
    pub percentage: u32,
    /// Whether this hashed beneficiary has already been claimed.
    pub claimed: bool,
}

/// Lifecycle state of a will.
///
/// ```text
/// PendingConfirmation --(confirm_will / within delay)--> Active
///        |
///        |--(cancel_will during pending window)--> Cancelled
///
/// Active --(missed check-in)--> Triggered --(grace period expires)--> Released
///   |                               |
///   |--(cancel_will)--> Cancelled   |--(emergency_checkin)--> Active
/// ```
#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WillStatus {
    /// The will has just been created and is waiting for the owner to confirm
    /// it within the confirmation window (issue #43). No check-in clock runs
    /// while a will is in this state.
    PendingConfirmation,
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
    /// The primary owner address. Used for backwards-compatible single-owner
    /// flows and as the refund destination on cancellation.
    pub owner: Address,
    /// Additional co-owner addresses (issue #44). When this list is non-empty,
    /// owner-privileged actions require `owner_threshold` distinct
    /// authorisations out of `[owner] ++ co_owners`.
    pub co_owners: Vec<Address>,
    /// Number of signatures required from the combined owner+co_owners set for
    /// privileged operations. Defaults to 1 (single-owner mode). (issue #44)
    pub owner_threshold: u32,
    /// The token contract (e.g. a USDC Stellar Asset Contract) held by the will.
    pub token: Address,
    /// The amount of `token` currently locked in the will, in the token's base units.
    pub balance: i128,
    /// The beneficiaries and their percentage shares. Always sums to 100 when
    /// hashed_beneficiaries is empty, or together with hashed_beneficiaries'
    /// percentages they sum to 100.
    pub beneficiaries: Vec<Beneficiary>,
    /// Privacy-preserving beneficiaries registered by commitment hash (issue #46).
    /// Their percentages count towards the 100-sum together with `beneficiaries`.
    pub hashed_beneficiaries: Vec<HashedBeneficiary>,
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
    /// Unix timestamp (seconds) by which the owner must call `confirm_will`
    /// to move from `PendingConfirmation` to `Active` (issue #43).
    /// `None` once the will is confirmed or if no delay was requested.
    pub confirmation_deadline: Option<u64>,
    /// Current lifecycle state of the will.
    pub status: WillStatus,
    /// Optional guardian addresses (up to 3) who may force an early release
    /// via a 2-of-N vote using `guardian_trigger`.
    pub guardians: Vec<Address>,
    /// Number of distinct guardians who have voted to trigger the current
    /// guardian-release cycle.
    pub guardian_votes: u32,
}
