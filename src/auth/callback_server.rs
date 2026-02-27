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
    let listener = TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    tracing::info!("OAuth callback server listening on port {}", port);

    let (mut stream, _addr) = listener.accept().await?;

    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    let code = extract_code(&request).ok_or(CallbackError::NoCode)?;

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

    tracing::info!("OAuth callback received authorization code");
    Ok(code)
}

fn extract_code(request: &str) -> Option<String> {
    // Parse "GET /callback?code=XXX&... HTTP/1.1"
    let first_line = request.lines().next()?;
    let path = first_line.split_whitespace().nth(1)?;

    let query_start = path.find('?')? + 1;
    let query = &path[query_start..];

    for param in query.split('&') {
        if let Some(value) = param.strip_prefix("code=") {
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
