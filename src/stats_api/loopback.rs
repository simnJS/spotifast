//! Connecting through the browser, with RFC 8252's loopback redirect.
//!
//! Spotifast listens on a free port of this computer, then opens
//! `{server}/auth/login?app_redirect=http://127.0.0.1:{port}/callback&app_state={state}`.
//! Once the person consents on Spotify's page, the server sends the
//! browser back to `/callback?token=…&state=…`, or `?error=…&state=…`.
//! The state is random for each attempt, so only the redirect of this
//! attempt is taken; the listener closes once it came, or after five
//! minutes. Like [`crate::auth`]'s, it runs on the backend's runtime.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

use super::Token;

/// How long the browser has to come back.
pub const TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// The path the server sends the browser back to.
pub const CALLBACK_PATH: &str = "/callback";
/// How long one request has to send its first line.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// The longest first line read.
const REQUEST_LINE_LIMIT: u64 = 8 * 1024;
/// The most of the server's refusal shown.
const MESSAGE_CHARS: usize = 200;

/// A browser sign-in waiting for the server's redirect.
pub struct Login {
    listener: TcpListener,
    port: u16,
    state: String,
    /// The server's login page, to open in the browser.
    pub url: String,
}

fn listen_error(error: std::io::Error) -> String {
    format!("Couldn't listen for the browser's answer: {error}")
}

impl Login {
    /// Listens on a free port of this computer and builds the server's
    /// login address for it. Call it on the runtime.
    pub fn start(server: &str) -> Result<Self, String> {
        let address: SocketAddr = ([127, 0, 0, 1], 0).into();
        let listener = std::net::TcpListener::bind(address).map_err(listen_error)?;
        listener.set_nonblocking(true).map_err(listen_error)?;
        let port = listener.local_addr().map_err(listen_error)?.port();
        let listener = TcpListener::from_std(listener).map_err(listen_error)?;
        let state = format!(
            "{:016x}{:016x}",
            rand::random::<u64>(),
            rand::random::<u64>()
        );
        let url = login_url(server, port, &state)?;
        Ok(Self {
            listener,
            port,
            state,
            url,
        })
    }

    /// The port the browser is sent back to.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Waits for the server's redirect and answers the browser with a
    /// page: the token, or why there is none. Gives up after [`TIMEOUT`].
    pub async fn wait(self) -> Result<Token, String> {
        tokio::time::timeout(TIMEOUT, wait_on(&self.listener, &self.state))
            .await
            .map_err(|_| {
                "The browser did not come back within five minutes. Connect again.".to_owned()
            })?
    }
}

/// The server's login page that sends the browser back to `port`.
pub fn login_url(server: &str, port: u16, state: &str) -> Result<String, String> {
    let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    reqwest::Url::parse_with_params(
        &format!("{server}/auth/login"),
        &[("app_redirect", redirect.as_str()), ("app_state", state)],
    )
    .map(|url| url.to_string())
    .map_err(|_| format!("{server} is not an address a browser can open."))
}

/// What one request to the listener was.
#[derive(Debug, PartialEq, Eq)]
enum Callback {
    /// The server sent the token.
    Token(Token),
    /// The server said why there is none.
    Refused(String),
    /// Not the redirect this attempt waits for: another path, or another
    /// attempt's state.
    Other,
}

/// Reads the request line of one request to the listener.
fn parse_callback(request_line: &str, expected_state: &str) -> Callback {
    let mut parts = request_line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        return Callback::Other;
    };
    if !target.starts_with('/') {
        return Callback::Other;
    }
    let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else {
        return Callback::Other;
    };
    if url.path() != CALLBACK_PATH {
        return Callback::Other;
    }
    let mut token = None;
    let mut state = None;
    let mut error = None;
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "token" => token = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Callback::Other;
    }
    if let Some(error) = error {
        let error: String = error.chars().take(MESSAGE_CHARS).collect();
        return Callback::Refused(format!("The statistics server refused: {error}"));
    }
    match token.as_deref().and_then(Token::new) {
        Some(token) => Callback::Token(token),
        None => Callback::Refused("The statistics server sent no token.".into()),
    }
}

/// Answers requests until the redirect of this attempt comes.
async fn wait_on(listener: &TcpListener, expected_state: &str) -> Result<Token, String> {
    loop {
        let (mut stream, _) = listener.accept().await.map_err(listen_error)?;
        let mut reader = BufReader::new(&mut stream).take(REQUEST_LINE_LIMIT);
        let mut request_line = String::new();
        match tokio::time::timeout(REQUEST_TIMEOUT, reader.read_line(&mut request_line)).await {
            Ok(Ok(_)) if request_line.ends_with('\n') => {}
            _ => continue,
        }
        let callback = parse_callback(&request_line, expected_state);
        let (status, body) = match &callback {
            Callback::Token(_) => ("200 OK", page(None)),
            Callback::Refused(reason) => ("400 Bad Request", page(Some(reason.as_str()))),
            Callback::Other => ("404 Not Found", String::new()),
        };
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes()).await;
        let _ = stream.shutdown().await;
        match callback {
            Callback::Token(token) => return Ok(token),
            Callback::Refused(reason) => return Err(reason),
            // A favicon request, or a request that is not the redirect:
            // keep waiting.
            Callback::Other => {}
        }
    }
}

fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            c => escaped.push(c),
        }
    }
    escaped
}

/// The page the browser shows: connected, or why not.
fn page(failure: Option<&str>) -> String {
    let (title, heading, body, accent) = match failure {
        None => (
            "Spotifast est connecté",
            "Spotifast est connecté",
            "Vous pouvez fermer cet onglet.".to_owned(),
            "#1ed760",
        ),
        Some(reason) => (
            "Connexion impossible",
            "La connexion n'a pas abouti",
            format!(
                "{}. Revenez dans Spotifast et réessayez.",
                escape(reason.trim_end_matches('.'))
            ),
            "#f5717f",
        ),
    };
    format!(
        "<!doctype html><html lang=\"fr\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title>\
<style>:root{{color-scheme:dark}}body{{margin:0;min-height:100vh;display:grid;place-items:center;background:#0f1114;color:#e8eaed;font-family:Inter,system-ui,sans-serif}}\
main{{max-width:28rem;padding:2.5rem;border-radius:1.25rem;background:#181b20;text-align:center}}\
.mark{{width:64px;height:64px;border-radius:50%;background:{accent};margin:0 auto 1.25rem}}\
h1{{font-size:1.4rem;margin:.25rem 0 .5rem}}p{{color:#a5adba;line-height:1.5;margin:0}}</style>\
<main><div class=\"mark\"></div><h1>{heading}</h1><p>{body}</p></main></html>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn token(text: &str) -> Callback {
        Callback::Token(Token::new(text).unwrap())
    }

    #[test]
    fn the_redirect_of_this_attempt_carries_the_token() {
        assert_eq!(
            parse_callback("GET /callback?token=abc%2Bdef&state=s1 HTTP/1.1\r\n", "s1"),
            token("abc+def")
        );
        assert_eq!(
            parse_callback("GET /callback?state=s1&token=xyz HTTP/1.1\r\n", "s1"),
            token("xyz")
        );
    }

    #[test]
    fn another_state_path_or_method_is_not_the_redirect() {
        assert_eq!(
            parse_callback("GET /callback?token=abc&state=other HTTP/1.1\r\n", "s1"),
            Callback::Other
        );
        assert_eq!(
            parse_callback("GET /callback?token=abc HTTP/1.1\r\n", "s1"),
            Callback::Other
        );
        assert_eq!(
            parse_callback("GET /favicon.ico HTTP/1.1\r\n", "s1"),
            Callback::Other
        );
        assert_eq!(
            parse_callback("POST /callback?token=abc&state=s1 HTTP/1.1\r\n", "s1"),
            Callback::Other
        );
        assert_eq!(parse_callback("garbage", "s1"), Callback::Other);
    }

    #[test]
    fn a_refusal_or_a_missing_token_ends_the_attempt() {
        assert_eq!(
            parse_callback(
                "GET /callback?error=access%20denied&state=s1 HTTP/1.1\r\n",
                "s1"
            ),
            Callback::Refused("The statistics server refused: access denied".into())
        );
        assert_eq!(
            parse_callback("GET /callback?state=s1 HTTP/1.1\r\n", "s1"),
            Callback::Refused("The statistics server sent no token.".into())
        );
    }

    #[test]
    fn the_login_address_names_the_redirect_and_the_state() {
        let url = login_url("https://stats.example", 41234, "abc").unwrap();
        assert_eq!(
            url,
            "https://stats.example/auth/login?app_redirect=http%3A%2F%2F127.0.0.1%3A41234%2Fcallback&app_state=abc"
        );
    }

    #[test]
    fn the_failure_page_escapes_the_servers_words() {
        let page = page(Some("<script>x</script>"));
        assert!(!page.contains("<script>x"));
        assert!(page.contains("&lt;script&gt;"));
    }

    #[tokio::test]
    async fn the_browsers_redirect_ends_the_wait_with_the_token() {
        let login = Login::start("https://stats.example").unwrap();
        let port = login.port;
        let state = login.state.clone();
        assert!(login.url.contains(&format!("app_state={state}")));
        let waiting = tokio::spawn(login.wait());
        // A favicon request first does not end it.
        let mut favicon = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        favicon
            .write_all(b"GET /favicon.ico HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        let mut answer = String::new();
        favicon.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 404"));
        let mut browser = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        browser
            .write_all(format!("GET /callback?token=t0k&state={state} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        let mut answer = String::new();
        browser.read_to_string(&mut answer).await.unwrap();
        assert!(answer.starts_with("HTTP/1.1 200"));
        assert!(answer.contains("Spotifast est connecté"));
        let token = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(token.as_str(), "t0k");
    }
}
