# Contributing

## Setup

### Rust

Install [Rust](https://rustup.rs) (stable). The project pins **Rust 1.82.0**
via `rust-toolchain.toml` -- rustup will install this automatically.

### System Dependencies

On Debian/Ubuntu (and derivatives):

    sudo apt-get install -y build-essential pkg-config cmake \
        libxkbcommon-dev libwayland-dev libx11-dev \
        libfontconfig1-dev libfreetype-dev

- **build-essential** -- C compiler and linker (required by native crate builds)
- **pkg-config** -- locates system libraries for native crate builds
- **cmake** -- build system used by some native dependencies
- **libxkbcommon-dev** -- keyboard handling (XKB)
- **libwayland-dev** -- Wayland display protocol
- **libx11-dev** -- X11 display protocol
- **libfontconfig1-dev** -- font discovery and configuration
- **libfreetype-dev** -- font rasterization

On Arch Linux: `pacman -S base-devel pkgconf cmake libxkbcommon wayland libx11 fontconfig freetype2`

### Dev Tools

Install [just](https://just.systems) and [cargo-nextest](https://nexte.st).

## Development Workflow

    just check           # Fast compile check
    just test            # Run tests (nextest)
    just test-cargo      # Run tests (standard cargo, if nextest unavailable)
    just test-filter foo # Run tests matching "foo"
    just test-crate name # Run tests for a specific crate

## Before Committing

    just preflight

This runs the same checks as CI: `check`, `clippy`, `fmt`, `test`.

## Code Style

- `cargo fmt` for formatting (run `just format` to auto-fix)
- `cargo clippy` for linting (warnings are errors via RUSTFLAGS)
- Follow existing patterns in the codebase

## Commit Conventions

Use conventional commits:

    feat(core): add gradient fill support
    fix(renderer): handle missing window on close
    docs: update feature flag table
    test(codec): add msgpack round-trip tests
    refactor(widgets): extract common prop parsing

## Extension Development

julep-core is the public SDK for writing widget extensions. The quick path:

1. Create a Rust crate that depends on `julep-core`.
2. Import everything from `julep_core::prelude::*`.
3. Implement the `WidgetExtension` trait (three required methods:
   `type_names()`, `config_key()`, `render()`).
4. For iced types not in the prelude, use `julep_core::iced::*` instead
   of adding a direct `iced` dependency -- this avoids version conflicts.

See the Julep project's `docs/extensions.md` for the full guide.

## Pull Requests

1. Create a branch from `master`
2. Make changes, ensure `just preflight` passes
3. Submit PR with a clear description
