#!/usr/bin/env bash
# Runs the CI rows of the ADR-013 layers table in order.
# Stops at the first failing step and exits with its status.
# Not run here (owner machine only): `cargo test --workspace -- --ignored`
# and `cargo xtask bench`.
set -euo pipefail

cd "$(dirname "$0")/.."

step() {
    echo "==> $*"
    "$@"
}

step cargo build -p tl-gpu-probe --features fake --locked

# Headless Vulkan for the render tests: use Mesa lavapipe unless the caller chose an ICD.
# Ubuntu names the file lvp_icd.x86_64.json, Arch names it lvp_icd.json.
if [[ -z "${VK_ICD_FILENAMES:-}" ]]; then
    for lvp_icd in /usr/share/vulkan/icd.d/lvp_icd.x86_64.json /usr/share/vulkan/icd.d/lvp_icd.json; do
        if [[ -f "$lvp_icd" ]]; then
            export VK_ICD_FILENAMES="$lvp_icd"
            break
        fi
    done
fi

step cargo test --workspace --locked
step cargo test -p tl-ui --features render-tests,test-hooks --locked
step cargo fmt --all --check
step cargo clippy --workspace --all-targets --locked -- -D warnings
step cargo xtask check-deps
