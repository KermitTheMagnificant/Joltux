# Joltux: unofficial Game Jolt launcher for Linux
# AI WARNING: 
## This entire app was coded w/ Claude and GitHub Copilot, it was made more for personal use, to be honest (the official one sucks ass!)
Downloads games from Game Jolt and runs them from one place. Prefers native
Linux builds; falls back to Windows builds run through Proton (via
[umu-launcher](https://github.com/Open-Wine-Components/umu-launcher)) or Wine.

```
joltux info    <game id | url>        # list available builds, shows which would install
joltux install <game id | url>        # download + extract (add --windows to skip native Linux builds)
joltux list                           # installed games
joltux run     <game id>              # launch (native, or through Proton/Wine)
joltux uninstall <game id> [--prefix] # remove game (and optionally its Wine prefix)
joltux login                          # sign in to Game Jolt
joltux logout                         # remove the saved login
joltux update                         # install the latest version from GitHub
joltux setup                          # install Wine + umu-launcher and configure Proton
joltux gui                            # open the desktop launcher
joltux config show
joltux config set runner umu|wine
joltux config set umu_path /path/to/umu-run
joltux config set wine_path /path/to/wine
joltux config set proton_path GE-Proton        # or a path to a Proton folder
```

A game's ID is the number at the end of its URL:
`https://gamejolt.com/games/some-game/123456` -> `123456`.

## Install on Linux

You need a stable Rust toolchain and Cargo. If they are not installed,
[install Rust with rustup](https://rustup.rs/). The first build needs internet
access to download the Rust dependencies.

Open a terminal in the Joltux project directory and run:

```
./install.sh
```

The script builds Joltux, installs the `joltux` command in Cargo's bin
directory (usually `~/.cargo/bin`), and adds a Joltux shortcut to your desktop
application menu. It does not need `sudo`. Check the installation with:

```
joltux --help
```

If the shell says `joltux` was not found, add `~/.cargo/bin` to your `PATH`
and open a new terminal. To install directly without the script, run
`cargo install --path . --locked` from the project directory; this does not add
the application-menu shortcut. To uninstall, run `cargo uninstall joltux` and
remove `joltux.desktop` from `$XDG_DATA_HOME/applications` (or
`~/.local/share/applications` when `XDG_DATA_HOME` is unset). Run `cargo test`
to execute the tests.

## Run a game

Use the numeric ID at the end of a Game Jolt game URL, or pass the full URL:

```
joltux info 123456
joltux install 123456
joltux list
joltux run 123456
joltux gui
```

Use `joltux login` to sign in. The password is entered without being echoed
and is not saved; Joltux stores only the Game Jolt session cookie in its
owner-only config file. If Game Jolt requires a CAPTCHA, sign in through your
browser and paste its `frontend` session cookie when prompted. Use
`joltux logout` to clear the saved session.

Run `joltux update` to rebuild and replace the installed Joltux binary from the
GitHub repository's default branch. This requires Cargo and internet access;
it does not update a local source checkout.

Native Linux games need no additional runner. Windows games use `umu-run` by
default, so install umu-launcher and make sure `umu-run` is on your `PATH`.
Alternatively, install Wine and select it with `joltux config set runner wine`.

Run `joltux setup` to install the latest official umu-launcher into Joltux's
user data directory, install Wine from a supported system package manager, and
configure `GE-Proton` as the UMU runner. UMU downloads GE-Proton the first time
you launch a Windows game. Setup supports Debian/Ubuntu (`apt`), Fedora/RHEL
(`dnf`), Arch (`pacman`), and openSUSE (`zypper`); Wine installation may ask for
administrator authentication. Python 3 with `venv` support is required for the
user-local umu installation.

## Status: what is and isn't verified

Verified here (unit tests + smoke tests with fake games):
build selection, ID parsing, zip/tar.gz extraction (keeps the exec bit, blocks
path traversal), executable lookup, library/config files, native launch, and
the umu-run environment/arguments for Windows launches.

**NOT verified against the live Game Jolt service.** The sandbox this was
written in could not reach gamejolt.com. The endpoints and response shapes in
`src/api.rs` / `src/models.rs` were taken from the official client's source
(github.com/gamejolt/gamejolt, MIT) and may be wrong or have changed:

- `GET  /web/discover/games/:id` (title)
- `GET  /web/discover/games/packages/:id` (packages, releases, builds, launchOptions)
- `POST /web/discover/games/builds/get-download-url/:buildId` (JSON body
  `{"forceDownload": true}` is a guess; the official client's exact encoding
  wasn't checked)

This is an undocumented site API, not a public one. It can change without
notice, and you should check Game Jolt's terms before distributing a client
that uses it.

## Known gaps

- **CAPTCHA login.** Joltux supports username/password login and device approval,
  but CAPTCHA challenges must be completed in a browser before pasting the
  browser's `frontend` cookie into Joltux.
- **No library sync** (`/web/library` and `/web/client/sync` aren't used yet).
  You install by game ID/URL.
- **Build formats:** .zip and .tar.gz archives, plus standalone Windows .exe
  files. Installer builds are not installable.
- **No updates / no resume** of interrupted downloads.
- **No installer (.msi/setup.exe) builds**, same as the official client.
- **Symlinks inside zips** are written as plain files.

## Layout

```
main.rs     CLI (clap) and the install/run/list/uninstall commands
gui.rs      desktop launcher for the library, installation, account, and settings
api.rs      Game Jolt site API client
models.rs   serde types for API responses + the installed-game record
select.rs   which build to install (native Linux first, then Windows)
install.rs  download, extract, locate the executable
launch.rs   run natively or through umu-run / wine
runtime.rs  install umu-launcher and system Wine; configure GE-Proton
config.rs   settings (runner, proton path, session cookie)
library.rs  installed-games list
paths.rs    XDG locations (~/.config/joltux, ~/.local/share/joltux)
```
