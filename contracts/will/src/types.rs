use soroban_sdk::{contracttype, Address, Symbol, Vec};
use soroban_sdk::{contracttype, Address, Map, Vec};

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

/// A guardian entry: an address paired with a vote weight.
///
/// Guardians with higher weights count for more when reaching quorum.
/// If all guardians have weight 1, the threshold is a simple majority count.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Guardian {
    pub address: Address,
    pub weight: u32,
}

/// Lifecycle state of a will.
///
/// ```text
/// Active --(missed check-in)--> Triggered --(grace period expires)--> Released --(close_will)--> Settled
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
    /// A Released will that has been explicitly closed/archived by the owner.
    Settled,
}

/// Aggregate protocol statistics that can be queried directly on-chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenLockedBalance {
    /// The token contract address.
    pub token: Address,
    /// The total amount of this token that is currently locked in active wills.
    pub total_locked: i128,
}

/// Aggregate protocol statistics that can be queried directly on-chain.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProtocolStats {
    /// The number of wills that are still in a non-terminal state.
    pub active_will_count: u64,
    /// Locked balances by token for all currently active wills.
    pub total_locked_by_token: Vec<TokenLockedBalance>,
}

/// The full on-chain state of a single will.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Will {
    /// Unique, monotonically increasing identifier for this will.
    pub id: u64,
    /// The address that created and funds the will.
    pub owner: Address,
    /// The token contract address (e.g. a USDC Stellar Asset Contract, or the
    /// native XLM asset address when `is_native` is true) held by the will.
    /// Map of token contract address → amount currently locked in the will,
    /// in each token's base units. A will may hold any number of distinct
    /// SEP-41 compliant tokens simultaneously.
    pub balances: Map<Address, i128>,
    /// The beneficiaries and their percentage shares. Always sums to 100.
    /// The token contract (e.g. a USDC Stellar Asset Contract) held by the will.
    pub token: Address,
    /// Whether the held asset is native XLM (as opposed to a token contract).
    /// When `true`, transfers use `env.transfer()` instead of the token client.
    pub is_native: bool,
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
    /// Optional guardians (up to 3) who may force an early release
    /// via a weight-based quorum using `guardian_trigger`.
    pub guardians: Vec<Guardian>,
    /// Accumulated weight of guardian votes cast in the current cycle.
    /// Release triggers when this reaches `GUARDIAN_THRESHOLD`.
    pub guardian_vote_weight: u32,
    /// Optional guardian addresses (up to 3) who may force an early release
    /// via a 2-of-N vote using `guardian_trigger`.
    pub guardians: Vec<Address>,
    /// Number of distinct guardians who have voted to trigger the current
    /// guardian-release cycle.
    pub guardian_votes: u32,
    /// Unix timestamp (seconds) of the last guardian-list change.
    /// `guardian_trigger` is only effective after a cooldown period has
    /// elapsed since this timestamp, preventing a compromised owner from
    /// swapping guardians right before a malicious action.
    pub guardian_list_updated_at: u64,
    /// Schema version for this will. Used to track which contract version
    /// wrote this state and enable forward/backward compatible migrations.
    pub schema_version: u32,
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
