//! Coverage-guided fuzzing of `WillContract::update_beneficiaries`.
//!
//! The harness first creates a will from a sanitised version of the input, so
//! every iteration actually reaches the update path, then replays a series of
//! arbitrary replacement lists against it. Alongside "never abort", it checks
//! that a rejected update leaves the will untouched and that the beneficiary
//! reverse index always matches the stored list — including for the
//! neighbouring will the harness creates to catch cross-will index damage.
//!
//! Run with:
//!
//! ```sh
//! cargo +nightly fuzz run update_beneficiaries
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use will::fuzz_harness::{run_update_beneficiaries, UpdateBeneficiariesInput};

fuzz_target!(|input: UpdateBeneficiariesInput| {
    run_update_beneficiaries(&input);
});
