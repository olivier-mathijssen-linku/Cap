use std::path::Path;
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
const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const CHUNK_SIZE: usize = 8 * 1024 * 1024;
const CANCELLED: &str = "Export cancelled";
static CONNECTED: OnceLock<AtomicBool> = OnceLock::new();

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
}

#[derive(Deserialize)]
struct Token {
    access_token: String,
    refresh_token: Option<String>,
}

#[derive(Deserialize)]
struct DriveFile {
    id: String,
    size: Option<String>,
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
    Ok(())
}

pub fn is_connected() -> bool {
    CONNECTED
        .get_or_init(|| AtomicBool::new(connection().ok().flatten().is_some()))
        .load(Ordering::Acquire)
}

pub fn disconnect() -> Result<()> {
    match credential()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => {
            if let Some(connected) = CONNECTED.get() {
                connected.store(false, Ordering::Release);
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
    Err(match status {
        StatusCode::UNAUTHORIZED => "Google sign-in expired. Reconnect Google Drive and retry.".into(),
        StatusCode::FORBIDDEN => "Google Drive denied the upload. Check Drive quota, permissions, and whether the Drive API is enabled.".into(),
        StatusCode::NOT_FOUND => "The Drive destination no longer exists. Disconnect and reconnect to create a new folder.".into(),
        _ => format!("Google Drive request failed ({status}). Your local recording is still saved."),
    })
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

pub async fn upload(
    path: &Path,
    name: &str,
    progress: impl Fn(f64),
    cancel: &AtomicBool,
) -> Result<String> {
    cancellable(cancel, async {
        let total = tokio::fs::metadata(path).await.map_err(|_| "Exported video not found")?.len();
        if total == 0 { return Err("Cannot upload an empty video".into()) }
        let client = client()?;
        let mut connection = connection()?.ok_or("Connect Google Drive before uploading")?;
        let token = access_token(&client, &connection).await?;
        let folder = if let Some(id) = &connection.folder_id { id.clone() } else {
            let response = client.post("https://www.googleapis.com/drive/v3/files")
                .bearer_auth(&token).query(&[("fields", "id")])
                .json(&json!({"name": "Cap Recordings", "mimeType": "application/vnd.google-apps.folder"}))
                .send().await.map_err(|_| "Could not create the recordings folder in Drive")?;
            let folder: DriveFile = drive_response(response).await?.json().await.map_err(|_| "Could not read the Drive folder")?;
            connection.folder_id = Some(folder.id.clone());
            save_connection(&connection)?;
            folder.id
        };
        let name = if name.ends_with(".mp4") { name.to_string() } else { format!("{name}.mp4") };
        let response = client.post("https://www.googleapis.com/upload/drive/v3/files")
            .bearer_auth(&token).query(&[("uploadType", "resumable"), ("fields", "id,size")])
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
