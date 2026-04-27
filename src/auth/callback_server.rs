use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, thiserror::Error)]
pub enum CallbackError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("No authorization code in callback")]
    NoCode,
}

/// Start a temporary local HTTP server to receive the OAuth callback.
/// Returns the authorization code from the callback URL.
pub async fn wait_for_callback(port: u16) -> Result<String, CallbackError> {
    wait_for_param(port, "code").await
}

/// Wait for any callback request and return the value of the first matching query param
/// from `keys` (in order). Used by MiAuth (`session=`) and Mastodon OAuth (`code=`).
pub async fn wait_for_callback_any(
    port: u16,
    keys: &[&str],
) -> Result<(String, String), CallbackError> {
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    tracing::info!("OAuth callback server listening on port {}", port);

    let (mut stream, _addr) = listener.accept().await?;

    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let mut found: Option<(String, String)> = None;
    for key in keys {
        if let Some(value) = extract_param(&request, key) {
            found = Some(((*key).to_string(), value));
            break;
        }
    }

    let html = "<html><body>\
        <h1>Authorization successful!</h1>\
        <p>You can close this tab and return to awayuki.</p>\
        </body></html>";
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        html.len(),
        html
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;

    found.ok_or(CallbackError::NoCode)
}

async fn wait_for_param(port: u16, key: &str) -> Result<String, CallbackError> {
    let (_, value) = wait_for_callback_any(port, &[key]).await?;
    tracing::info!("Callback received {}", key);
    Ok(value)
}

fn extract_param(request: &str, key: &str) -> Option<String> {
    // Parse "GET /callback?key=XXX&... HTTP/1.1"
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;

    let query_start = path.find('?')? + 1;
    let query = &path[query_start..];

    let prefix = format!("{}=", key);
    for param in query.split('&') {
        if let Some(value) = param.strip_prefix(prefix.as_str()) {
            return Some(urlencoding::decode(value).ok()?.into_owned());
        }
    }
    None
}

/// Find an available port for the callback server.
pub async fn find_available_port() -> Result<u16, std::io::Error> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}
