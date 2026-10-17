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

# Fuzz the mount/walk/create paths on patched and garbage images (nightly + cargo-fuzz); TARGET: mount or mount_patched
fuzz TARGET="mount_patched" SECONDS="300":
    cd fuzz && cargo +nightly fuzz run {{TARGET}} corpus/{{TARGET}} seeds/{{TARGET}} -- -max_total_time={{SECONDS}} -max_len=65536

# Library only: nothing to install on this PC
install: test

# Bring this PC up to main
deploy:
    tpm repos pull exfat_lite
