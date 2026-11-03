//! The local server on 127.0.0.1: the SDK WebSocket at `/`, and plugin
//! files at `/plugin/<uuid>/<path>` for settings panels and HTML plugins.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use tokio::sync::mpsc;

use crate::host::Msg;

#[derive(Clone)]
struct Shared {
    plugin_dirs: Arc<HashMap<String, PathBuf>>,
    host: mpsc::UnboundedSender<Msg>,
}

static NEXT_CONN: AtomicU64 = AtomicU64::new(1);

/// Binds a free port on 127.0.0.1 and serves in the background.
pub(crate) async fn bind(
    plugin_dirs: Arc<HashMap<String, PathBuf>>,
    host: mpsc::UnboundedSender<Msg>,
) -> Result<u16, String> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|e| e.to_string())?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    let app = Router::new()
        .route("/", get(ws))
        .route("/plugin/{uuid}/{*path}", get(file))
        .layer(axum::middleware::from_fn(move |req: axum::extract::Request, next: axum::middleware::Next| async move {
            if local_request(req.headers(), port) {
                next.run(req).await
            } else {
                StatusCode::FORBIDDEN.into_response()
            }
        }))
        .with_state(Shared { plugin_dirs, host });
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(port)
}

/// Only Agora's own pages and local programs may use the server. A web page
/// open in the user's browser can't: its Origin is another site, and a
/// DNS-rebound name shows up in Host. Pages served by other local servers
/// (another port on 127.0.0.1) are kept out too: only pages this server
/// handed out carry its own port.
fn local_request(headers: &HeaderMap, port: u16) -> bool {
    let ours = [format!("127.0.0.1:{port}"), format!("localhost:{port}")];
    let host = headers.get(header::HOST).and_then(|h| h.to_str().ok()).unwrap_or("");
    let host_ok = ours.iter().any(|h| h.eq_ignore_ascii_case(host));
    let origin_ok = match headers.get(header::ORIGIN) {
        // Node and .exe plugins usually send no Origin.
        None => true,
        Some(o) => o.to_str().ok().is_some_and(|o| {
            ours.iter()
                .any(|h| o.eq_ignore_ascii_case(&format!("http://{h}")))
        }),
    };
    host_ok && origin_ok
}

async fn ws(upgrade: WebSocketUpgrade, State(s): State<Shared>) -> Response {
    // Plugin images can be large data URIs.
    upgrade
        .max_message_size(16 << 20)
        .on_upgrade(move |socket| run_socket(socket, s.host))
}

async fn run_socket(mut socket: WebSocket, host: mpsc::UnboundedSender<Msg>) {
    let id = NEXT_CONN.fetch_add(1, Ordering::Relaxed);
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    if host.send(Msg::SocketOpen(id, tx)).is_err() {
        return;
    }
    loop {
        tokio::select! {
            out = rx.recv() => match out {
                Some(text) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                None => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(t))) => {
                    let _ = host.send(Msg::SocketText(id, t.to_string()));
                }
                Some(Ok(Message::Binary(b))) => {
                    if let Ok(t) = String::from_utf8(b.to_vec()) {
                        let _ = host.send(Msg::SocketText(id, t));
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
        }
    }
    let _ = host.send(Msg::SocketClosed(id));
}

/// Calls the page's `connectElgatoStreamDeckSocket` (or `connectSDSocket`)
/// with the values from the address, once the page has loaded. This is how
/// the Stream Dock app starts settings panels and HTML plugins.
const BOOTSTRAP: &str = r#"<script>(function(){var q=new URLSearchParams(location.search);if(!q.has("agora"))return;function go(){var f=window.connectElgatoStreamDeckSocket||window.connectSDSocket;if(typeof f!=="function"){console.warn("Agora: no connect function on this page");return;}var a=[q.get("port"),q.get("uuid"),q.get("event"),q.get("info")];if(q.has("actionInfo"))a.push(q.get("actionInfo"));f.apply(window,a);}if(document.readyState==="complete")setTimeout(go,0);else window.addEventListener("load",go);})();</script>"#;

/// Address of a plugin page that connects itself to the host when loaded.
pub(crate) fn connect_url(
    port: u16,
    plugin: &str,
    rel: &str,
    uuid: &str,
    event: &str,
    info: &str,
    action_info: Option<&str>,
) -> String {
    let enc = |s: &str| utf8_percent_encode(s, NON_ALPHANUMERIC).to_string();
    let path: Vec<String> = rel.split('/').filter(|s| !s.is_empty()).map(enc).collect();
    let mut url = format!(
        "http://127.0.0.1:{port}/plugin/{}/{}?agora=1&port={port}&uuid={}&event={}&info={}",
        enc(plugin),
        path.join("/"),
        enc(uuid),
        enc(event),
        enc(info)
    );
    if let Some(ai) = action_info {
        url.push_str("&actionInfo=");
        url.push_str(&enc(ai));
    }
    url
}

fn content_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("js") | Some("mjs") | Some("cjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        _ => "application/octet-stream",
    }
}

async fn file(
    Path((uuid, rel)): Path<(String, String)>,
    RawQuery(query): RawQuery,
    State(s): State<Shared>,
) -> Response {
    let Some(root) = s.plugin_dirs.get(&uuid) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // Resolve the real path and refuse anything outside the plugin folder.
    let (Ok(root), Ok(path)) = (root.canonicalize(), root.join(&rel).canonicalize()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    if !path.starts_with(&root) || !path.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(mut bytes) = tokio::fs::read(&path).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ctype = content_type(&path);
    let wants_connect = query
        .as_deref()
        .is_some_and(|q| q.split('&').any(|p| p == "agora=1"));
    if ctype.starts_with("text/html") && wants_connect {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let lower = text.to_ascii_lowercase();
        let html = match lower.rfind("</body>") {
            Some(i) => format!("{}{}{}", &text[..i], BOOTSTRAP, &text[i..]),
            None => format!("{text}{BOOTSTRAP}"),
        };
        bytes = html.into_bytes();
    }
    (
        [
            (header::CONTENT_TYPE, ctype),
            (header::CACHE_CONTROL, "no-store"),
        ],
        bytes,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_local_pages_and_programs_get_in() {
        let h = |host: &str, origin: Option<&str>| {
            let mut m = HeaderMap::new();
            m.insert(header::HOST, host.parse().unwrap());
            if let Some(o) = origin {
                m.insert(header::ORIGIN, o.parse().unwrap());
            }
            local_request(&m, 5000)
        };
        assert!(h("127.0.0.1:5000", None));
        assert!(h("localhost:5000", Some("http://127.0.0.1:5000")));
        assert!(h("127.0.0.1:5000", Some("http://LOCALHOST:5000")));
        // Pages from other local servers don't get in.
        assert!(!h("127.0.0.1:5000", Some("http://localhost")));
        assert!(!h("127.0.0.1:5000", Some("http://127.0.0.1:5173")));
        assert!(!h("127.0.0.1:5000", Some("http://[::1]:8080")));
        assert!(!h("127.0.0.1:5000", Some("https://127.0.0.1:5000")));
        assert!(!h("127.0.0.1:5000", Some("http://127.0.0.1:50000")));
        assert!(!h("127.0.0.1:5000", Some("https://evil.example")));
        assert!(!h("127.0.0.1:5000", Some("http://127.0.0.1.evil.example")));
        assert!(!h("127.0.0.1:5000", Some("http://localhost.evil.example:5000")));
        assert!(!h("127.0.0.1:5000", Some("null")));
        assert!(!h("rebound.evil.example:5000", None));
        assert!(!h("127.0.0.1:5001", None));
    }

    #[test]
    fn connect_url_encodes_everything() {
        let u = connect_url(
            5000,
            "com.a",
            "pi/index.html",
            "CTX",
            "registerPropertyInspector",
            "{\"a\":1}",
            Some("{}"),
        );
        assert!(u.starts_with(
            "http://127.0.0.1:5000/plugin/com%2Ea/pi/index%2Ehtml?agora=1&port=5000&uuid=CTX"
        ));
        assert!(u.contains("info=%7B%22a%22%3A1%7D"));
        assert!(u.ends_with("&actionInfo=%7B%7D"));
    }
}
