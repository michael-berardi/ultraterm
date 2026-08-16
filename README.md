# UltraTerm

UltraTerm is a macOS desktop app that runs all of your Oh My Pi (OMP) agent
terminals in one window. Sessions keep running when the app closes or updates
and reconnect automatically when it opens again. It includes token-usage
tracking with per-model history, UltraVox voice input, PS4 controller
navigation, and a choice of OLED-first themes.

## Requirements

- macOS on Apple Silicon
- OMP installed and on your `PATH`
- `tmux` (recommended; required for sessions that survive app restarts)

## Install

```bash
curl -fsSL https://raw.githubusercontent.com/michael-berardi/ultraterm/main/install.sh | bash
```

The installer downloads the signed, notarized release, verifies its checksum
and code signature, and installs UltraTerm to `~/Applications`. Add
`--system` for a machine-wide install in `/Applications`.

Then open UltraTerm like any other app. Each pane starts an OMP terminal in
your home directory; use the profile picker next to "New terminal" to choose
which OMP profile a pane launches with.

## Updates

UltraTerm checks for updates on every launch. When a new version is available
you can update in one click — the app downloads the signed release, verifies
it, and relaunches itself while your terminals keep running. Enable "Install
updates automatically" in the update prompt to skip the prompt entirely.

## Controls

- `⌘T` — new terminal
- `⌘1`–`⌘9` — focus a pane
- `Escape` — restore a maximized pane
- Sidebar: token usage dials, per-terminal model and activity, token history
  (click any model to filter), settings, and restart

## Privacy

On first launch UltraTerm asks whether you want to share anonymous usage
stats. Before opt-in it creates no telemetry identifier and sends no network
traffic. When enabled it sends a random install ID, app version, platform,
architecture, UTC day, and coarse daily counters for successful terminal pane
starts and OMP session starts to
`https://analytics.libertydesign.studio/api/app-telemetry/event`: one launch,
at most one heartbeat per UTC day, and at most one usage report per UTC day.
It never sends names, file paths, prompts, terminal content, model or token
data, commands, URLs, network identifiers, or secrets. The choice can be
changed anytime under Settings → Privacy; declining persists and erases the
local telemetry ID, send timestamps, pending counters, and legacy app
telemetry state.

## Development

Requires Node.js and Rust.

```sh
npm install
npm run tauri dev
```

Checks and tests:

```sh
npm run check
npm test
```

Local build + install into `/Applications` (backs up the previous app to
`.app-backup/`):

```sh
scripts/self-update.sh
```

## Releasing

```sh
npm run release:publish
```

Builds, signs (Developer ID), notarizes, and publishes the `.zip` and `.pkg`
assets to GitHub Releases. Requires the signing identities and a notarytool
keychain profile; see `scripts/package-release.sh` for the environment
variables.

Release checklist:

1. Bump the version in `package.json`, `src-tauri/Cargo.toml`, and
   `src-tauri/tauri.conf.json` together.
2. **Review this README against the release** — features, controls, and
   privacy notes change over time; update anything that drifted.
3. `npm run check && npm test`
4. `npm run release:publish`

## License

MIT. See `LICENSE`.
