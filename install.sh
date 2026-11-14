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
data_dir=${XDG_DATA_HOME:-"$HOME/.local/share"}
applications_dir=$data_dir/applications
icons_dir=$data_dir/icons/hicolor
desktop_file=$applications_dir/joltux.desktop
desktop_exec=$(printf '%s' "$cargo_bin_dir/joltux" | sed 's/\\/\\\\/g; s/"/\\"/g; s/`/\\`/g; s/\$/\\$/g')

mkdir -p "$applications_dir" "$icons_dir/scalable/apps" "$icons_dir/256x256/apps"
cp "$project_dir/assets/joltux-icon.svg" "$icons_dir/scalable/apps/joltux.svg"
cp "$project_dir/assets/joltux-icon-256.png" "$icons_dir/256x256/apps/joltux.png"
# Refresh the icon cache if this desktop keeps one; harmless if it doesn't.
gtk-update-icon-cache -q -t "$icons_dir" 2>/dev/null || true

cat >"$desktop_file" <<EOF
[Desktop Entry]
Type=Application
Name=Joltux
Comment=Unofficial Game Jolt launcher for Linux
Exec="$desktop_exec" gui
Icon=joltux
StartupWMClass=joltux
Terminal=false
Categories=Game;
StartupNotify=true
EOF

case ":$PATH:" in
    *":$cargo_bin_dir:"*)
        printf '%s\n' 'Joltux installed and added to your application menu. Run it with: joltux --help'
        ;;
    *)
        printf 'Joltux installed to %s/joltux and added to your application menu. Add %s to your PATH to run it by name.\n' \
            "$cargo_bin_dir" "$cargo_bin_dir"
        ;;
esac