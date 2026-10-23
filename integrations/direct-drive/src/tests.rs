use super::*;

#[test]
fn callback_requires_expected_path_state_and_one_code() {
    assert_eq!(
        callback_code(
            "GET /oauth/callback?state=secret&code=a%2Bb HTTP/1.1\r\n",
            "secret"
        )
        .unwrap(),
        "a+b"
    );
    for target in [
        "/oauth/callback?state=wrong&code=x",
        "/oauth/callback?code=x",
        "/oauth/callback?state=secret&state=wrong&code=x",
        "/oauth/callback?state=secret&code=x&code=y",
        "/oauth/callback?state=secret&error=access_denied",
        "/oauth/callback?state=secret&code=",
        "/other?state=secret&code=x",
        "//other/oauth/callback?state=secret&code=x",
        "http://other/oauth/callback?state=secret&code=x",
    ] {
        assert!(
            callback_code(&format!("GET {target} HTTP/1.1\r\n"), "secret").is_err(),
            "{target}"
        );
    }
    assert!(
        callback_code(
            "POST /oauth/callback?state=secret&code=x HTTP/1.1",
            "secret"
        )
        .is_err()
    );
}

#[test]
fn upload_session_cannot_send_tokens_to_another_host() {
    assert!(session_url("https://www.googleapis.com/upload/drive/v3/files?upload_id=abc").is_ok());
    for url in [
        "http://www.googleapis.com/upload/drive/v3/files",
        "https://www.googleapis.com.evil.invalid/upload/drive/v3/files",
        "https://evil.invalid/upload/drive/v3/files",
        "https://www.googleapis.com:8443/upload/drive/v3/files",
        "https://user@www.googleapis.com/upload/drive/v3/files",
        "https://www.googleapis.com/other",
        "https://www.googleapis.com/upload/drive/v3/files-other",
    ] {
        assert!(session_url(url).is_err(), "{url}");
    }
}

#[test]
fn drive_progress_is_bounded_and_unambiguous() {
    let response = |range: Option<&str>| http_response(range);
    assert_eq!(
        resume_offset(&response(Some("bytes=0-255")), 512).unwrap(),
        256
    );
    assert_eq!(resume_offset(&response(None), 512).unwrap(), 0);
    for range in [
        "bytes=1-255",
        "bytes=0-511",
        "bytes=0-1000",
        "bytes=0-18446744073709551615",
        "garbage",
    ] {
        assert!(resume_offset(&response(Some(range)), 512).is_err());
    }
}

fn http_response(range: Option<&str>) -> reqwest::Response {
    let mut response = http::Response::builder().status(308);
    if let Some(range) = range {
        response = response.header("Range", range);
    }
    response.body("").unwrap().into()
}

async fn serve(
    responses: Vec<Option<String>>,
) -> (Url, tokio::task::JoinHandle<Vec<(String, Vec<u8>)>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = Url::parse(&format!("http://{}/upload", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut header = Vec::new();
            let mut byte = [0_u8; 1];
            while !header.ends_with(b"\r\n\r\n") {
                socket.read_exact(&mut byte).await.unwrap();
                header.push(byte[0]);
            }
            let header = String::from_utf8(header).unwrap();
            let length: usize = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap_or(0);
            let mut body = vec![0; length];
            socket.read_exact(&mut body).await.unwrap();
            requests.push((header.to_lowercase(), body));
            if let Some(response) = response {
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        }
        requests
    });
    (url, task)
}

fn complete_response(size: u64) -> String {
    let body = json!({"id": "drive-file-id", "size": size.to_string()}).to_string();
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[tokio::test]
async fn uploads_aligned_chunks_and_returns_a_normal_drive_link() {
    let bytes = vec![42; CHUNK_SIZE + 123];
    let video = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(video.path(), &bytes).unwrap();
    let (url, requests) = serve(vec![
        Some(format!("HTTP/1.1 308 Resume Incomplete\r\nRange: bytes=0-{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", CHUNK_SIZE - 1)),
        Some(complete_response(bytes.len() as u64)),
    ]).await;
    let progress = std::sync::Mutex::new(Vec::new());
    let link = upload_chunks(
        &client().unwrap(),
        url,
        "test-token",
        video.path(),
        bytes.len() as u64,
        |value| progress.lock().unwrap().push(value),
    )
    .await
    .unwrap();
    assert_eq!(link, "https://drive.google.com/file/d/drive-file-id/view");
    let requests = requests.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(
        [requests[0].1.as_slice(), requests[1].1.as_slice()].concat(),
        bytes
    );
    assert!(requests[0].0.contains(&format!(
        "content-range: bytes 0-{}/{}",
        CHUNK_SIZE - 1,
        bytes.len()
    )));
    assert!(requests[1].0.contains(&format!(
        "content-range: bytes {}-{}/{}",
        CHUNK_SIZE,
        bytes.len() - 1,
        bytes.len()
    )));
    assert_eq!(progress.lock().unwrap().last(), Some(&1.0));
    assert!(
        requests
            .iter()
            .all(|(headers, _)| headers.contains("authorization: bearer test-token"))
    );
}

#[tokio::test]
async fn probes_server_after_lost_completion_response() {
    let video = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(video.path(), [1, 2, 3]).unwrap();
    let (url, requests) = serve(vec![None, Some(complete_response(3))]).await;
    let link = upload_chunks(&client().unwrap(), url, "token", video.path(), 3, |_| {})
        .await
        .unwrap();
    assert!(link.ends_with("/view"));
    let requests = requests.await.unwrap();
    assert_eq!(requests[0].1, [1, 2, 3]);
    assert!(requests[1].1.is_empty());
    assert!(requests[1].0.contains("content-range: bytes */3"));
}

#[tokio::test]
async fn refuses_to_report_success_for_truncated_uploads() {
    let video = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(video.path(), [1, 2, 3]).unwrap();
    let (url, requests) = serve(vec![Some(complete_response(2))]).await;
    assert!(
        upload_chunks(&client().unwrap(), url, "token", video.path(), 3, |_| {})
            .await
            .is_err()
    );
    requests.await.unwrap();
}

#[tokio::test]
async fn cancellation_drops_pending_network_work() {
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        trigger.store(true, Ordering::Release);
    });
    let result: Result<()> = cancellable(&cancel, std::future::pending()).await;
    assert_eq!(result.unwrap_err(), CANCELLED);
}

#[tokio::test]
async fn denied_google_consent_finishes_without_waiting_for_timeout() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let authorization = Authorization {
        url: String::new(),
        listener,
        redirect: String::new(),
        verifier: String::new(),
        state: "expected-state".into(),
        client_secret: None,
    };
    let task = tokio::spawn(async move { authorization.complete(&AtomicBool::new(false)).await });
    let mut socket = tokio::net::TcpStream::connect(address).await.unwrap();
    socket.write_all(b"GET /oauth/callback?state=expected-state&error=access_denied HTTP/1.1\r\nHost: localhost\r\n\r\n").await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.unwrap_err(), "Google Drive access was not granted");
}
