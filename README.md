# UltraTerm

UltraTerm is a macOS desktop workspace for running multiple Oh My Pi (OMP) terminals inside one managed window. It combines a Rust/Tauri PTY backend with a React/xterm.js interface, a three-pane matrix layout, persistent tmux-backed sessions, Dictator voice input, PS4 controller navigation, and OLED-focused themes.

## Requirements

- macOS on Apple Silicon
- Rust toolchain
- Node.js and npm
- `tmux` available on `PATH` or configured with `TMUX_BIN`
- OMP available on `PATH` or configured with `OMP_BIN`

## Resilient launcher

UltraTerm uses the `scripts/omp-safe` launcher for persistent tmux-backed OMP sessions. Install it in any directory on `PATH`, or point UltraTerm to it explicitly:

```sh
ULTRATERM_OMP_LAUNCHER="$PWD/scripts/omp-safe" npm run tauri dev
```

Each matrix slot receives its own `ultraterm-matrix-N` tmux session. The session survives app restarts, automatically resumes OMP after an unexpected process exit, and hides tmux's status bar inside UltraTerm.

`OMP_PROFILE` is optional. When it is unset or empty, the launcher omits `--profile` and OMP selects its normal default. To select a profile explicitly, set `OMP_PROFILE` before starting UltraTerm. UltraTerm refuses to attach to a persistent session created with a different OMP binary, version, or profile; preserve that session under another name or stop it before changing configuration.

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
- `src-tauri/target/release/bundle/dmg/UltraTerm_0.2.0_aarch64.dmg`

## Self-update and restart

UltraTerm restarts itself cleanly — no installer package required. Terminal
sessions are tmux-backed, so every OMP session survives an app restart and
reattaches automatically when the new instance boots.

- **In-app**: use the **Restart** button in the System section of the side
  rail. UltraTerm detaches its terminal clients, exits, and relaunches; the
  workspace resumes with the same terminals.
- **From a terminal (including one inside UltraTerm)**:

```sh
scripts/self-update.sh            # build, swap into /Applications, restart
scripts/self-update.sh --restart  # install the current build and restart
```

The previous install is preserved under `.app-backup/` before each swap.

## Controls

- `⌘T`: open another OMP pane
- `⌘1` through `⌘9`: focus a pane
- `Escape`: restore a maximized pane
- Rebalance: restore the default three-pane matrix
- Reconnect all: recreate PTY clients while preserving tmux work

## License

MIT. See `LICENSE`.
