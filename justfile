# julep-renderer - Development Tasks
#
# Run `just` to see available recipes.
# Run `just preflight` before pushing to catch CI failures locally.

export RUSTFLAGS := "-D warnings"

default:
    @just --list

# === CI Preflight ===

preflight: check clippy fmt test
    @echo ""
    @echo "All preflight checks passed!"

# === Individual Checks ===

check:
    cargo check --workspace --all-targets

clippy:
    cargo clippy --workspace --all-targets

fmt:
    cargo fmt --check

test:
    cargo nextest run --workspace --profile ci

test-cargo:
    cargo test --workspace

# === Build Variants ===

build:
    cargo build --workspace

build-release:
    cargo build --release --workspace

build-headless:
    cargo build --features headless

build-test-mode:
    cargo build --features test-mode

build-no-a11y:
    cargo build --no-default-features --features builtin-all,dialogs,clipboard,notifications

# === Test Variants ===

test-features:
    cargo nextest run --workspace --features headless
    cargo nextest run --workspace --features test-mode

coverage:
    #!/usr/bin/env bash
    if command -v cargo-llvm-cov &>/dev/null; then
        cargo llvm-cov --workspace --html
    elif command -v cargo-tarpaulin &>/dev/null; then
        cargo tarpaulin --workspace --out html
    else
        echo "Install cargo-llvm-cov or cargo-tarpaulin for coverage." >&2
        exit 1
    fi

# === Development Helpers ===

format:
    cargo fmt

test-filter pattern:
    cargo nextest run --workspace -- {{pattern}}

test-crate crate:
    cargo nextest run -p {{crate}}

clean:
    cargo clean

docs:
    cargo doc --workspace --open

# === Watch Mode ===

watch-check:
    cargo watch -x 'check --workspace --all-targets'

watch-test:
    cargo watch -x 'nextest run --workspace'

# === Dependency Health ===

audit:
    cargo audit

outdated:
    cargo outdated --workspace
