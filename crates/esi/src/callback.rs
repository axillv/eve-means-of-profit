//! The local listener that receives EVE's answer after the player logs in in the browser.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::auth::CALLBACK_PORT;
use crate::error::EsiError;
use crate::http::decode;

const POLL: Duration = Duration::from_millis(50);
/// A browser sends its request line at once; a connection silent for longer is dropped so cancel and the deadline stay prompt.
const READ_LIMIT: Duration = Duration::from_millis(100);

/// Waits for one answer for the login that carries `expected_state`. Both listeners close before this returns.
pub fn wait_for_code(
    expected_state: &str,
    deadline: Instant,
    cancel: &AtomicBool,
) -> Result<String, EsiError> {
    let v4 = listen(("127.0.0.1", CALLBACK_PORT))?;
    // Some machines have no IPv6; the IPv4 listener is enough there.
    let v6 = listen(("::1", CALLBACK_PORT));
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(EsiError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(EsiError::TimedOut);
        }
        for listener in [Some(&v4), v6.as_ref().ok()].into_iter().flatten() {
            if let Ok((stream, _)) = listener.accept()
                && let Some(answer) = handle(&stream, expected_state)
            {
                return answer;
            }
        }
        sleep(POLL);
    }
}

fn listen(address: (&str, u16)) -> Result<TcpListener, EsiError> {
    TcpListener::bind(address)
        .and_then(|listener| listener.set_nonblocking(true).map(|()| listener))
        .ok()
        .ok_or(EsiError::PortInUse(CALLBACK_PORT))
}

/// `None` means the request was not the answer (another path, or no code) and the wait goes on.
fn handle(stream: &TcpStream, expected_state: &str) -> Option<Result<String, EsiError>> {
    // An accepted socket may inherit the listener's non-blocking mode; the read below must wait.
    if stream.set_nonblocking(false).is_err() || stream.set_read_timeout(Some(READ_LIMIT)).is_err()
    {
        return None;
    }
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let (Some("GET"), Some(target)) = (parts.next(), parts.next()) else {
        respond(stream, 404, "Not Found", "Not found.");
        return None;
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        respond(stream, 404, "Not Found", "Not found.");
        return None;
    }
    let params: Vec<(&str, String)> = query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .filter_map(|(key, value)| Some((key, decode(value)?)))
        .collect();
    let find = |name: &str| {
        params
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    };

    if let Some(error) = find("error") {
        let description = find("error_description").unwrap_or(error);
        respond(
            stream,
            400,
            "Bad Request",
            &format!("Login failed: {description}"),
        );
        return Some(Err(EsiError::Denied(description.to_owned())));
    }
    let (Some(code), Some(state)) = (find("code"), find("state")) else {
        respond(
            stream,
            400,
            "Bad Request",
            "Login failed: the answer had no code.",
        );
        return None;
    };
    if state != expected_state {
        respond(
            stream,
            400,
            "Bad Request",
            "Login failed: this answer was not for the login the app started. Try again from the app.",
        );
        return Some(Err(EsiError::StateMismatch));
    }
    respond(
        stream,
        200,
        "OK",
        "Login complete. You can close this tab and go back to EVE Means of Profit.",
    );
    Some(Ok(code.to_owned()))
}

fn respond(mut stream: &TcpStream, status: u16, reason: &str, message: &str) {
    let body = format!(
        "<!doctype html><html><body><p>{}</p></body></html>",
        escape(message)
    );
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    // A browser that went away gets no answer; the wait goes on either way.
    drop(stream.write_all(format!("{head}{body}").as_bytes()));
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
