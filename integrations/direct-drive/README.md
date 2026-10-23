# Direct Google Drive uploads

This fork's GPUI desktop app enables the `direct-drive` feature by default.
Studio recordings are exported locally and uploaded directly to Google Drive.
The editor copies a standard `drive.google.com/file/d/.../view` link after
Google confirms the uploaded byte count. Each upload creates a new video.
Existing Cap links are never overwritten by this path.

## Connect Google

1. Enable the Google Drive API in the project owning the OAuth client.
2. Use a **Desktop app** OAuth client. If the consent screen is in testing,
   add your Google account as a test user.
3. Download the client JSON from Google Auth Platform > Clients. Store it at
   `~/Library/Application Support/CapDrive/google-oauth.json` on macOS, or set
   `CAP_DRIVE_OAUTH_FILE` to its absolute path before starting the app. Do not
   commit this file. Its client ID must match `CLIENT_ID` in this library.
4. Open Settings > Integrations > Connect Google Drive and complete Google's
   browser authorization.
5. Record in Studio Mode. In the editor's export panel select Google Drive
   and click Upload to Google Drive.
6. Open the copied link and use Google's Share dialog to give viewers access.

The app requests the full `drive` scope so people can browse My Drive and
shared drives, pick any folder and create project folders. Google treats this
as a restricted scope: keep the OAuth consent screen on **Internal** for the
Linku Workspace, which avoids Google's verification but limits sign-in to Linku
accounts. Connections made with the older `drive.file` scope keep uploading to
their existing `Cap Recordings` folder but must disconnect and reconnect before folders can be chosen.
Refresh tokens are stored in the operating system credential store, not in the
repository or Cap's server. The app never creates public sharing permissions.
Changing sharing on a folder in Drive can affect its children's access.

### Team folder

Place `drive-team.json` next to `google-oauth.json` (or point
`CAP_DRIVE_TEAM_FILE` at it):

```json
{ "teamFolderId": "<Drive folder id, for example from a shared drive>" }
```

Everyone's default upload folder is then `<team folder>/<Google display name>`,
created on first use. Without this file the default is a private `Kiekes-opnames` folder (connections made earlier keep their existing `Cap Recordings` folder).
The folder chosen in the export panel is remembered per user.
Disconnect removes the local credential; it does not delete recordings or revoke
Google's grant. Grants can also be revoked in your Google account settings.

## Fork behavior

- Studio Mode and local video editing are retained. Instant Mode is hidden and
  blocked because it depends on Cap's streaming server workflow.
- Direct Drive sharing bypasses Cap authentication, subscription checks, video
  creation, thumbnail uploads, and Cap share pages.
- The app uses a separate `CapDrive` settings directory and disables Cap's
  update mechanisms so an upstream binary cannot replace this fork.
- Google Drive processes playback after upload. The viewing link may be available
  before its preview is ready.
- Uploads use aligned 8 MiB chunks and query Drive's received-byte offset after
  transient failures. Cancellation leaves the local recording intact. Upload
  sessions are not persisted across app restarts; a manual retry creates a new
  file and may leave a previous completed upload in Drive.
- This is a video sharing integration. Screenshots and the rest of Cap's cloud
  account features have not been converted to Google Drive.
- This change does not yet rename the app's bundle or replace its visual branding.

## Build and verification

Prepare Cap's native dependencies using its existing setup scripts, including
`scripts/prepare-gpui-dependency.sh`, then build the GPUI app from
`apps/desktop-gpui`. `--no-default-features` retains upstream cloud behavior.

The Drive library has an independent workspace so it can be checked without
building the recorder and GPU stack:

```sh
cargo test --manifest-path integrations/direct-drive/Cargo.toml
cargo clippy --manifest-path integrations/direct-drive/Cargo.toml --all-targets -- -D warnings
```

Tests cover callback state/path validation, upload URL validation, bounded
resume offsets, chunk contents, completion size verification, recovery after
a lost final response, and cancellation. They use a local HTTP test server and
do not contact Google or access the system credential store.

Google authorization has been completed with the configured Desktop client.
A generated three-second, silent test clip was imported into the packaged
app, exported at 720p/30 fps, uploaded to Drive, and played from the returned
standard Drive link. Drive showed the file as private to its owner. Access
by an additional invited viewer has not been tested.

Validation: all eight Drive library tests and its strict Clippy check pass.
After installing full Xcode, accepting its license, and installing CMake, the
GPUI desktop binary builds successfully. A local macOS 27 Apple Silicon app
bundle with the recording helper, video libraries, backgrounds, and audio assets
has been signed locally, verified, and opened to the onboarding screen.
The local prototype does not bundle Cap's CLI diagnostics sidecar. Screen
Recording and Accessibility permissions have been granted and onboarding
completed. A live screen-capture session has not yet been tested; the verified
flow used an imported generated clip and did not capture screen or microphone
content.

The source retains Cap's AGPLv3 license and copyright notices.
