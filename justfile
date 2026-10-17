# exfat_lite: a small no-alloc exFAT implementation for SD cards.

default:
    @just --list

# Tests (host, std feature) plus a no_std build check
test:
    cargo test --features std --quiet
    cargo check --no-default-features --target thumbv8m.main-none-eabihf

# Format check, cargo-sort, typos and clippy without warnings
lint:
    cargo fmt --all --check
    cargo sort --check
    typos
    cargo clippy --all-targets --features std -- -D warnings
    cargo clippy --no-default-features --target thumbv8m.main-none-eabihf -- -D warnings

# Library only: nothing to install on this PC
install: test

# Bring this PC up to main
deploy:
    tpm repos pull exfat_lite
