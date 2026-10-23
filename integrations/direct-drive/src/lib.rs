use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

pub const CLIENT_ID: &str =
    "322331330903-m6mimqp7vedg8i2l070l2k3f5pkcsuma.apps.googleusercontent.com";
const SCOPE: &str = "https://www.googleapis.com/auth/drive";
const FOLDER_MIME: &str = "application/vnd.google-apps.folder";
const FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";
const SCOPE_MESSAGE: &str =
    "Reconnect Google Drive in Settings so the app can browse and choose folders.";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CHUNK_SIZE: usize = 8 * 1024 * 1024;
const CANCELLED: &str = "Export cancelled";
static CONNECTED: OnceLock<AtomicBool> = OnceLock::new();
static FULL_ACCESS: OnceLock<AtomicBool> = OnceLock::new();

pub type Result<T> = std::result::Result<T, String>;

#[derive(Deserialize)]
struct OAuthFile {
    installed: OAuthClient,
}

#[derive(Deserialize)]
struct OAuthClient {
    client_id: String,
    client_secret: String,
}

#[derive(Serialize, Deserialize)]
struct Connection {
    refresh_token: String,
    folder_id: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct DriveFile {
    id: String,
    size: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriveFolder {
    pub id: String,
    pub name: String,
}

/// A folder together with the names leading to it, for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveDestination {
    pub folder: DriveFolder,
    pub path: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderParent {
    MyDrive,
    SharedDrives,
    Folder(String),
}

#[derive(Deserialize)]
struct FolderList {
    #[serde(default)]
    files: Vec<DriveFolder>,
}

#[derive(Deserialize)]
struct SharedDriveList {
    #[serde(default)]
    drives: Vec<DriveFolder>,
}

#[derive(Deserialize)]
struct About {
    user: AboutUser,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AboutUser {
    display_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamConfig {
    team_folder_id: String,
}

pub struct Authorization {
    pub url: String,
    listener: TcpListener,
    redirect: String,
    verifier: String,
    state: String,
    client_secret: Option<String>,
}

fn client() -> Result<Client> {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Could not create the Google Drive connection".into())
}

fn credential() -> Result<keyring::Entry> {
    keyring::Entry::new("CapDrive.Desktop", CLIENT_ID)
        .map_err(|_| "Could not open the system credential store".into())
}

fn connection() -> Result<Option<Connection>> {
    match credential()?.get_password() {
        Ok(value) => serde_json::from_str(&value).map(Some).map_err(|_| {
            "The Google Drive connection is invalid. Disconnect and reconnect.".into()
        }),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(_) => {
            Err("Could not read Google Drive credentials from the system credential store".into())
        }
    }
}

fn save_connection(connection: &Connection) -> Result<()> {
    let value =
        serde_json::to_string(connection).map_err(|_| "Could not save Google connection")?;
    credential()?
        .set_password(&value)
        .map_err(|_| "Could not save Google Drive credentials to the system credential store")?;
    if let Some(connected) = CONNECTED.get() {
        connected.store(true, Ordering::Release);
    }
    if let Some(full) = FULL_ACCESS.get() {
        full.store(
            connection.scope.as_deref().is_some_and(grants_full_drive),
            Ordering::Release,
        );
    }
    Ok(())
}

pub fn is_connected() -> bool {
    CONNECTED
        .get_or_init(|| AtomicBool::new(connection().ok().flatten().is_some()))
        .load(Ordering::Acquire)
}

/// Whether the stored connection was granted full Drive access. Connections
/// made before folder choice existed only hold `drive.file` and must reconnect.
pub fn can_browse_folders() -> bool {
    FULL_ACCESS
        .get_or_init(|| {
            AtomicBool::new(
                connection()
                    .ok()
                    .flatten()
                    .and_then(|connection| connection.scope)
                    .is_some_and(|scope| grants_full_drive(&scope)),
            )
        })
        .load(Ordering::Acquire)
}

fn grants_full_drive(scope: &str) -> bool {
    scope.split_whitespace().any(|granted| granted == SCOPE)
}

fn team_config_path() -> Option<PathBuf> {
    std::env::var_os("CAP_DRIVE_TEAM_FILE")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|path| path.join("CapDrive/drive-team.json")))
}

fn parse_team_config(text: &str) -> Result<Option<String>> {
    let config: TeamConfig = serde_json::from_str(text)
        .map_err(|_| "The Drive team configuration must contain a teamFolderId")?;
    let id = config.team_folder_id.trim().to_string();
    Ok((!id.is_empty()).then_some(id))
}

fn team_folder_id() -> Result<Option<String>> {
    let Some(path) = team_config_path() else {
        return Ok(None);
    };
    match std::fs::read_to_string(path) {
        Ok(text) => parse_team_config(&text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err("Could not read the Drive team configuration".into()),
    }
}

pub fn has_team_folder() -> bool {
    team_folder_id().ok().flatten().is_some()
}

pub fn disconnect() -> Result<()> {
    match credential()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {
            if let Some(connected) = CONNECTED.get() {
                connected.store(false, Ordering::Release);
            }
            if let Some(full) = FULL_ACCESS.get() {
                full.store(false, Ordering::Release);
            }
            Ok(())
        }
        Err(_) => Err("Could not remove Google Drive credentials".into()),
    }
}

fn client_secret() -> Result<Option<String>> {
    let path = std::env::var_os("CAP_DRIVE_OAUTH_FILE")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::config_dir().map(|path| path.join("CapDrive/google-oauth.json")));
    let Some(path) = path else { return Ok(None) };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("Could not read the local Google OAuth configuration".into()),
    };
    let config: OAuthFile = serde_json::from_str(&text)
        .map_err(|_| "Use the downloaded Google OAuth JSON for a Desktop app client")?;
    if config.installed.client_id != CLIENT_ID {
        return Err("The local OAuth configuration belongs to a different Google client".into());
    }
    Ok(Some(config.installed.client_secret))
}

fn random_string() -> Result<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "Could not start secure Google sign-in")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub async fn begin_authorization() -> Result<Authorization> {
    let client_secret = client_secret()?;
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|_| "Could not start the local Google sign-in callback")?;
    let port = listener
        .local_addr()
        .map_err(|_| "Could not get the callback address")?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/oauth/callback");
    let verifier = random_string()?;
    let state = random_string()?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    let mut url = Url::parse("https://accounts.google.com/o/oauth2/v2/auth")
        .map_err(|_| "Invalid Google URL")?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", CLIENT_ID),
        ("redirect_uri", &redirect),
        ("response_type", "code"),
        ("scope", SCOPE),
        ("state", &state),
        ("code_challenge", &challenge),
        ("code_challenge_method", "S256"),
        ("access_type", "offline"),
        ("prompt", "consent"),
    ]);
    Ok(Authorization {
        url: url.into(),
        listener,
        redirect,
        verifier,
        state,
        client_secret,
    })
}

fn callback_code(request: &str, state: &str) -> Result<String> {
    let mut parts = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();
    if parts.next() != Some("GET") {
        return Err("Invalid Google sign-in callback".into());
    }
    let target = parts
        .next()
        .filter(|target| target.starts_with('/') && !target.starts_with("//"))
        .ok_or("Invalid Google sign-in callback")?;
    let url = Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|_| "Invalid Google sign-in callback")?;
    if url.path() != "/oauth/callback" {
        return Err("Invalid Google callback path".into());
    }
    let params: Vec<_> = url.query_pairs().collect();
    let values = |key| {
        params
            .iter()
            .filter(move |(name, _)| name == key)
            .map(|(_, value)| value.as_ref())
            .collect::<Vec<_>>()
    };
    if values("state") != [state] {
        return Err("Google sign-in verification failed".into());
    }
    if !values("error").is_empty() {
        return Err("Google Drive access was not granted".into());
    }
    let codes = values("code");
    match codes.as_slice() {
        [code] if !code.is_empty() => Ok((*code).to_string()),
        _ => Err("Google did not return an authorization code".into()),
    }
}

async fn cancellable<T>(
    cancel: &AtomicBool,
    future: impl std::future::Future<Output = Result<T>>,
) -> Result<T> {
    tokio::pin!(future);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(CANCELLED.into());
        }
        tokio::select! {
            result = &mut future => return result,
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}

async fn token_response(response: reqwest::Response) -> Result<Token> {
    if !response.status().is_success() {
        let status = response.status();
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        return match body.get("error").and_then(|error| error.as_str()) {
            Some("invalid_grant") => Err("Google authorization expired. Disconnect and reconnect Google Drive in Settings.".into()),
            Some("invalid_client") => Err("Google rejected this OAuth client. Confirm it is a Desktop app and place its downloaded JSON in CapDrive/google-oauth.json in your application settings folder.".into()),
            _ => Err(format!("Google sign-in failed ({status}). Check the Desktop OAuth client configuration and Drive API access.")),
        };
    }
    response
        .json()
        .await
        .map_err(|_| "Google returned an invalid sign-in response".into())
}

impl Authorization {
    pub async fn complete(self, cancel: &AtomicBool) -> Result<()> {
        cancellable(cancel, async {
            let deadline = Instant::now() + Duration::from_secs(300);
            let code = loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let (mut socket, _) = tokio::time::timeout(remaining, self.listener.accept()).await
                    .map_err(|_| "Google sign-in timed out. Try again.")?
                    .map_err(|_| "Could not receive Google sign-in")?;
                let mut request = Vec::new();
                let read = tokio::time::timeout(Duration::from_secs(3), async {
                    let mut byte = [0_u8; 1];
                    while request.len() < 8192 && !request.ends_with(b"\r\n\r\n") {
                        if socket.read(&mut byte).await.map_err(|_| "Could not read Google callback")? == 0 { break }
                        request.push(byte[0]);
                    }
                    Ok::<_, String>(())
                }).await;
                if !matches!(read, Ok(Ok(()))) { continue }
                let result = callback_code(&String::from_utf8_lossy(&request), &self.state);
                let (status, message) = if result.is_ok() {
                    ("200 OK", "Google responded. Return to the app to finish connecting.")
                } else {
                    ("400 Bad Request", "Sign-in could not be verified. Return to the app or retry Google sign-in.")
                };
                let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{message}", message.len());
                let _ = socket.write_all(response.as_bytes()).await;
                match result {
                    Ok(code) => break code,
                    Err(error) if error == "Google Drive access was not granted" => return Err(error),
                    Err(_) => continue,
                }
            };
            let mut form = vec![("client_id", CLIENT_ID), ("code", &code), ("redirect_uri", &self.redirect),
                ("code_verifier", &self.verifier), ("grant_type", "authorization_code")];
            if let Some(secret) = &self.client_secret { form.push(("client_secret", secret)); }
            let response = client()?.post(TOKEN_URL).form(&form).send().await
                .map_err(|_| "Could not contact Google sign-in")?;
            let token = token_response(response).await?;
            save_connection(&Connection {
                refresh_token: token.refresh_token.ok_or("Google did not grant offline access. Reconnect and approve access.")?,
                folder_id: None,
                scope: token.scope,
            })
        }).await
    }
}

async fn access_token(client: &Client, connection: &Connection) -> Result<String> {
    let secret = client_secret()?;
    let mut form = vec![
        ("client_id", CLIENT_ID),
        ("refresh_token", &connection.refresh_token),
        ("grant_type", "refresh_token"),
    ];
    if let Some(secret) = &secret {
        form.push(("client_secret", secret));
    }
    let response = client
        .post(TOKEN_URL)
        .form(&form)
        .send()
        .await
        .map_err(|_| "Could not refresh Google Drive sign-in")?;
    Ok(token_response(response).await?.access_token)
}

async fn drive_response(response: reqwest::Response) -> Result<reqwest::Response> {
    if response.status().is_success() {
        return Ok(response);
    }
    let status = response.status();
    if status == StatusCode::FORBIDDEN {
        let body: serde_json::Value = response.json().await.unwrap_or_default();
        if scope_insufficient(&body) {
            return Err(SCOPE_MESSAGE.into());
        }
    }
    Err(match status {
        StatusCode::UNAUTHORIZED => "Google sign-in expired. Reconnect Google Drive and retry.".into(),
        StatusCode::FORBIDDEN => "Google Drive denied the upload. Check Drive quota, permissions, and whether the Drive API is enabled.".into(),
        StatusCode::NOT_FOUND => "The Drive destination no longer exists. Disconnect and reconnect to create a new folder.".into(),
        _ => format!("Google Drive request failed ({status}). Your local recording is still saved."),
    })
}

fn scope_insufficient(body: &serde_json::Value) -> bool {
    let error = &body["error"];
    error["status"] == "PERMISSION_DENIED"
        && error["details"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|detail| detail["reason"] == "ACCESS_TOKEN_SCOPE_INSUFFICIENT")
        || error["errors"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|entry| entry["reason"] == "insufficientPermissions")
}

fn quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn folder_query(
    parent: Option<&str>,
    name_contains: Option<&str>,
    name_is: Option<&str>,
) -> String {
    let mut query = format!("mimeType = '{FOLDER_MIME}' and trashed = false");
    if let Some(parent) = parent {
        query.push_str(&format!(" and '{}' in parents", quote(parent)));
    }
    if let Some(name) = name_contains {
        query.push_str(&format!(" and name contains '{}'", quote(name)));
    }
    if let Some(name) = name_is {
        query.push_str(&format!(" and name = '{}'", quote(name)));
    }
    query
}

async fn authorized() -> Result<(Client, String)> {
    let client = client()?;
    let connection = connection()?.ok_or("Connect Google Drive first")?;
    let token = access_token(&client, &connection).await?;
    Ok((client, token))
}

async fn find_folders(client: &Client, token: &str, query: &str) -> Result<Vec<DriveFolder>> {
    let response = client
        .get(FILES_URL)
        .bearer_auth(token)
        .query(&[
            ("q", query),
            ("fields", "files(id,name)"),
            ("orderBy", "name_natural"),
            ("pageSize", "200"),
            ("supportsAllDrives", "true"),
            ("includeItemsFromAllDrives", "true"),
            ("corpora", "allDrives"),
        ])
        .send()
        .await
        .map_err(|_| "Could not load Drive folders")?;
    let list: FolderList = drive_response(response)
        .await?
        .json()
        .await
        .map_err(|_| "Could not read Drive folders")?;
    Ok(list.files)
}

async fn make_folder(
    client: &Client,
    token: &str,
    parent: &str,
    name: &str,
) -> Result<DriveFolder> {
    let response = client
        .post(FILES_URL)
        .bearer_auth(token)
        .query(&[("fields", "id,name"), ("supportsAllDrives", "true")])
        .json(&json!({"name": name, "mimeType": FOLDER_MIME, "parents": [parent]}))
        .send()
        .await
        .map_err(|_| "Could not create the Drive folder")?;
    drive_response(response)
        .await?
        .json()
        .await
        .map_err(|_| "Could not read the new Drive folder".into())
}

/// The folders directly inside `parent`. For [`FolderParent::SharedDrives`]
/// these are the shared drives themselves, whose ids work as folder ids.
pub async fn list_folders(parent: FolderParent) -> Result<Vec<DriveFolder>> {
    let (client, token) = authorized().await?;
    match parent {
        FolderParent::SharedDrives => {
            let response = client
                .get("https://www.googleapis.com/drive/v3/drives")
                .bearer_auth(&token)
                .query(&[("fields", "drives(id,name)"), ("pageSize", "100")])
                .send()
                .await
                .map_err(|_| "Could not load shared drives")?;
            let list: SharedDriveList = drive_response(response)
                .await?
                .json()
                .await
                .map_err(|_| "Could not read shared drives")?;
            Ok(list.drives)
        }
        FolderParent::MyDrive => {
            find_folders(&client, &token, &folder_query(Some("root"), None, None)).await
        }
        FolderParent::Folder(id) => {
            find_folders(&client, &token, &folder_query(Some(&id), None, None)).await
        }
    }
}

pub async fn search_folders(text: &str) -> Result<Vec<DriveFolder>> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let (client, token) = authorized().await?;
    find_folders(&client, &token, &folder_query(None, Some(text), None)).await
}

/// `parent` is a folder id, a shared drive id, or `root` for My Drive.
pub async fn create_folder(parent: &str, name: &str) -> Result<DriveFolder> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Give the folder a name".into());
    }
    let (client, token) = authorized().await?;
    make_folder(&client, &token, parent, name).await
}

/// The configured team folder with a subfolder named after the signed-in
/// Google account inside it, created on first use. `None` without a team
/// configuration.
pub async fn team_destination() -> Result<Option<DriveDestination>> {
    let Some(team_id) = team_folder_id()? else {
        return Ok(None);
    };
    let (client, token) = authorized().await?;
    let response = client
        .get(format!("{FILES_URL}/{team_id}"))
        .bearer_auth(&token)
        .query(&[("fields", "id,name"), ("supportsAllDrives", "true")])
        .send()
        .await
        .map_err(|_| "Could not open the team folder in Drive")?;
    let team: DriveFolder = drive_response(response)
        .await?
        .json()
        .await
        .map_err(|_| "Could not read the team folder")?;
    let response = client
        .get("https://www.googleapis.com/drive/v3/about")
        .bearer_auth(&token)
        .query(&[("fields", "user(displayName)")])
        .send()
        .await
        .map_err(|_| "Could not read your Google account name")?;
    let about: About = drive_response(response)
        .await?
        .json()
        .await
        .map_err(|_| "Could not read your Google account name")?;
    let name = about.user.display_name.trim().to_string();
    if name.is_empty() {
        return Err("Your Google account has no name to use for the folder".into());
    }
    let existing = find_folders(
        &client,
        &token,
        &folder_query(Some(&team.id), None, Some(&name)),
    )
    .await?
    .into_iter()
    .next();
    let folder = match existing {
        Some(folder) => folder,
        None => make_folder(&client, &token, &team.id, &name).await?,
    };
    Ok(Some(DriveDestination {
        path: vec![team.name, folder.name.clone()],
        folder,
    }))
}

fn session_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| "Google returned an invalid upload address")?;
    if url.scheme() != "https"
        || url.host_str() != Some("www.googleapis.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port_or_known_default() != Some(443)
        || url.path() != "/upload/drive/v3/files"
    {
        return Err("Google returned an unexpected upload address".into());
    }
    Ok(url)
}

fn resume_offset(response: &reqwest::Response, total: u64) -> Result<u64> {
    let Some(value) = response.headers().get("Range") else {
        return Ok(0);
    };
    let value = value
        .to_str()
        .map_err(|_| "Invalid Drive upload progress")?;
    let end: u64 = value
        .strip_prefix("bytes=0-")
        .ok_or("Invalid Drive upload progress")?
        .parse()
        .map_err(|_| "Invalid Drive upload progress")?;
    let offset = end
        .checked_add(1)
        .filter(|offset| *offset < total)
        .ok_or("Invalid Drive upload progress")?;
    Ok(offset)
}

async fn upload_chunks(
    client: &Client,
    url: Url,
    token: &str,
    path: &Path,
    total: u64,
    progress: impl Fn(f64),
) -> Result<String> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|_| "Could not open the exported video")?;
    let mut offset = 0;
    let mut failures = 0;
    let mut buffer = vec![0_u8; CHUNK_SIZE];
    progress(0.0);
    loop {
        file.seek(std::io::SeekFrom::Start(offset))
            .await
            .map_err(|_| "Could not read the exported video")?;
        let length = (total - offset).min(CHUNK_SIZE as u64) as usize;
        file.read_exact(&mut buffer[..length])
            .await
            .map_err(|_| "The exported video changed during upload")?;
        let end = offset + length as u64;
        let sent = client
            .put(url.clone())
            .bearer_auth(token)
            .header("Content-Type", "video/mp4")
            .header(
                "Content-Range",
                format!("bytes {offset}-{}/{total}", end - 1),
            )
            .body(buffer[..length].to_vec())
            .send()
            .await;
        let response = match sent {
            Ok(response)
                if response.status() == StatusCode::PERMANENT_REDIRECT
                    || response.status().is_success() =>
            {
                response
            }
            Ok(response)
                if !response.status().is_server_error()
                    && response.status() != StatusCode::TOO_MANY_REQUESTS =>
            {
                drive_response(response).await?;
                unreachable!()
            }
            _ => loop {
                failures += 1;
                if failures > 5 {
                    return Err("Google Drive upload was interrupted. Retry; your local recording is saved.".into());
                }
                tokio::time::sleep(Duration::from_secs(1 << failures)).await;
                match client
                    .put(url.clone())
                    .bearer_auth(token)
                    .header("Content-Range", format!("bytes */{total}"))
                    .header("Content-Length", 0)
                    .send()
                    .await
                {
                    Ok(response)
                        if response.status() == StatusCode::PERMANENT_REDIRECT
                            || response.status().is_success() =>
                    {
                        break response;
                    }
                    Ok(response)
                        if !response.status().is_server_error()
                            && response.status() != StatusCode::TOO_MANY_REQUESTS =>
                    {
                        drive_response(response).await?;
                        unreachable!()
                    }
                    _ => continue,
                }
            },
        };
        if response.status() == StatusCode::PERMANENT_REDIRECT {
            let next = resume_offset(&response, total)?;
            if next < offset || next > end {
                return Err("Google returned inconsistent upload progress".into());
            }
            if next == offset {
                failures += 1;
                if failures > 5 {
                    return Err(
                        "Google Drive stopped accepting video data. Retry the upload.".into(),
                    );
                }
            } else {
                failures = 0;
            }
            offset = next;
            progress(offset as f64 / total as f64);
            continue;
        }
        let uploaded: DriveFile = response
            .json()
            .await
            .map_err(|_| "Could not verify the uploaded video")?;
        if uploaded.id.is_empty() || uploaded.size.as_deref() != Some(&total.to_string()) {
            return Err(
                "Google did not confirm the complete video. Check Drive before retrying.".into(),
            );
        }
        progress(1.0);
        return Ok(format!(
            "https://drive.google.com/file/d/{}/view",
            uploaded.id
        ));
    }
}

#[cfg(test)]
mod tests;

/// Uploads into `folder_id` when given, otherwise into the app's own
/// `Kiekes-opnames` folder.
pub async fn upload(
    path: &Path,
    name: &str,
    folder_id: Option<&str>,
    progress: impl Fn(f64),
    cancel: &AtomicBool,
) -> Result<String> {
    cancellable(cancel, async {
        let total = tokio::fs::metadata(path).await.map_err(|_| "Exported video not found")?.len();
        if total == 0 { return Err("Cannot upload an empty video".into()) }
        let client = client()?;
        let mut connection = connection()?.ok_or("Connect Google Drive before uploading")?;
        let token = access_token(&client, &connection).await?;
        let folder = if let Some(id) = folder_id { id.to_string() } else if let Some(id) = &connection.folder_id { id.clone() } else {
            let response = client.post("https://www.googleapis.com/drive/v3/files")
                .bearer_auth(&token).query(&[("fields", "id")])
                .json(&json!({"name": "Kiekes-opnames", "mimeType": "application/vnd.google-apps.folder"}))
                .send().await.map_err(|_| "Could not create the recordings folder in Drive")?;
            let folder: DriveFile = drive_response(response).await?.json().await.map_err(|_| "Could not read the Drive folder")?;
            connection.folder_id = Some(folder.id.clone());
            save_connection(&connection)?;
            folder.id
        };
        let name = if name.ends_with(".mp4") { name.to_string() } else { format!("{name}.mp4") };
        let response = client.post("https://www.googleapis.com/upload/drive/v3/files")
            .bearer_auth(&token).query(&[("uploadType", "resumable"), ("fields", "id,size"), ("supportsAllDrives", "true")])
            .header("X-Upload-Content-Type", "video/mp4")
            .header("X-Upload-Content-Length", total)
            .json(&json!({"name": name, "parents": [folder], "mimeType": "video/mp4"}))
            .send().await.map_err(|_| "Could not start the Google Drive upload")?;
        let response = drive_response(response).await?;
        let url = session_url(response.headers().get("Location").and_then(|value| value.to_str().ok())
            .ok_or("Google did not return an upload address")?)?;
        upload_chunks(&client, url, &token, path, total, progress).await
    }).await
}
