use std::collections::HashMap;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use url::Url;

const CALLBACK_PATH: &str = "/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_REQUEST_BYTES: usize = 8 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CallbackError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("OAuth callback timed out")]
    Timeout,
    #[error("Authorization was rejected: {0}")]
    AuthorizationRejected(String),
}

/// A loopback OAuth callback listener that retains ownership of its bound port
/// from flow creation until the callback is accepted.
pub struct CallbackListener {
    listener: TcpListener,
    port: u16,
}

impl CallbackListener {
    pub async fn bind() -> Result<Self, CallbackError> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        tracing::info!(port, "OAuth callback server listening");
        Ok(Self { listener, port })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Wait for a callback that contains every `required_params` entry, then
    /// return the first present result key. Invalid/noise requests are rejected
    /// without consuming the login session.
    pub async fn wait_for_callback(
        self,
        required_params: &[(&str, &str)],
        result_keys: &[&str],
    ) -> Result<(String, String), CallbackError> {
        self.wait_for_callback_with_timeout(required_params, result_keys, CALLBACK_TIMEOUT)
            .await
    }

    async fn wait_for_callback_with_timeout(
        self,
        required_params: &[(&str, &str)],
        result_keys: &[&str],
        wait_timeout: Duration,
    ) -> Result<(String, String), CallbackError> {
        timeout(
            wait_timeout,
            self.wait_for_valid_request(required_params, result_keys),
        )
        .await
        .map_err(|_| CallbackError::Timeout)?
    }

    async fn wait_for_valid_request(
        self,
        required_params: &[(&str, &str)],
        result_keys: &[&str],
    ) -> Result<(String, String), CallbackError> {
        loop {
            let (mut stream, remote_addr) = self.listener.accept().await?;
            if !remote_addr.ip().is_loopback() {
                write_response(&mut stream, "403 Forbidden", failure_html()).await?;
                continue;
            }

            let request = match read_request(&mut stream).await {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!(error = %error, "Rejected malformed OAuth callback request");
                    write_response(&mut stream, "400 Bad Request", failure_html()).await?;
                    continue;
                }
            };

            let params = match parse_callback_request(&request, self.port) {
                Ok(params) => params,
                Err(reason) => {
                    tracing::warn!(reason, "Rejected invalid OAuth callback request");
                    write_response(&mut stream, "400 Bad Request", failure_html()).await?;
                    continue;
                }
            };

            if !required_params.iter().all(|(key, expected)| {
                params
                    .get(*key)
                    .is_some_and(|actual| constant_time_eq(actual.as_bytes(), expected.as_bytes()))
            }) {
                tracing::warn!("Rejected OAuth callback with invalid session state");
                write_response(&mut stream, "400 Bad Request", failure_html()).await?;
                continue;
            }

            if let Some(error) = params.get("error") {
                write_response(&mut stream, "200 OK", rejected_html()).await?;
                return Err(CallbackError::AuthorizationRejected(
                    error.chars().take(128).collect(),
                ));
            }

            if let Some((key, value)) = result_keys.iter().find_map(|key| {
                params
                    .get(*key)
                    .map(|value| ((*key).to_string(), value.clone()))
            }) {
                write_response(&mut stream, "200 OK", success_html()).await?;
                tracing::info!(parameter = key, "OAuth callback accepted");
                return Ok((key, value));
            }

            write_response(&mut stream, "400 Bad Request", failure_html()).await?;
        }
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<String, CallbackError> {
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];

    loop {
        let read = timeout(REQUEST_READ_TIMEOUT, stream.read(&mut chunk))
            .await
            .map_err(|_| CallbackError::Timeout)??;
        if read == 0 {
            break;
        }
        request.extend_from_slice(&chunk[..read]);
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
        if request.len() >= MAX_REQUEST_BYTES {
            return Err(CallbackError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "OAuth callback request headers are too large",
            )));
        }
    }

    String::from_utf8(request).map_err(|error| {
        CallbackError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })
}

fn parse_callback_request(
    request: &str,
    expected_port: u16,
) -> Result<HashMap<String, String>, &'static str> {
    let mut lines = request.split("\r\n");
    let request_line = lines.next().ok_or("missing request line")?;
    let mut request_parts = request_line.split_whitespace();
    if request_parts.next() != Some("GET") {
        return Err("callback method must be GET");
    }
    let target = request_parts.next().ok_or("missing request target")?;
    if request_parts.next().is_none() || request_parts.next().is_some() {
        return Err("malformed request line");
    }

    let host = lines
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("host")
                    .then(|| value.trim().to_string())
            })
        })
        .ok_or("missing Host header")?;
    let expected_host = format!("127.0.0.1:{expected_port}");
    if host != expected_host {
        return Err("unexpected Host header");
    }

    let url =
        Url::parse(&format!("http://{host}{target}")).map_err(|_| "invalid callback target")?;
    if url.path() != CALLBACK_PATH {
        return Err("unexpected callback path");
    }

    Ok(url.query_pairs().into_owned().collect())
}

async fn write_response(
    stream: &mut TcpStream,
    status: &str,
    html: &str,
) -> Result<(), CallbackError> {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await?;
    Ok(())
}

fn success_html() -> &'static str {
    "<html><body><h1>Authorization successful</h1><p>You can close this tab and return to Awayuki.</p></body></html>"
}

fn rejected_html() -> &'static str {
    "<html><body><h1>Authorization was cancelled</h1><p>You can close this tab and return to Awayuki.</p></body></html>"
}

fn failure_html() -> &'static str {
    "<html><body><h1>Invalid authorization callback</h1><p>Return to Awayuki and try again.</p></body></html>"
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn try_send_request(port: u16, request: &str) -> std::io::Result<String> {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).await?;
        stream.write_all(request.as_bytes()).await?;
        stream.shutdown().await?;
        let mut response = String::new();
        stream.read_to_string(&mut response).await?;
        Ok(response)
    }

    async fn send_request(port: u16, request: &str) -> String {
        try_send_request(port, request)
            .await
            .expect("complete callback request")
    }

    #[tokio::test]
    async fn accepts_matching_state_and_code() {
        let listener = CallbackListener::bind().await.expect("bind listener");
        let port = listener.port();
        let callback = tokio::spawn(async move {
            listener
                .wait_for_callback(&[("state", "expected-state")], &["code"])
                .await
        });

        let response = send_request(
            port,
            &format!(
                "GET /callback?code=abc%20123&state=expected-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(
            callback.await.expect("callback task").expect("callback"),
            ("code".to_string(), "abc 123".to_string())
        );
    }

    #[tokio::test]
    async fn ignores_invalid_state_until_valid_callback_arrives() {
        let listener = CallbackListener::bind().await.expect("bind listener");
        let port = listener.port();
        let callback = tokio::spawn(async move {
            listener
                .wait_for_callback(&[("state", "expected-state")], &["code"])
                .await
        });

        let invalid_response = send_request(
            port,
            &format!(
                "GET /callback?code=attacker&state=wrong HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            ),
        )
        .await;
        assert!(invalid_response.starts_with("HTTP/1.1 400 Bad Request"));

        let valid_response = send_request(
            port,
            &format!(
                "GET /callback?code=real&state=expected-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            ),
        )
        .await;
        assert!(valid_response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(
            callback.await.expect("callback task").expect("callback"),
            ("code".to_string(), "real".to_string())
        );
    }

    #[tokio::test]
    async fn matching_cancellation_finishes_the_session_without_returning_a_code() {
        let listener = CallbackListener::bind().await.expect("bind listener");
        let port = listener.port();
        let callback = tokio::spawn(async move {
            listener
                .wait_for_callback(&[("state", "cancel-state")], &["code"])
                .await
        });

        let response = send_request(
            port,
            &format!(
                "GET /callback?error=access_denied&state=cancel-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(response.contains("Authorization was cancelled"));
        assert!(matches!(
            callback.await.expect("callback task"),
            Err(CallbackError::AuthorizationRejected(reason)) if reason == "access_denied"
        ));
    }

    #[tokio::test]
    async fn aborting_an_old_login_releases_its_listener_and_new_login_rejects_old_state() {
        let old_listener = CallbackListener::bind().await.expect("bind old listener");
        let old_port = old_listener.port();
        let old_callback = tokio::spawn(async move {
            old_listener
                .wait_for_callback(&[("state", "old-state")], &["code"])
                .await
        });
        old_callback.abort();
        assert!(old_callback
            .await
            .expect_err("old login must be cancelled")
            .is_cancelled());
        assert!(try_send_request(
            old_port,
            &format!(
                "GET /callback?code=stale&state=old-state HTTP/1.1\r\nHost: 127.0.0.1:{old_port}\r\n\r\n"
            )
        )
        .await
        .is_err());

        let new_listener = CallbackListener::bind().await.expect("bind new listener");
        let new_port = new_listener.port();
        let new_callback = tokio::spawn(async move {
            new_listener
                .wait_for_callback(&[("state", "new-state")], &["code"])
                .await
        });
        let stale = send_request(
            new_port,
            &format!(
                "GET /callback?code=stale&state=old-state HTTP/1.1\r\nHost: 127.0.0.1:{new_port}\r\n\r\n"
            ),
        )
        .await;
        assert!(stale.starts_with("HTTP/1.1 400 Bad Request"));
        let current = send_request(
            new_port,
            &format!(
                "GET /callback?code=current&state=new-state HTTP/1.1\r\nHost: 127.0.0.1:{new_port}\r\n\r\n"
            ),
        )
        .await;
        assert!(current.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(
            new_callback
                .await
                .expect("new callback task")
                .expect("new callback"),
            ("code".to_string(), "current".to_string())
        );
    }

    #[tokio::test]
    async fn accepted_state_is_single_use_because_listener_is_consumed() {
        let listener = CallbackListener::bind().await.expect("bind listener");
        let port = listener.port();
        let callback = tokio::spawn(async move {
            listener
                .wait_for_callback(&[("state", "once")], &["code"])
                .await
        });
        let request = format!(
            "GET /callback?code=first&state=once HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
        );
        assert!(send_request(port, &request)
            .await
            .starts_with("HTTP/1.1 200 OK"));
        callback
            .await
            .expect("callback task")
            .expect("first callback");

        assert!(try_send_request(port, &request).await.is_err());
    }

    #[tokio::test]
    async fn expired_session_returns_timeout_and_releases_listener() {
        let listener = CallbackListener::bind().await.expect("bind listener");
        let port = listener.port();
        let result = listener
            .wait_for_callback_with_timeout(
                &[("state", "expired")],
                &["code"],
                Duration::from_millis(10),
            )
            .await;
        assert!(matches!(result, Err(CallbackError::Timeout)));
        assert!(try_send_request(
            port,
            &format!(
                "GET /callback?code=late&state=expired HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            )
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn callback_preserves_url_encoded_authorization_codes_used_with_pkce() {
        let listener = CallbackListener::bind().await.expect("bind listener");
        let port = listener.port();
        let callback = tokio::spawn(async move {
            listener
                .wait_for_callback(&[("state", "pkce-state")], &["code"])
                .await
        });

        let response = send_request(
            port,
            &format!(
                "GET /callback?code=AZaz09-._~%2B%2F%3D&state=pkce-state HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert_eq!(
            callback.await.expect("callback task").expect("callback"),
            ("code".to_string(), "AZaz09-._~+/=".to_string())
        );
    }

    #[test]
    fn rejects_wrong_method_path_and_host() {
        let port = 12345;
        assert!(parse_callback_request(
            "POST /callback?code=x HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n\r\n",
            port
        )
        .is_err());
        assert!(parse_callback_request(
            "GET /other?code=x HTTP/1.1\r\nHost: 127.0.0.1:12345\r\n\r\n",
            port
        )
        .is_err());
        assert!(parse_callback_request(
            "GET /callback?code=x HTTP/1.1\r\nHost: localhost:12345\r\n\r\n",
            port
        )
        .is_err());
    }
}
