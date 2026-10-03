# SteamIDfinder — spec (v0.1.0)

Agreed in a grilling session on 2026-10-03. This is the record of what the app does and why.

## Purpose

A small desktop app: give it a Steam ID in any common format, get back a clickable Steam
profile link plus the profile's current name, avatar and name history. Runs on Linux and
Windows. Launchable from any XDG app launcher (Noctalia, rofi, fuzzel, GNOME, KDE, …).

## Input

- Accepted formats, detected automatically:
  - SteamID64 — `76561197960287930`
  - SteamID2 — `STEAM_0:0:11101` (`STEAM_1:` also accepted)
  - SteamID3 — `[U:1:22202]` (brackets optional)
  - Account ID — `22202` (bare 32-bit number)
  - Profile URLs — `steamcommunity.com/profiles/<id64 or [U:1:N]>` and `steamcommunity.com/id/<name>`
  - Bare custom URL (vanity) names — `gabelogannewell`
- Batch: the input is split on whitespace, commas and semicolons; each token is one lookup.
  Invalid tokens are flagged inline.
- Command-line arguments are treated the same way and looked up on startup
  (`steamidfinder 7656… STEAM_0:1:…`). This is how the Noctalia `/steam` entry feeds the app.

## Lookup

- ID conversions (64 / 2 / 3 / account ID / profile URL) are computed offline.
- Profile data comes from Steam's keyless XML endpoint
  `https://steamcommunity.com/profiles/<id64>/?xml=1` (or `/id/<name>/?xml=1` for vanity
  names, which also resolves them to a SteamID64). Fields used: name, full avatar, online
  state / current game, privacy, VAC ban, trade ban, custom URL. An `<error>` reply means
  "profile not found".
- Previous names come from the keyless `https://steamcommunity.com/profiles/<id64>/ajaxaliases`
  JSON endpoint. Dates there are Valve display strings and are shown verbatim. Private
  profiles may return an empty list.
- All network access lives in `src/profile.rs` so a Steam Web API key backend can be added
  later without touching the UI.
- Lookups run on a pool of 4 worker threads.
- On failure the card still shows the offline link and ID formats, plus an error note and Retry.

## UI (Rust + egui/eframe)

- **Results tab** — the session's running list, newest first. A batch is inserted at the top
  in input order. Looking up an ID that's already listed moves it to the top and refreshes
  it. Toolbar: **Copy all links**, **Clear** (clears the session list only).
- **Card**: avatar, current name, "aka …" line (first 3 previous names, `+N`), hyperlink
  (opens the default browser), **Copy** button, badges (online state / game, VAC ban,
  trade ban, privacy), collapsible **Details** with every ID format (each copyable) and the
  full name history with dates. Renames the app saw itself are tagged "seen by you".
  Each card has a dismiss button.
- **History tab** — persistent across restarts:
  - Every successfully resolved profile is recorded (offline/not-found lookups are not).
  - Up to 500 entries, deduplicated by SteamID64, newest first.
  - Stores id64, last-seen name, avatar (cached on disk), first/last lookup time, lookup
    count, custom URL and renames the app observed (`was: OldName`).
  - Filter box (name, previous names, any ID format, custom URL), click an entry to look it
    up again, copy link, delete one, Clear all (with confirmation).
  - Location: `~/.local/share/steamidfinder/` (Linux, respects `XDG_DATA_HOME`),
    `%APPDATA%\SteamIDfinder\` (Windows).
  - Safe with several windows open: every mutation is read-modify-write with an atomic
    rename, so windows never overwrite each other's deletions.
- Keys: **Enter** looks up, **Ctrl+Enter** looks up and opens the top result in the browser,
  **Esc** quits.
- Theme follows the system light/dark setting, with a Steam-blue accent.
- Each launch opens a new window (no single-instance IPC).
- Window title `SteamIDfinder`, Wayland app_id / X11 class `steamidfinder`.
- System CJK fonts are loaded as fallbacks when present so non-Latin names render.

## Icon

Custom, simple: a slider-crank mechanism whose crank wheel is a magnifier lens. Deliberately
not Steam's logo (trademark).

## Distribution

- Linux tarball `steamidfinder-<ver>-linux-x86_64.tar.gz`: binary, icon, `.desktop`,
  `install.sh`, README, LICENSE.
- Linux AppImage `SteamIDfinder-<ver>-x86_64.AppImage`.
- Windows portable `SteamIDfinder-<ver>-windows-x86_64.exe`: embedded icon, no console
  window, no installer. Unsigned (SmartScreen warns).
- Linux CI builds run on Ubuntu 22.04 for glibc compatibility.
- `install.sh` (POSIX sh): works from a release tarball or a source checkout (builds if
  needed); per-user install by default (`~/.local`), `--system` for `/usr/local` via sudo,
  `--uninstall`; rewrites the desktop entry's `Exec` to the absolute binary path; refreshes
  desktop/icon caches when the tools exist; warns if the bin dir isn't on `PATH`.
- `packaging/arch/PKGBUILD` — a `steamidfinder-git` package for self-builds with `makepkg`.
  Not submitted to the AUR.
- `extras/noctalia/steamidfinder.toml` — optional Noctalia launcher entry (`/steam <ids>`).

## Repo and CI

- Public GitHub repo `Alelau18/SteamIDfinder`, MIT.
- CI on every push / PR: `cargo test` plus Linux and Windows builds uploaded as artifacts.
- Release workflow on `v*` tags publishes the three assets to a GitHub Release.
- `scripts/build.sh`: local Linux build (tests, release binary, optional tarball/AppImage).
  Windows builds come only from CI.

## Out of scope for v0.1.0

Steam Web API key backend, a live Noctalia Luau launcher plugin, Noctalia colour matching,
single-instance mode, AUR submission, code signing, macOS.
