use soroban_sdk::contracterror;

/// Errors returned by the SoroWill contract.
///
/// Every error variant is surfaced to callers as a `#[contracterror]` so that
/// SDK and client code can match on a stable numeric code instead of parsing
/// panic messages.
#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum WillError {
    /// No will exists for the given identifier.
    WillNotFound = 1,
    /// The caller is not the owner of the will.
    NotOwner = 2,
    /// The requested action requires the will to be `Active`.
    WillNotActive = 3,
    /// The requested action requires the will to be `Triggered`.
    WillNotTriggered = 4,
    /// `release_inheritance` was called before the grace period elapsed.
    GracePeriodNotExpired = 5,
    /// `emergency_checkin` was called after the grace period already elapsed.
    GracePeriodExpired = 6,
    /// Beneficiary percentages did not sum to exactly 100.
    InvalidPercentages = 7,
    /// The guardian has already voted to trigger this will.
    AlreadyVoted = 8,
    /// The caller is not a designated guardian of this will.
    NotGuardian = 9,
    /// `trigger_will` was called before the check-in deadline passed.
    CheckinNotDue = 10,
    /// An amount of zero (or less) was supplied where a positive amount is required.
    ZeroAmount = 11,
    /// Too many beneficiaries (or guardians) were supplied.
    TooManyBeneficiaries = 12,
    /// The caller is not the owner nor the designated delegate.
    NotOwnerOrDelegate = 13,
    /// No delegate has been set on this will.
    DelegateNotSet = 14,
    /// The partial release amount must be positive.
    ZeroPartialRelease = 15,
    /// None of the supplied beneficiary addresses are named in this will.
    InvalidReleaseBeneficiaries = 16,
    /// The requested partial release exceeds the remaining balance.
    InsufficientBalance = 17,
    /// A backup guardian cannot vote while a primary guardian is available.
    BackupGuardianUnavailable = 18,
    /// Nothing has vested yet; the vesting duration has not elapsed.
    NothingVested = 19,
    /// The will has no vesting schedule configured.
    VestingNotConfigured = 20,
    /// The will is already fully released; no more claims are possible.
    FullyReleased = 21,
}
