# Changelog

## Unreleased

### Breaking change: mixed percentage / fixed-amount beneficiary allocations

`Beneficiary.basis_points: u32` has been replaced by
`Beneficiary.allocation: Allocation`, where:

```rust
pub enum Allocation {
    Percentage(u32),  // basis points (1 bp = 0.01%) of whatever remains
                       // after every FixedAmount beneficiary is paid
    FixedAmount(i128), // an exact amount, paid before any percentage split
}
```

A single will may now mix both kinds — e.g. one beneficiary who receives an
exact amount (e.g. "my sister gets exactly 5,000 USDC") and the rest who
split whatever remains by percentage, without anyone needing to recompute
percentages by hand every time the balance changes via `top_up`.

Validation (`assert_valid_allocations`, formerly `assert_valid_percentages`)
now additionally requires:
- the sum of every `FixedAmount` on a will never exceeds the will's balance;
- all `Percentage` shares still sum to exactly 10,000 basis points (100% of
  the remainder after fixed amounts);
- a will made up entirely of `FixedAmount` beneficiaries must account for
  the whole balance, since no percentage beneficiary is left to absorb a
  remainder.

`distribute()` (full release) and `distribute_tier()` (partial/tiered
release) now pay fixed-amount beneficiaries first, then split whatever
remains among percentage-based beneficiaries. Fixed-amount beneficiaries are
intentionally excluded from tiered partial releases — a fixed promise is
only meaningful once, at final release, so paying a fraction of it early
would either shortchange or double-pay them; they are always paid in full at
final release instead.

**Migration note for existing deployed wills:** this is a storage-breaking
change. Any `Will` written by a contract version prior to this change has
beneficiaries stored with a `basis_points` field that no longer exists in
the `Beneficiary` type. Before upgrading a live deployment:

1. Bump `CURRENT_SCHEMA_VERSION` and treat every will with
   `schema_version < CURRENT_SCHEMA_VERSION` as needing migration.
2. Extend `migrate_will` to rewrite each legacy beneficiary entry as
   `Beneficiary { address, allocation: Allocation::Percentage(basis_points) }`
   — this is a lossless, purely additive reinterpretation, since every
   pre-existing will was pure-percentage.
3. Until a will is migrated, reject `update_beneficiaries` /
   `update_will_settings` calls that would write a mixed or fixed-amount
   list against it, to avoid a partially-migrated will with an ambiguous
   on-chain shape.

New error: `WillError::FixedAmountExceedsBalance` (21).
