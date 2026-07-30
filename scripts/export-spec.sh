#!/usr/bin/env bash
# Builds the release wasm and exports the contract's embedded spec
# (function signatures, Will/Beneficiary/Guardian/WillStatus/WillError
# types) as a versioned JSON file under spec/.
#
# Usage: scripts/export-spec.sh
#
# Requires: cargo, the wasm32v1-none target, stellar-cli (`cargo install
# --locked stellar-cli --features opt`), and jq.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

version="$(cargo metadata --no-deps --format-version 1 \
  | jq -r '.packages[] | select(.name=="will") | .version')"

wasm_path="target/wasm32v1-none/release/will.wasm"
out_path="spec/will-v${version}.json"

echo "Building will contract (release, wasm32v1-none)..."
cargo build -p will --release --target wasm32v1-none

echo "Exporting spec to ${out_path}..."
stellar contract bindings json \
  --wasm "${wasm_path}" \
  --output "${out_path}"

echo "Done. Diff spec/${out_path} against the previous version to review drift."
