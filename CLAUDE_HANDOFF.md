# Linku / Cap Drive — handoff to Claude Code

Updated: 23 October 2026. Read `AGENTS.md` first. This document supplements the
repository rules; it does not replace them.

## What Olivier wants next

Explore visual styles for a Linku agency version of Cap, then develop the chosen
direction into consistent design tokens and reusable native UI components.
Olivier is moving to Claude to work visually with `/design` and plans to return
to Codex afterward. Communicate in plain language; Dutch is welcome.

Start with visual exploration of the compact recorder. Do not assume that any
brand palette, typography, app name, or mockup has received final approval.
Linku's public website is https://linku.nl, but no official brand guide or Figma
file has been provided. Obtain actual references before claiming a design is
official Linku branding.

Three exploratory recorder concepts were made: Quiet studio (soft green,
restrained native utility), Editorial (warm orange, serif heading, source list),
and Control room (blue/violet, segmented sources, denser controls). These are
concepts only, not implemented native screens. Quiet studio was visible in the
last preview; this is not an explicit final selection by Olivier.

A portable, browser-openable preview is at `../outputs/linku-style-exploration.html`
relative to this repository. It is a design reference, not the product frontend.
Its settings icon is decorative and its record button does not record anything.

Suggested process: compare directions, refine the selected recorder design,
record approved decisions in a design document, map those decisions to native
tokens/components, then apply them to recorder, settings and editor/export.
Preserve both light and dark appearance and real recording/upload behavior.

## Repository and current state

- Repository root on this Mac:
  `/Users/oliviermathijssen/Documents/Codex/2026-10-23/so-x20/Cap`
- Current branch: `feat/direct-google-drive`.
- HEAD at handoff preparation: `8d0c8ee` (`docs: verify Drive export upload and playback`).
- Fork: https://github.com/olivier-mathijssen-linku/Cap
- Git remote `fork` is Olivier's fork; `origin` is `CapSoftware/Cap` upstream.
  Do not accidentally push to upstream.
- The working tree was clean before these handoff documentation changes.
  The handoff files have not been committed or pushed by Codex.
- Keep the existing AGPLv3 license and copyright notices.

## Work on the actual native desktop app

The current desktop app is **Rust + GPUI**, in `apps/desktop-gpui`.
It is not a browser frontend. HTML/CSS mockups can help exploration but must be
translated to native GPUI for implementation.

`apps/desktop` is the older Tauri/Solid implementation, retained for the dev
harness and assets. `apps/web`, `packages/ui` and `packages/ui-solid` are not the
component library that renders this native desktop app. Do not style those and
claim the installed app has changed.

Useful files, relative to repository root:

| Path | Purpose |
| --- | --- |
| `apps/desktop-gpui/src/theme.rs` | `Theme`, light/dark appearance, `MaterialTokens`, separate `EditorPalette`, `UI_FONT` |
| `apps/desktop-gpui/src/ui/mod.rs` | Shared native component library and surface conventions |
| `apps/desktop-gpui/src/ui/` | Buttons, toggles, fields, selects, sliders, segmented controls, cards, tabs, menus, tooltips |
| `apps/desktop-gpui/src/main_window.rs` | Compact recorder screen and capture-source controls |
| `apps/desktop-gpui/src/settings_window.rs` | Settings window shell |
| `apps/desktop-gpui/src/settings_pages.rs` | Settings contents, including Google Drive connection |
| `apps/desktop-gpui/src/editor_sidebar.rs` | Editor sidebar |
| `apps/desktop-gpui/src/editor_panels.rs` | Editor control panels |
| `apps/desktop-gpui/src/editor_export.rs` | Export UI and upload integration |
| `apps/desktop-gpui/src/direct_drive.rs` | Native adapter to the Drive library |
| `integrations/direct-drive/` | Independent Rust library for Google OAuth and uploads |
| `integrations/direct-drive/README.md` | Detailed integration behavior and test status |

The native shared components use constructors such as `::body`, `::settings`,
`::glass`, and `::plain` for different surfaces. The editor has a separate palette;
changing only the main theme may not affect every screen. Some screen spacing and
colors remain local. No native component gallery was identified in the prior
investigation. Prefer extending existing components over creating duplicates.

The recorder is approximately 330 logical pixels wide with custom macOS chrome.
Its real controls include Display, Window, Area, Camera Only, camera, microphone,
and system audio. Simplified concept mockups do not define the complete feature
set; do not silently drop native features while implementing them.

## Working Google Drive behavior to preserve

The `direct-drive` feature is enabled by default in the GPUI app.
Studio recording -> local editing/export -> direct upload to Google Drive ->
copy a normal `https://drive.google.com/file/d/.../view` link.

- Uses Google's **Desktop app** OAuth client with browser authorization,
  loopback callback and PKCE, and only the `drive.file` scope.
- OAuth refresh tokens are in the OS credential store, not repository files.
- Client configuration is already installed on this Mac at
  `~/Library/Application Support/CapDrive/google-oauth.json`.
  `CAP_DRIVE_OAUTH_FILE` can override this path.
- Do not print, copy into source control, or paste credential JSON, access tokens,
  or refresh tokens into chat. A design task does not require reading them.
- Creates a private `Cap Recordings` folder. Olivier accepts normal Drive links
  and will give viewers access through Google's Share dialog.
- Each upload creates a new file; replacing an existing Drive file is not built.
- Uploads use resumable aligned 8 MiB chunks and verify completion size.
  Sessions are not persisted across app restarts.
- Instant Mode is hidden/blocked; it depends on Cap's streaming service.
- Direct Drive export bypasses Cap login, subscription checks and Cap hosting.
- Settings use the separate `CapDrive` directory; upstream auto-updates are disabled.
- Screenshots and other cloud account features have not been converted to Drive.

## What has actually been verified

Previously completed in this session, not rerun for this documentation handoff:

- All eight Drive library tests and strict Clippy passed.
- The full native desktop binary built after Xcode and CMake installation.
- A local app bundle was packaged, signed locally, verified and opened.
- Olivier completed onboarding and granted app permissions.
- Google authorization completed and Settings showed Drive connected.
- A generated three-second silent clip was imported into the app, exported at
  720p/30 fps, uploaded to Drive and played successfully via the returned link.
  Drive showed the file private to its owner.

Not yet verified: a live screen/microphone recording session, access by an invited
second viewer, distribution/notarization, or a newly implemented visual design.
The imported test clip is not evidence that live capture has been tested.
The prototype omits Cap's CLI diagnostics sidecar. The Mac reported memory
pressure during export; Olivier closed applications and explicitly proceeded
for that test. Be mindful of build/export load.

## Local build environment

This machine is Apple Silicon. Full Xcode and CMake are installed. Existing
build outputs and native dependencies are available; avoid deleting them or
reinstalling everything without diagnosing an actual need.

The working Rust toolchain is task-local. Build from the GPUI directory so its
`rust-toolchain.toml` selects stable instead of the older root toolchain:

```sh
cd /Users/oliviermathijssen/Documents/Codex/2026-10-23/so-x20/Cap/apps/desktop-gpui
export CARGO_HOME=/Users/oliviermathijssen/Documents/Codex/2026-10-23/so-x20/work/toolchain/cargo
export RUSTUP_HOME=/Users/oliviermathijssen/Documents/Codex/2026-10-23/so-x20/work/toolchain/rustup
export PATH="$CARGO_HOME/bin:$PATH"
cargo build --bin cap-gpui
```

For Rust UI edits, follow `AGENTS.md`: scoped formatting and
`cargo check -p cap-desktop-gpui` from the GPUI directory. No full build is needed
just to explore mockups or edit documentation. Do not start extra shared dev
servers; see the repository's pre-generation invariants.

The Drive library has its own workspace. From repository root with the same
toolchain environment, when changing Drive logic:

```sh
cargo test --manifest-path integrations/direct-drive/Cargo.toml
cargo clippy --manifest-path integrations/direct-drive/Cargo.toml --all-targets -- -D warnings
```

Native dependencies are in `target/native-deps`, referenced by `.cargo/config.toml`.
The GPUI dependency checkout is the sibling `../zed-cap`; preserve it.
The setup helper is `scripts/prepare-gpui-dependency.sh` if genuinely needed.
The full GPUI README contains historical implementation notes; the fork-specific
Drive README is the better source for current fork behavior.

## Existing local artifacts

Paths below are relative to the parent of this repository:

- `outputs/Cap Drive.app`: working prototype, bundle ID `local.capdrive.desktop`.
- `outputs/Cap Drive.zip`: packaged prototype archive.
- `outputs/cap-drive-prototype.patch`: existing Drive implementation patch.
- `outputs/linku-style-exploration.html`: portable visual concepts.
- `work/package_cap_drive.py`: local packaging script, includes native helper,
  frameworks and assets and performs local signing. It deliberately refuses to
  overwrite an existing output bundle. It is outside the Git repository.
- `work/cap-drive-generated-test.mp4`: generated import/export test input.

Editing source does not update the existing `.app` automatically. A later native
implementation needs a fresh build and packaging step. Preserve the known-working
prototype while making a new reviewable bundle. Changing bundle identity can
affect macOS permissions; don't rename it incidentally during design work.

## Leave a useful handoff back

Record the chosen visual direction, approved tokens, changed files, checks run,
remaining issues and whether the native app was rebuilt. Keep mockup-only work
clearly distinguished from changes actually implemented in GPUI. Preserve
Olivier's uncommitted work and avoid unrelated rewrites.

## Linku recorder redesign (Claude, 23 October 2026)

Design was explored on a claude.ai Design canvas (private to Olivier):
https://claude.ai/artifact/DmXdvUZgEv5nHKbWgxrTJx. Board "A2 · Native Linku
simpel" is the approved recorder; "Stijlrichting" records the style direction.

### Approved decisions

- Layout (Loom-style, all Dutch): Video / Afbeelding switch; one source row
  (Scherm, Venster, Gebied, Alleen camera) with a dropdown and "Kies" to pick a
  specific display or window; rows for Camera, Microfoon and Systeemgeluid with
  Aan/Uit pills; a large "Opname starten" button; a footer with Instellingen,
  Opnames, Afbeeldingen and Meer. Meer holds Teleprompter, Over de opnamemodi,
  Hulp & rondleiding and Wat is er nieuw. No logo, no plan badge, no info button.
- Feelings per moment: confidence (muted blue), enthusiasm at start (yellow
  `#ffc83d`), relaxed while recording (sage green `#4fae8c`). Only the blue is
  implemented so far; countdown, floating bar and "Opname staat veilig" are
  concepts on the canvas.
- Confidence blue is the muted Linku blue `#3b66c4` (tint `#edf1fa`, text
  `#2a4b94`), not the website's `#2969ff`.
- Font: Sen (Google Fonts, SIL OFL), bundled as four static weights.
- System audio defaults to on for users who never set it.

### Implemented in GPUI (not yet rebuilt or packaged)

- `src/theme.rs`: `UI_FONT` is now "Sen" on every platform (all windows:
  recorder, editor, settings, onboarding, overlays, controls, teleprompter,
  screenshot editor, mode select), plus `RecorderPalette` (light/dark tokens).
  Content fonts (caption presets, `MONO_FONT`, Source Serif heading) are unchanged.
- `assets/fonts/Sen-{Regular,Medium,SemiBold,Bold}.ttf`, registered in
  `src/assets.rs`; new icons `video-off.svg` and `photo-camera.svg`.
- `src/main_window.rs`: new home layout (`render_home`, mode switch, source row
  and menu, device rows, start button, footer, Meer menu). Header keeps only the
  traffic lights and drag region. Display/Window/Camera Only start directly from
  the button; Area still opens the overlay to draw. Picking a display or window
  in the "Kies" panel no longer opens the full-screen overlays.
- `src/main.rs`: main window height 435 -> 463.
- `src/store.rs`: `systemAudio` defaults to true when unset.
- Panels (device pickers, libraries, mode info), settings and editor are still
  English and in the old style.

Checks: `rustfmt` on touched files; `cargo check -p cap-desktop-gpui` clean apart
from one pre-existing deprecation warning in `permissions/screen_capture.rs`;
the `store`, `assets`, `theme` and `main_window` unit tests pass (73). The test
binary needs `DYLD_FALLBACK_LIBRARY_PATH` set to
`target/native-deps/Spacedrive.framework/Versions/A/Libraries` to find FFmpeg.
One store test was updated for the new system-audio default, and two theme
asserts for the removed mode-pill ring were dropped.
Full unit suite passes (1301) after the app-wide font change.
First test build and packaged with
`python3 work/package_cap_drive.py "Cap Drive Linku-test.app"` (the script now
takes an optional output name) to `outputs/Cap Drive Linku-test.app`, signed
locally and verified. Same bundle ID as `Cap Drive.app`, which is untouched;
quit one before opening the other.
The test bundle is now signed with the local "Linku Loom Dev" certificate:
`CAP_DRIVE_SIGN_IDENTITY="Linku Loom Dev" python3 work/package_cap_drive.py
"Cap Drive Linku-test.app"`. Its designated requirement is bundle ID plus that
certificate, so macOS privacy permissions survive rebuilds. Without the
variable the script still signs ad-hoc. Move the old test bundle away first;
the script refuses to overwrite. Not yet done: a live look at the window and
a live recording. Drive code was not touched.

### Editor export panel (Claude, 23 October 2026)

Design: canvas row "Editor · export rechtsonder" (only the export panel was to be
built; the surrounding editor mock is reference only).

Implemented:
- `src/editor_export.rs`: the full-page export (`render_export_page`, preview
  pane, sidebar, overlay) is gone. `render_export_panel` draws a 416 px card in
  the editor's bottom-right: title + summary and the Bestand / Klembord / Drive
  switch fixed at the top, settings scrolling in the middle, the export button
  fixed at the bottom; while exporting and when done the settings are replaced
  by progress or the result (Link kopiëren / Openen / Bestand openen / Opnieuw
  kopiëren / Nieuwe export). `open_export` became `ensure_export` (created once
  the project is ready; no longer pauses playback). Starting an export pauses
  playback instead. `reset_export` returns to the settings after a result.
  Export strings are Dutch; the rest of the editor is still English.
- `src/editor_window.rs`: header Export button removed; the timeline row is now
  timeline + export panel; the slider drag layer moved to the editor root.
- `src/editor_timeline.rs`: `content_width`/`ruler_width` subtract the panel and
  its gutter so time maths match the narrower timeline.
- `src/app_windows.rs`: auto-export harness uses `ensure_export`.

Checks: cargo check clean (pre-existing warning only), full unit suite 1301
passed. Packaged again as `outputs/Cap Drive Linku-test.app` (Linku Loom Dev).
Not verified by hand yet: a real export to file, clipboard and Drive from the
panel. Known gaps: the size estimate refreshes when export settings change, not
after every timeline edit; the panel uses the editor's existing accent colour.

### Export panel v2 (Claude, 23 October 2026)

Design: canvas row "Export v2 · instellingen achter Aanpassen".

- The panel no longer scrolls settings. Under the destination switch it shows
  four chips (Formaat, Resolutie, Framerate, Kwaliteit). Each opens a quick
  `ui::Menu` anchored to the chip (`QuickSetting`, `QuickMenu`,
  `open_quick_setting`, `choose_quick_setting`, `render_export_quick_menu`).
  Quality shows "—" and is disabled for GIF/cursor-only; Format is disabled for
  transparent or cursor-only exports.
- "Alle instellingen" opens `render_export_settings_popover`, a 416 px card above
  the panel with every setting (the existing field builders), "Standaard
  herstellen" (MP4, 720p, 30 FPS, Maximum, no optimise) and "Klaar". Escape,
  the backdrop, the close button or Klaar close it; starting an export closes it.
- Both overlays are drawn from the editor root; arrow keys/Enter/Escape for the
  quick menu go through `export_quick_menu_key` in the editor key handler.

Checks: cargo check clean (pre-existing warning only), 1301 tests pass,
packaged again (Linku Loom Dev). Not hand-tested yet.

### Drive folder choice (Claude, 23 October 2026)

Design: canvas row "Drive · map kiezen of maken". Olivier chose the full
`drive` scope and will set the OAuth project to Internal for Linku, and a team
folder configured centrally with a per-user subfolder named after the Google
display name.

- `integrations/direct-drive/src/lib.rs`: scope `drive`; the granted scope is
  stored with the connection (`can_browse_folders`, cached); `list_folders`
  (My Drive, shared drives, a folder), `search_folders`, `create_folder`,
  `team_destination` (reads `CapDrive/drive-team.json` / `CAP_DRIVE_TEAM_FILE`,
  finds or creates `<team>/<display name>`), `upload` takes an optional folder
  id and supports shared drives. Insufficient-scope 403s ask to reconnect.
  12 library tests pass, strict Clippy clean.
- `src/direct_drive.rs`, `src/store.rs` (`drive_folder_id`, `drive_folder_path`
  in `ExportPrefs`), `src/editor_export.rs` (`DriveChoice`, `FolderPicker`,
  folder button in the panel, `render_drive_folder_picker`, team default),
  `src/editor_window.rs` (picker drawn from the editor root),
  `src/settings_pages.rs` (reconnect hint for old `drive.file` connections).
- Not verified against real Drive: browsing, search, folder creation, the team
  folder and an upload into a chosen or shared-drive folder all need Olivier to
  reconnect Drive first. No `drive-team.json` exists on this Mac yet.

### Export memory warning (Claude, 23 October 2026)

The "memory pressure" message before exports came from Cap's preflight check
(`crates/utils/src/export_resources.rs`), not from Cap's own use: measured peak
footprint was about 720 MB on a 24 GB Mac, while macOS reported pressure level
"warning" because of other open apps. macOS now warns only at "critical", like
other platforms; the in-export supervisor still stops on sustained critical
pressure. Test added; cap-utils export_resources tests pass (13). Rebuilt and
repackaged.

### Linku blue everywhere (Claude, 23 October 2026)

`#3b66c4` is now the accent in every window: `EditorPalette.accent`/`accent_2`
(light and dark), the whole `blue_*` scale in `Theme` (a Linku scale around
`#3b66c4`, light and dark), `blue_100`/`blue_500`, `SETTINGS_ACCENT`,
`TARGET_HIGHLIGHT`, the blue `ui::Button` variant, the timeline clip track
(`CLIP`) and the screenshot editor selection. Other track colours (orange,
pink, green...) are unchanged, pending the next colour exploration. Full suite
passes (1301); rebuilt and repackaged.

### Kiekes rebrand (Claude, 23 October 2026)

Name **Kiekes** ("een Nijmeegse opname-tool van Linku"). Mark from Olivier's
`LoomCloneLogo.svg`; app icon is the blue tile with the white mark; wordmark is
Sen Regular (canvas row "Kiekes · merk").

- Icons: `assets/branding/Kiekes.icns` and `kiekes-icon-1024.png` (rendered by a
  CoreGraphics script from the SVG path), `assets/dock-icon.png`, all
  `assets/tray/*` menu bar templates (mark at rest, rounded square while
  recording), `assets/icons/kiekes-mark.svg` (screenshot editor loader).
  `logo-full*.svg` (Cap wordmark) removed.
- Visible strings: "Cap" -> "Kiekes" in window titles, menus (`APP_NAME`), tray,
  onboarding (FAQ rewritten for Drive), permissions, settings, updates,
  recording names ("Kiekes Recording"), default Drive folder "Kiekes-opnames".
  Saved window-exclusion titles "Cap ..." are mapped to "Kiekes ..." on load
  (`renamed_own_window_title`, tested) so own windows stay out of recordings.
- Deliberately unchanged: process/installer names, tracing logs, file paths,
  auth headers, the `.cap` project format, and Cap service features (Cap Pro,
  license, classic app, Cap server/Web, CLI) - those settings pages still
  mention Cap and should be hidden in a follow-up.
- `FontWeight::BOLD` -> `SEMIBOLD` across the UI, except bold text segments a
  user styled on the timeline.
- Packaging (`work/package_cap_drive.py`): executable and bundle name `Kiekes`,
  Kiekes icon, usage strings, `NSHumanReadableCopyright` and `Credits.html`
  (About panel credits Cap + AGPLv3 + source link). Bundle ID stays
  `local.capdrive.desktop`; settings dir `CapDrive`, keychain service and
  `google-oauth.json` unchanged so permissions and Drive keep working.
- Built and packaged as `outputs/Kiekes.app` (Linku Loom Dev). 1302 app tests
  and 12 Drive tests pass. Not yet looked at by hand.

### Cap services hidden in Kiekes (Claude, 23 October 2026)

With `direct_drive::enabled()`: settings pages CLI, Plan & license and
Feedback are hidden (`Page::visible`; a link to one opens General), General
drops Updates, Server URL and the telemetry switch (the GPUI app sends no
telemetry), the settings sidebar drops the Cap profile and sign-in, and the
dev-only classic-app row stays off. The Changelog page (Cap's release
history) is hidden too and "Wat is er nieuw" is gone from the recorder's
"Meer" menu; hidden pages also skip their fetches (`pages_shown`). 1302 tests pass;
repackaged as `outputs/Kiekes.app`.

### Dock icon fix (Claude, 23 October 2026)

`platform::set_dock_icon` borrowed `/Applications/Cap.app`'s icon whenever Cap
was installed, so Kiekes showed Cap's Dock tile. It now asks `NSWorkspace` for
the running bundle's own icon (`NSBundle.mainBundle.bundlePath`) and only an
unbundled dev binary uses `assets/dock-icon.png`. Rebuilt and repackaged.
