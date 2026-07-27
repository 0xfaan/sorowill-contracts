//! Coverage-guided fuzzing of `WillContract::create_will`.
//!
//! libFuzzer supplies raw bytes, `arbitrary` decodes them into a
//! [`CreateWillInput`], and the shared harness invokes the contract and checks
//! its invariants. A crash here means `create_will` either aborted on input it
//! should have rejected with a `WillError`, or accepted input that left a will
//! violating one of its documented invariants — the harness prints which, plus
//! the offending input.
//!
//! Run with:
//!
//! ```sh
//! cargo +nightly fuzz run create_will
//! ```

#![no_main]

use libfuzzer_sys::fuzz_target;
use will::fuzz_harness::{run_create_will, CreateWillInput};

fuzz_target!(|input: CreateWillInput| {
    run_create_will(&input);
});
