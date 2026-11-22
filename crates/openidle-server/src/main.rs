//! openidle-server: shared worlds for OpenIdle games.
//!
//! The server reads every game in a folder, keeps worlds of them running on its clock (an idle
//! game goes on when nobody watches), and lets browsers in over a WebSocket at `/ws`; see hub.rs
//! for the messages. The worlds are saved as they change, and on a restart they run for the time
//! the server was away (up to each game's `offline_max`).
//!
//! Plain HTTP on the same port: `/health`, `/stats.json` (games, connections, worlds),
//! `/games/<game>.json` (a game's bundle, for single-player pages), and with `OI_SITE` the files
//! of a static site (the browser player in `web/`).
//!
//! Settings, from the environment:
//!
//! * `OI_ADDR`: where to listen (default `0.0.0.0:8080`);
//! * `OI_GAMES`: the folder of games, one sub-folder each (default `games`);
//! * `OI_DATA`: where worlds are kept (default `data`; empty: in memory only);
//! * `OI_SITE`: a folder to serve as the site (default none);
//! * `OI_ORIGINS`: pages allowed to connect, comma-separated (default `*`, any);
//! * `OI_MAX_WORLDS`, `OI_MAX_PLAYERS`: limits (default 100 worlds, 200 players a world).

mod hub;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use openidle::Game;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::Message;

use hub::{Hub, Out};

const MAX_MESSAGE: usize = 16 * 1024;
/// Messages a second a connection may send (and in a burst).
const RATE: f64 = 30.0;
const BURST: f64 = 60.0;

struct Server {
    hub: Mutex<Hub>,
    senders: Mutex<BTreeMap<u64, UnboundedSender<Message>>>,
    site: Option<PathBuf>,
    origins: Vec<String>,
}

impl Server {
    fn deliver(&self, out: Out) {
        if out.is_empty() {
            return;
        }
        let senders = self.senders.lock().unwrap();
        for (id, msg) in out {
            if let Some(tx) = senders.get(&id) {
                let _ = tx.send(Message::Text(msg.to_string()));
            }
        }
    }
}

fn allowed(origin: &str, origins: &[String]) -> bool {
    origins
        .iter()
        .any(|a| a == "*" || origin == a || origin.starts_with(&format!("{a}:")))
}

/// A request's head, peeked at without reading it: (its length, method, target, is it a WebSocket).
/// The target is the path with its query, if any.
async fn peek_head(stream: &TcpStream) -> Option<(usize, String, String, bool)> {
    let mut buf = vec![0u8; 8192];
    for _ in 0..250 {
        let n = stream.peek(&mut buf).await.ok()?;
        if n == 0 {
            return None;
        }
        if let Some(end) = buf[..n].windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]);
            let mut lines = head.lines();
            let mut first = lines.next()?.split(' ');
            let method = first.next()?.to_string();
            let target = first.next()?.to_string();
            let ws = lines.any(|l| {
                l.split_once(':').is_some_and(|(k, v)| {
                    k.trim().eq_ignore_ascii_case("upgrade") && v.to_ascii_lowercase().contains("websocket")
                })
            });
            return Some((end + 4, method, target, ws));
        }
        if n == buf.len() {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webmanifest" => "application/manifest+json",
        "txt" | "md" | "idle" | "toml" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

/// Answers a plain HTTP request.
async fn http(mut stream: TcpStream, len: usize, method: &str, target: &str, server: &Server) {
    let mut head = vec![0u8; len];
    if stream.read_exact(&mut head).await.is_err() {
        return;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut location = None;
    let found = |kind: &'static str, body: Vec<u8>| ("200 OK", kind, body);
    let missing = ("404 Not Found", "text/plain", b"not here\n".to_vec());
    let (status, kind, body) = match method {
        "GET" | "HEAD" => match path {
            "/health" => found("text/plain", b"ok\n".to_vec()),
            "/stats.json" => found(
                "application/json",
                server.hub.lock().unwrap().report().to_string().into_bytes(),
            ),
            p if p.starts_with("/games/") && p.ends_with(".json") => {
                let name = &p["/games/".len()..p.len() - 5];
                match server.hub.lock().unwrap().games.get(name) {
                    Some(g) => found("application/json", g.bundle().to_string().into_bytes()),
                    None => missing.clone(),
                }
            }
            p => match &server.site {
                Some(site) if !p.split('/').any(|s| s == "..") => {
                    let mut file = site.join(p.trim_start_matches('/'));
                    if file.is_dir() && !p.ends_with('/') {
                        // A folder's page links relative to the folder: /play becomes /play/.
                        let q = if query.is_empty() {
                            String::new()
                        } else {
                            format!("?{query}")
                        };
                        location = Some(format!("/{}/{q}", p.trim_matches('/')));
                        ("301 Moved Permanently", "text/plain", b"moved\n".to_vec())
                    } else {
                        if file.is_dir() {
                            file = file.join("index.html");
                        }
                        match tokio::fs::read(&file).await {
                            Ok(b) => found(content_type(&file), b),
                            Err(_) => missing.clone(),
                        }
                    }
                }
                _ => missing.clone(),
            },
        },
        _ => ("405 Method Not Allowed", "text/plain", b"no\n".to_vec()),
    };
    let mut reply = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-cache\r\nX-Content-Type-Options: nosniff\r\n\
         Access-Control-Allow-Origin: *\r\nConnection: close\r\n{}\r\n",
        body.len(),
        location.map_or(String::new(), |l| format!("Location: {l}\r\n"))
    )
    .into_bytes();
    if method != "HEAD" {
        reply.extend_from_slice(&body);
    }
    let _ = stream.write_all(&reply).await;
    let _ = stream.shutdown().await;
}

// The handshake's refusal is tungstenite's own type, large as it is.
#[allow(clippy::result_large_err)]
async fn connection(stream: TcpStream, server: Arc<Server>, id: u64) {
    let head = tokio::time::timeout(Duration::from_secs(10), peek_head(&stream)).await;
    let Ok(Some((len, method, target, ws))) = head else {
        return;
    };
    if !ws {
        return http(stream, len, &method, &target, &server).await;
    }
    if target.split('?').next() != Some("/ws") {
        return;
    }
    let origins = server.origins.clone();
    let check = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
        match req.headers().get("origin").and_then(|o| o.to_str().ok()) {
            Some(o) if !allowed(o, &origins) => {
                let mut e = ErrorResponse::new(Some("this page may not connect".into()));
                *e.status_mut() = StatusCode::FORBIDDEN;
                Err(e)
            }
            _ => Ok(resp),
        }
    };
    let config = WebSocketConfig {
        max_message_size: Some(MAX_MESSAGE),
        max_frame_size: Some(MAX_MESSAGE),
        ..Default::default()
    };
    let Ok(ws) = tokio_tungstenite::accept_hdr_async_with_config(stream, check, Some(config)).await else {
        return;
    };
    let (mut sink, mut source) = ws.split();
    let (tx, mut rx) = unbounded_channel::<Message>();
    server.senders.lock().unwrap().insert(id, tx);
    server.hub.lock().unwrap().connect(id);
    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(Duration::from_secs(25));
        loop {
            tokio::select! {
                m = rx.recv() => match m {
                    Some(m) => if sink.send(m).await.is_err() { break },
                    None => break,
                },
                _ = ping.tick() => if sink.send(Message::Ping(Vec::new())).await.is_err() { break },
            }
        }
        let _ = sink.close().await;
    });
    let (mut allowance, mut last) = (BURST, Instant::now());
    while let Some(Ok(m)) = source.next().await {
        let text = match m {
            Message::Text(t) => t,
            Message::Close(_) => break,
            _ => continue,
        };
        let now = Instant::now();
        allowance = (allowance + now.duration_since(last).as_secs_f64() * RATE).min(BURST);
        last = now;
        if allowance < 1.0 {
            break; // flooding: goodbye
        }
        allowance -= 1.0;
        let out = match serde_json::from_str::<Value>(&text) {
            Ok(msg) => server.hub.lock().unwrap().handle(id, &msg, now),
            Err(_) => vec![(id, json!({ "t": "err", "msg": "not JSON" }))],
        };
        server.deliver(out);
    }
    let out = server.hub.lock().unwrap().disconnect(id);
    server.senders.lock().unwrap().remove(&id);
    server.deliver(out);
    writer.abort();
}

fn env(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

fn load_games(dir: &Path) -> BTreeMap<String, Arc<Game>> {
    let mut games = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        eprintln!("no games folder at {}", dir.display());
        return games;
    };
    for e in entries.flatten() {
        let path = e.path();
        if !path.join("game.toml").exists() {
            continue;
        }
        let name = e.file_name().to_string_lossy().to_string();
        match Game::from_dir(&path, &[]) {
            Ok(g) => {
                eprintln!("game {name}: {}", g.name);
                games.insert(name, Arc::new(g));
            }
            Err(err) => eprintln!("game {name}: {err}"),
        }
    }
    games
}

#[tokio::main]
async fn main() {
    let addr = env("OI_ADDR", "0.0.0.0:8080");
    let games = load_games(Path::new(&env("OI_GAMES", "games")));
    let data = env("OI_DATA", "data");
    let data = (!data.is_empty()).then(|| PathBuf::from(data));
    let site = std::env::var("OI_SITE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let origins: Vec<String> = env("OI_ORIGINS", "*")
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let mut hub = Hub::new(games, data.clone());
    hub.max_worlds = env("OI_MAX_WORLDS", "100").parse().unwrap_or(100);
    hub.max_players = env("OI_MAX_PLAYERS", "200").parse().unwrap_or(200);
    hub.load_all(Instant::now());
    let server = Arc::new(Server {
        hub: Mutex::new(hub),
        senders: Mutex::new(BTreeMap::new()),
        site,
        origins,
    });
    let listener = TcpListener::bind(&addr).await.expect("cannot listen");
    eprintln!(
        "openidle-server on {addr}; worlds in {}; site {}",
        data.as_ref().map_or("memory only".into(), |d| d.display().to_string()),
        server.site.as_ref().map_or("none".into(), |d| d.display().to_string()),
    );
    let clock = server.clone();
    tokio::spawn(async move {
        let mut every = tokio::time::interval(Duration::from_millis(100));
        let mut n: u64 = 0;
        loop {
            every.tick().await;
            n += 1;
            let out = clock.hub.lock().unwrap().clock(Instant::now());
            clock.deliver(out);
            if n % 300 == 0 {
                clock.hub.lock().unwrap().save_all(); // every thirty seconds
            }
        }
    });
    let next = AtomicU64::new(1);
    let mut term =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("cannot watch for signals");
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    let _ = stream.set_nodelay(true);
                    let id = next.fetch_add(1, Ordering::Relaxed);
                    tokio::spawn(connection(stream, server.clone(), id));
                }
            }
            _ = term.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    server.hub.lock().unwrap().save_all();
    eprintln!("openidle-server: worlds saved, stopping");
}
