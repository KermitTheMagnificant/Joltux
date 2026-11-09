#!/bin/sh
set -eu

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

if ! command -v cargo >/dev/null 2>&1; then
    printf '%s\n' 'Cargo is required. Install Rust and Cargo from https://rustup.rs/.' >&2
    exit 1
fi

cargo install --path "$project_dir" --locked

cargo_root=${CARGO_INSTALL_ROOT:-${CARGO_HOME:-"$HOME/.cargo"}}
cargo_bin_dir=$cargo_root/bin
case ":$PATH:" in
    *":$cargo_bin_dir:"*)
        printf '%s\n' 'Joltux installed. Run it with: joltux --help'
        ;;
    *)
        printf 'Joltux installed to %s/joltux. Add %s to your PATH to run it by name.\n' \
            "$cargo_bin_dir" "$cargo_bin_dir"
        ;;
esac