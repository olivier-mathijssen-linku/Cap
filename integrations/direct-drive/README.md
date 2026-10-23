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

The app requests only `drive.file`. Refresh tokens are stored in the operating
system credential store, not in the repository or Cap's server. The app creates
a private `Cap Recordings` folder and never creates public sharing permissions.
Changing sharing on that folder in Drive can affect its children's access.
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

A real Google authorization, a sample video upload, playback, and a viewer
access check must still be verified with the configured Google project.

Validation: all eight Drive library tests and its strict Clippy check pass.
After installing full Xcode, accepting its license, and installing CMake, the
GPUI desktop binary builds successfully. A local macOS 27 Apple Silicon app
bundle with the recording helper, video libraries, backgrounds, and audio assets
has been signed locally, verified, and opened to the onboarding screen.
The local prototype does not bundle Cap's CLI diagnostics sidecar. Screen
Recording and Accessibility permissions, Google consent, and a real recording
and upload still require verification before calling the workflow complete.

The source retains Cap's AGPLv3 license and copyright notices.
