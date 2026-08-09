# UltraTerm

UltraTerm is a macOS desktop workspace for running multiple Oh My Pi (OMP) terminals inside one managed window. It combines a Rust/Tauri PTY backend with a React/xterm.js interface, a three-pane matrix layout, persistent tmux-backed sessions, Dictator voice input, PS4 controller navigation, and OLED-focused themes.

## Requirements

- macOS on Apple Silicon
- OMP available on `PATH` or configured with `OMP_BIN`
- `tmux` is recommended for persistent sessions; UltraTerm falls back to a
  direct OMP session when it is unavailable

Node.js and Rust are required only when building from source.

## Installation

For an AI agent or a user account install (no compiler or `sudo` required):

```bash
curl -fsSL https://raw.githubusercontent.com/michael-berardi/ultraterm/main/install.sh | bash
```

This installs the prebuilt app to `~/Applications/UltraTerm.app`. The resilient
`omp-safe` launcher is bundled inside the app. For a machine-wide install:

```bash
curl -fsSL https://raw.githubusercontent.com/michael-berardi/ultraterm/main/install.sh | bash -s -- --system
```

The installer verifies the published SHA-256 checksum and macOS code signature.
Release assets are also available from
[GitHub Releases](https://github.com/michael-berardi/ultraterm/releases).

## Resilient launcher

UltraTerm bundles the `scripts/omp-safe` launcher used for persistent
tmux-backed OMP sessions. A source checkout can override it during development:

```sh
ULTRATERM_OMP_LAUNCHER=\"$PWD/scripts/omp-safe\" npm run tauri dev
```

Each matrix slot receives its own `ultraterm-matrix-N` tmux session. The session survives app restarts, automatically resumes OMP after an unexpected process exit, and hides tmux's status bar inside UltraTerm.

`OMP_PROFILE` is optional. When it is unset or empty, the launcher omits `--profile` and OMP selects its normal default. To select a profile explicitly, set `OMP_PROFILE` before starting UltraTerm. OMP upgrades do not interrupt persistent-session reconnects. UltraTerm still refuses to attach to a session created with a different OMP binary path or profile; preserve that session under another name or stop it before changing configuration.

### Configuration

| Variable | Purpose |
| --- | --- |
| `ULTRATERM_OMP_LAUNCHER` | Executable path or command name for `omp-safe`; otherwise resolved from `ULTRATERM_PATH` or `PATH`. |
| `ULTRATERM_PATH` | Optional command search path prepended to the inherited `PATH` for backend lookup and launched OMP sessions. |
| `ULTRATERM_WORKING_DIRECTORY` | Default directory for new terminals. If unset, UltraTerm uses the user's home directory; an invalid configured path is rejected. |
| `OMP_BIN` | Executable path or command name for OMP; otherwise `omp` is resolved from `PATH`. |
| `OMP_PROFILE` | Optional OMP profile passed as `--profile`; unset uses OMP's default. |
| `TMUX_BIN` | Executable path or command name for tmux; otherwise resolved from `PATH` with standard installation paths as fallbacks. |
| `CAFFEINATE_BIN` | Optional executable path or command name for macOS `caffeinate`; when unavailable, the launcher runs without it. |
| `PI_CODING_AGENT_DIR` | Explicit OMP agent data directory used by OMP, history search, and telemetry. |
| `OMP_HISTORY_DB` | Explicit history database path for UltraTerm history search. |

Without `PI_CODING_AGENT_DIR`, history and telemetry ask OMP for its effective agent directory with `omp config path`, including any default profile selected by OMP configuration. If that discovery command is unavailable, UltraTerm falls back to `~/.omp/agent`, or `~/.omp/profiles/$OMP_PROFILE/agent` when a profile is explicit.

## Development

```sh
npm install
npm run tauri dev
```

## Verification

```sh
npm run check
npm test
```

## Production build

```sh
npm run tauri build
```

Build artifacts:

- `src-tauri/target/release/bundle/macos/UltraTerm.app`
- `src-tauri/target/release/bundle/dmg/UltraTerm_<version>_aarch64.dmg`

## Updating and restarting

Re-run the installer to download and atomically replace UltraTerm with the
latest published release. Terminal sessions are tmux-backed, so OMP sessions
survive the app restart and reattach when UltraTerm opens again.

```bash
curl -fsSL https://raw.githubusercontent.com/michael-berardi/ultraterm/main/install.sh | bash
```

For local development builds, `scripts/self-update.sh` still builds, swaps the
app into `/Applications`, and restarts it. The previous development install is
preserved under `.app-backup/`.

## Publishing a release

Releases are built locally, Developer ID signed, notarized, checksum-staged,
and published without GitHub Actions:

```bash
export APPLE_SIGNING_IDENTITY=\"Developer ID Application: …\"
export APPLE_INSTALLER_SIGNING_IDENTITY=\"Developer ID Installer: …\"
export NOTARYTOOL_PROFILE=\"your-keychain-profile\"
npm run release:publish
```

`npm run release:package` only stages the `.zip`, `.pkg`, and checksum assets in
`dist/release/`. Publishing additionally requires an authenticated GitHub CLI.

## Controls

- `⌘T`: open another OMP pane
- `⌘1` through `⌘9`: focus a pane
- `Escape`: restore a maximized pane
- Rebalance: restore the default three-pane matrix
- Reconnect all: recreate PTY clients while preserving tmux work

## License

MIT. See `LICENSE`.
