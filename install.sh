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
# Sandboxed terminals (e.g. Flatpak VS Code) point XDG_DATA_HOME into the
# sandbox, where the desktop's menu never looks.
if [ -n "${FLATPAK_ID:-}" ]; then
    data_dir=$HOME/.local/share
fi
applications_dir=$data_dir/applications
icons_dir=$data_dir/icons/hicolor
desktop_file=$applications_dir/joltux.desktop
desktop_exec=$(printf '%s' "$cargo_bin_dir/joltux" | sed 's/\\/\\\\/g; s/"/\\"/g; s/`/\\`/g; s/\$/\\$/g')

mkdir -p "$applications_dir" "$icons_dir/scalable/apps" "$icons_dir/256x256/apps"
cp "$project_dir/assets/joltux-icon.svg" "$icons_dir/scalable/apps/joltux.svg"
cp "$project_dir/assets/joltux-icon-256.png" "$icons_dir/256x256/apps/joltux.png"
# An icon-theme.cache here without an index.theme is nearly empty, and Qt/KDE
# trust it over the files, hiding the icon. Earlier versions of this script
# created one, so remove it.
rm -f "$icons_dir/icon-theme.cache"

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

# Make the menu and task manager notice the new entry and icon now rather
# than at next login. Each tool only exists on some desktops.
run_host() {
    if [ -n "${FLATPAK_ID:-}" ] && command -v flatpak-spawn >/dev/null 2>&1; then
        flatpak-spawn --host "$@"
    else
        "$@"
    fi
}
for tool in kbuildsycoca6 kbuildsycoca5; do
    if run_host sh -c "command -v $tool" >/dev/null 2>&1; then
        run_host "$tool" >/dev/null 2>&1 || true
        break
    fi
done
run_host update-desktop-database -q "$applications_dir" >/dev/null 2>&1 || true

case ":$PATH:" in
    *":$cargo_bin_dir:"*)
        printf '%s\n' 'Joltux installed and added to your application menu. Run it with: joltux --help'
        ;;
    *)
        printf 'Joltux installed to %s/joltux and added to your application menu. Add %s to your PATH to run it by name.\n' \
            "$cargo_bin_dir" "$cargo_bin_dir"
        ;;
esac