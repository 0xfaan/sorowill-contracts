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
    /// The will is still in `PendingConfirmation` and cannot be operated on
    /// until the owner calls `confirm_will` (issue #43).
    WillNotConfirmed = 13,
    /// `confirm_will` was called after the confirmation window has closed,
    /// or the will is not in `PendingConfirmation` state (issue #43).
    ConfirmationWindowExpired = 14,
    /// The split amount exceeds the will's current balance (issue #45).
    InsufficientBalance = 15,
    /// The beneficiaries_to_split list is empty or would leave the original
    /// will with no beneficiaries (issue #45).
    InvalidSplit = 16,
    /// The supplied pre-image does not match any commitment hash (issue #46).
    InvalidPreimage = 17,
    /// The hashed beneficiary slot has already been claimed (issue #46).
    AlreadyClaimed = 18,
    /// The threshold supplied for multi-sig is zero or exceeds the total
    /// number of owners (issue #44).
    InvalidThreshold = 19,
    /// Not enough co-owner authorisations were presented for a privileged
    /// action that requires the multi-sig threshold (issue #44).
    ThresholdNotMet = 20,
}
