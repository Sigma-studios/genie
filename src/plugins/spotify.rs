//! `/spotify`: the player's controls over MPRIS, and `/spotify name` searches.
//! The player is told what to play through MPRIS too, so playing doesn't go
//! through the Web API. Searching and `/queue` do: the first use opens a browser
//! tab to sign in (OAuth with PKCE, so no secret), and the tokens are kept after
//! that. Spotify only lets Premium accounts touch the queue.

use crate::config;
use crate::launch;
use crate::matcher;
use crate::plugin::Action;
use crate::plugin::Command as LauncherCommand;
use crate::plugin::Effect;
use crate::plugin::Icon;
use crate::plugin::Item;
use crate::plugin::Plugin;
use crate::plugin::Query;
use percent_encoding::NON_ALPHANUMERIC;
use percent_encoding::utf8_percent_encode;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use base64::Engine;
use sha2::Digest;
use sha2::Sha256;
use std::io::BufRead;
use std::io::BufReader;
use std::io::Read;
use std::io::Write;
use std::net::TcpListener;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::process::Command;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use std::time::SystemTime;
use std::time::UNIX_EPOCH;

/// Built in: no icon theme has it.
const ICON: &str = "genie-spotify";
const BUS: &str = "org.mpris.MediaPlayer2.spotify";
const OBJECT: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
/// Typing pauses this long before a search goes out.
const DEBOUNCE: Duration = Duration::from_millis(250);

const PORT: u16 = 47813;
const REDIRECT_URI: &str = "http://127.0.0.1:47813/callback";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const DASHBOARD: &str = "https://developer.spotify.com/dashboard";
/// How long the browser tab has to come back.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

/// Adding to the queue. Searching itself needs no scope.
const SCOPES: &str = "user-modify-playback-state";

pub struct Spotify {
    auth: Arc<Auth>,
}

struct Auth {
    client_id: String,
    tokens: Mutex<Option<Tokens>>,
    signing_in: Arc<AtomicBool>,
}

#[derive(Serialize, Deserialize, Clone)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    /// Unix time.
    expires_at: u64,
}

enum Failure {
    /// Nobody's signed in, or the sign-in was revoked.
    SignIn,
    Other(String),
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn tokens_path() -> PathBuf {
    config::data_home().join("genie/spotify.json")
}

fn load_tokens() -> Option<Tokens> {
    serde_json::from_str(&std::fs::read_to_string(tokens_path()).ok()?).ok()
}

/// Readable by the user alone: these give access to the Spotify account.
fn save_tokens(tokens: &Tokens) {
    let path = tokens_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path);
    let written = file.and_then(|mut f| f.write_all(serde_json::to_string(tokens).unwrap_or_default().as_bytes()));
    if let Err(e) = written {
        log::warn!("couldn't save Spotify's tokens: {e}");
    }
}

/// Spotify's token endpoint answers the same way to both uses. It can leave
/// out the refresh token when refreshing, which means keeping the old one.
fn parse_tokens(answer: &Value, previous_refresh: Option<&str>, now: u64) -> Option<Tokens> {
    Some(Tokens {
        access_token: answer["access_token"].as_str()?.to_string(),
        refresh_token: answer["refresh_token"].as_str().or(previous_refresh)?.to_string(),
        // A minute early, to never send one that's just expired.
        expires_at: now + answer["expires_in"].as_u64().unwrap_or(3600).saturating_sub(60),
    })
}

fn encode(text: &str) -> String {
    utf8_percent_encode(text, NON_ALPHANUMERIC).to_string()
}

fn random_text() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes)).map_err(|e| e.to_string())?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn authorize_url(client_id: &str, challenge: &str, state: &str) -> String {
    format!(
        "https://accounts.spotify.com/authorize?response_type=code&client_id={}&redirect_uri={}&scope={}\
         &code_challenge_method=S256&code_challenge={challenge}&state={state}",
        encode(client_id),
        encode(REDIRECT_URI),
        encode(SCOPES),
    )
}

/// From the request line `GET /callback?code=…&state=… HTTP/1.1`, the code if
/// this is the answer to our state. `Err` is a refusal, `Ok(None)` is anything
/// else: a browser also asks for its favicon.
fn parse_callback(request_line: &str, state: &str) -> Result<Option<String>, String> {
    let Some(target) = request_line.split_whitespace().nth(1) else { return Ok(None) };
    let Some(query) = target.strip_prefix("/callback?") else { return Ok(None) };
    let get = |key: &str| query.split('&').find_map(|pair| pair.strip_prefix(key)?.strip_prefix('='));
    if get("state") != Some(state) {
        return Ok(None);
    }
    if let Some(error) = get("error") {
        return Err(error.to_string());
    }
    Ok(get("code").map(String::from))
}

fn respond(mut stream: &std::net::TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Génie</title>\
         <body style=\"font:18px sans-serif;background:#202326;color:#fcfcfc;text-align:center;padding-top:20vh\">{message}"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

/// Waits for the browser to come back to our loopback address.
fn wait_for_code(listener: &TcpListener, state: &str) -> Result<String, String> {
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + SIGN_IN_TIMEOUT;
    while std::time::Instant::now() < deadline {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut line = String::new();
        let _ = BufReader::new(&stream).read_line(&mut line);
        match parse_callback(&line, state) {
            Ok(Some(code)) => {
                respond(&stream, "Signed in to Spotify. You can close this tab and search from Génie.");
                return Ok(code);
            }
            Ok(None) => respond(&stream, ""),
            Err(error) => {
                respond(&stream, "Spotify didn't sign you in. Searching from Génie again will retry.");
                return Err(error);
            }
        }
    }
    Err("timed out".to_string())
}

fn request_tokens(form: &[(&str, &str)], previous_refresh: Option<&str>) -> Result<Tokens, Failure> {
    let result = ureq::post(TOKEN_URL).send_form(form.iter().copied());
    let mut response = match result {
        Ok(response) => response,
        // The refresh token was revoked.
        Err(ureq::Error::StatusCode(400)) => return Err(Failure::SignIn),
        Err(e) => return Err(Failure::Other(e.to_string())),
    };
    let answer: Value = response.body_mut().read_json().map_err(|e| Failure::Other(e.to_string()))?;
    parse_tokens(&answer, previous_refresh, unix_now()).ok_or_else(|| Failure::Other("unexpected answer from Spotify".into()))
}

impl Spotify {
    pub fn new(credentials: config::Spotify) -> Self {
        let auth = Auth { client_id: credentials.client_id, tokens: Mutex::new(load_tokens()), signing_in: Arc::new(AtomicBool::new(false)) };
        Self { auth: Arc::new(auth) }
    }
}

impl Auth {
    fn token(&self) -> Result<String, Failure> {
        let mut tokens = self.tokens.lock().unwrap();
        // The sign-in thread saves to the file: it may know better.
        if tokens.is_none() {
            *tokens = load_tokens();
        }
        let current = tokens.as_ref().ok_or(Failure::SignIn)?;
        if unix_now() < current.expires_at {
            return Ok(current.access_token.clone());
        }
        let refresh = current.refresh_token.clone();
        let form = [("grant_type", "refresh_token"), ("refresh_token", refresh.as_str()), ("client_id", self.client_id.as_str())];
        match request_tokens(&form, Some(&refresh)) {
            Ok(fresh) => {
                save_tokens(&fresh);
                let access = fresh.access_token.clone();
                *tokens = Some(fresh);
                Ok(access)
            }
            Err(Failure::SignIn) => {
                *tokens = None;
                let _ = std::fs::remove_file(tokens_path());
                Err(Failure::SignIn)
            }
            Err(other) => Err(other),
        }
    }

    /// Opens the tab, once, and finishes in the background. `Err` if that can't
    /// even start.
    fn start_sign_in(&self) -> Result<(), String> {
        if self.signing_in.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        let started = (|| {
            let listener = TcpListener::bind(("127.0.0.1", PORT)).map_err(|e| format!("port {PORT}: {e}"))?;
            let verifier = random_text()?;
            let state = random_text()?;
            Ok::<_, String>((listener, verifier, state))
        })();
        let (listener, verifier, state) = match started {
            Ok(started) => started,
            Err(error) => {
                self.signing_in.store(false, Ordering::SeqCst);
                return Err(error);
            }
        };
        launch::open(&authorize_url(&self.client_id, &challenge(&verifier), &state), None);
        let (client_id, signing_in) = (self.client_id.clone(), self.signing_in.clone());
        std::thread::spawn(move || {
            let result = wait_for_code(&listener, &state).and_then(|code| {
                let form = [
                    ("grant_type", "authorization_code"),
                    ("code", code.as_str()),
                    ("redirect_uri", REDIRECT_URI),
                    ("client_id", client_id.as_str()),
                    ("code_verifier", verifier.as_str()),
                ];
                request_tokens(&form, None).map_err(|failure| match failure {
                    Failure::SignIn => "Spotify refused the code".to_string(),
                    Failure::Other(error) => error,
                })
            });
            match result {
                Ok(tokens) => save_tokens(&tokens),
                Err(error) => log::warn!("signing in to Spotify: {error}"),
            }
            signing_in.store(false, Ordering::SeqCst);
        });
        Ok(())
    }

    /// `types` is what Spotify's `type` parameter takes.
    fn search(&self, text: &str, types: &str) -> Result<Vec<Item>, Failure> {
        let url = format!("https://api.spotify.com/v1/search?type={types}&limit=5&q={}", encode(text));
        let token = self.token()?;
        let mut response = match ureq::get(&url).header("Authorization", format!("Bearer {token}")).call() {
            Ok(response) => response,
            // Expired early, or revoked: the next search refreshes or signs in.
            Err(ureq::Error::StatusCode(401)) => {
                if let Some(tokens) = self.tokens.lock().unwrap().as_mut() {
                    tokens.expires_at = 0;
                }
                return Err(Failure::Other("Spotify refused the token. Try again.".into()));
            }
            Err(e) => return Err(Failure::Other(e.to_string())),
        };
        let answer: Value = response.body_mut().read_json().map_err(|e| Failure::Other(e.to_string()))?;
        Ok(parse_results(&answer))
    }

    /// Queues it, or plays it if there's nothing to queue after: Spotify isn't
    /// running or has stopped. Paused still queues.
    /// Says how it went with a notification: the launcher has hidden by then.
    fn add_to_queue(&self, uri: &str, title: &str) {
        if idle() {
            open_uri(uri);
            return launch::notify(&format!("Playing {title}."));
        }
        let token = match self.token() {
            Ok(token) => token,
            Err(_) => return launch::notify("Sign in to Spotify first: search for something with /spotify."),
        };
        let url = format!("https://api.spotify.com/v1/me/player/queue?uri={}", encode(uri));
        match ureq::post(&url).header("Authorization", format!("Bearer {token}")).send_empty() {
            Ok(_) => launch::notify(&format!("{title} is next in the queue.")),
            // The player has something loaded, but Spotify knows no active device.
            Err(ureq::Error::StatusCode(404)) => {
                open_uri(uri);
                launch::notify(&format!("Playing {title}."));
            }
            Err(ureq::Error::StatusCode(403)) => launch::notify("Spotify refused: the queue takes a Premium account."),
            Err(e) => launch::notify(&format!("Couldn't add to the queue: {e}")),
        }
    }
}

/// Tracks first: it's what's usually looked for. Each section keeps Spotify's
/// own order.
fn parse_results(answer: &Value) -> Vec<Item> {
    let mut items = Vec::new();
    let sections = [("tracks", "Track", 0.8), ("artists", "Artist", 0.7), ("albums", "Album", 0.6), ("playlists", "Playlist", 0.5)];
    for (section, label, base) in sections {
        for (i, entry) in answer[section]["items"].as_array().map_or(&[][..], Vec::as_slice).iter().enumerate() {
            // Spotify leaves nulls in playlist results.
            let (Some(uri), Some(name)) = (entry["uri"].as_str(), entry["name"].as_str()) else { continue };
            let subtitle = match section {
                "tracks" => format!("{label} · {} · {}", artists(entry), entry["album"]["name"].as_str().unwrap_or("")),
                "albums" => format!("{label} · {}", artists(entry)),
                "playlists" => format!("{label} · {}", entry["owner"]["display_name"].as_str().unwrap_or("")),
                _ => label.to_string(),
            };
            let images = if section == "tracks" { &entry["album"]["images"] } else { &entry["images"] };
            let icon = images
                .as_array()
                .and_then(|images| images.last())
                .and_then(|image| image["url"].as_str())
                .map_or_else(|| Icon::Theme(ICON.into()), |url| Icon::Url(url.to_string()));
            let play = uri.to_string();
            let link = format!("https://open.spotify.com/{}", uri.trim_start_matches("spotify:").replace(':', "/"));
            items.push(
                Item::new(uri, name, subtitle, icon)
                    .action(Action::new("play", "Play", Effect::Call(Arc::new(move || open_uri(&play)))))
                    .action(Action::new("copy", "Copy the link", Effect::Copy(link)))
                    .score(base - i as f32 * 0.01),
            );
        }
    }
    items
}

fn artists(entry: &Value) -> String {
    entry["artists"]
        .as_array()
        .map(|artists| artists.iter().filter_map(|a| a["name"].as_str()).collect::<Vec<_>>().join(", "))
        .unwrap_or_default()
}

fn busctl(arguments: &[&str]) -> Option<String> {
    let output = Command::new("busctl")
        .args(["--user", "--timeout=1"])
        .args(arguments)
        .env_remove("LD_LIBRARY_PATH")
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn player_call(method: &str) {
    busctl(&["call", BUS, OBJECT, PLAYER, method]);
}

/// Not running, which busctl fails on, or stopped.
fn idle() -> bool {
    busctl(&["-j", "get-property", BUS, OBJECT, PLAYER, "PlaybackStatus"]).is_none_or(|answer| is_stopped(&answer))
}

/// From busctl's JSON.
fn is_stopped(answer: &str) -> bool {
    serde_json::from_str::<Value>(answer).is_ok_and(|status| status["data"] == "Stopped")
}

fn running() -> bool {
    busctl(&["-j", "get-property", BUS, OBJECT, PLAYER, "PlaybackStatus"]).is_some()
}

/// Starts Spotify if it isn't running.
fn open_uri(uri: &str) {
    if busctl(&["call", BUS, OBJECT, PLAYER, "OpenUri", "s", uri]).is_none() {
        launch::spawn(&["spotify".to_string(), format!("--uri={uri}")], None, "spotify", None);
    }
}

/// `Playing · Title · Artist` of the current track, from busctl's JSON.
fn now_playing() -> Option<String> {
    let status = busctl(&["-j", "get-property", BUS, OBJECT, PLAYER, "PlaybackStatus"])?;
    let metadata = busctl(&["-j", "get-property", BUS, OBJECT, PLAYER, "Metadata"])?;
    let status: Value = serde_json::from_str(&status).ok()?;
    let metadata: Value = serde_json::from_str(&metadata).ok()?;
    describe(status["data"].as_str()?, &metadata["data"])
}

fn describe(status: &str, metadata: &Value) -> Option<String> {
    let title = metadata["xesam:title"]["data"].as_str().filter(|t| !t.is_empty())?;
    let artists = metadata["xesam:artist"]["data"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    Some(format!("{status} · {title} · {artists}"))
}

/// Back to 0:00. `SetPosition` wants the track's id, which older Spotify
/// builds give as `spotify:track:…`, not an object path busctl will send:
/// then seeking far back, which MPRIS stops at the start.
fn restart_track() {
    let track = busctl(&["-j", "get-property", BUS, OBJECT, PLAYER, "Metadata"]).and_then(|answer| track_id(&answer));
    if track.is_some_and(|track| busctl(&["call", BUS, OBJECT, PLAYER, "SetPosition", "ox", &track, "0"]).is_some()) {
        return;
    }
    busctl(&["call", BUS, OBJECT, PLAYER, "Seek", "x", "-86400000000"]);
}

/// The current track's object path, from busctl's JSON of `Metadata`.
fn track_id(answer: &str) -> Option<String> {
    let metadata: Value = serde_json::from_str(answer).ok()?;
    let id = metadata["data"]["mpris:trackid"]["data"].as_str()?;
    id.starts_with('/').then(|| id.to_string())
}

fn restart(subtitle: &str, score: f32) -> Item {
    Item::new("restart", "Restart the track", subtitle, Icon::Theme("media-seek-backward".into()))
        .action(Action::new("run", "Restart the track", Effect::Call(Arc::new(restart_track))))
        .score(score)
}

fn control(id: &str, title: &str, subtitle: &str, icon: &str, method: &'static str, score: f32) -> Item {
    Item::new(id, title, subtitle, Icon::Theme(icon.into()))
        .action(Action::new("run", title, Effect::Call(Arc::new(move || player_call(method)))))
        .score(score)
}

impl Spotify {
    /// Results for `/spotify` or `/queue`, signing in if need be.
    fn search_results(&self, query: &Query, text: &str, queue: bool) {
        if self.auth.client_id.is_empty() {
            let subtitle = format!("Create an app with the redirect URI {REDIRECT_URI}, then set [spotify] client_id in ~/.config/genie/config.toml");
            query.send(vec![
                warning("nocredentials", "Spotify search isn't set up", &subtitle)
                    .action(Action::new("open", "Open Spotify's dashboard", Effect::Open(DASHBOARD.into()))),
            ]);
            return;
        }
        std::thread::sleep(DEBOUNCE);
        if query.cancelled() {
            return;
        }
        match self.auth.search(text, if queue { "track" } else { "track,artist,album,playlist" }) {
            Ok(items) if queue => query.send(items.into_iter().map(|item| self.queueable(item)).collect()),
            Ok(items) => query.send(items),
            Err(Failure::SignIn) => match self.auth.start_sign_in() {
                Ok(()) => query.send(vec![warning("signin", "Signing in to Spotify", "Continue in your browser, then search again")]),
                Err(error) => query.send(vec![warning("error", "Couldn't start signing in to Spotify", &error)]),
            },
            Err(Failure::Other(error)) => query.send(vec![warning("error", "Spotify search failed", &error)]),
        }
    }

    /// Enter adds it to the queue, and playing moves to Alt's list.
    fn queueable(&self, mut item: Item) -> Item {
        let (auth, uri, title) = (self.auth.clone(), item.id.clone(), item.title.clone());
        let queue = Effect::Call(Arc::new(move || auth.add_to_queue(&uri, &title)));
        item.actions.insert(0, Action::new("queue", "Add to the queue, or play it", queue));
        item
    }
}

impl Plugin for Spotify {
    fn id(&self) -> &'static str {
        "spotify"
    }

    fn group(&self) -> &'static str {
        "Spotify"
    }

    fn commands(&self) -> Vec<LauncherCommand> {
        vec![
            LauncherCommand::new("spotify", &["sp", "play"], "Control the music, or search and play", ICON),
            LauncherCommand::new("queue", &[], "Search songs, and queue one or play it if nothing is playing", "media-playlist-append"),
            LauncherCommand::new("pause", &[], "Play or pause the music", "media-playback-pause"),
            LauncherCommand::new("next", &[], "Skip to the next track", "media-skip-forward"),
            LauncherCommand::new("previous", &[], "Back to the previous track", "media-skip-backward"),
            LauncherCommand::new("track-restart", &[], "Play the track again from the start", "media-seek-backward"),
        ]
    }

    fn query(&self, query: &Query) {
        let text = query.text.trim();
        match query.command.as_deref() {
            Some("pause") => query.send(vec![control("playpause", "Play / Pause", &now_playing().unwrap_or_default(), "media-playback-start", "PlayPause", 1.0)]),
            Some("next") => query.send(vec![control("next", "Next track", &now_playing().unwrap_or_default(), "media-skip-forward", "Next", 1.0)]),
            Some("previous") => query.send(vec![control("previous", "Previous track", &now_playing().unwrap_or_default(), "media-skip-backward", "Previous", 1.0)]),
            Some("track-restart") => query.send(vec![restart(&now_playing().unwrap_or_default(), 1.0)]),
            Some("queue") if text.is_empty() => {
                query.send(vec![Item::new("queue.hint", "Add to the queue", "Type a song's name", Icon::Theme("media-playlist-append".into())).completion("/queue ")]);
            }
            Some("queue") => self.search_results(query, text, true),
            _ => {
                if running() {
                    let playing = now_playing().unwrap_or_default();
                    let controls = [
                        control("playpause", "Play / Pause", &playing, "media-playback-start", "PlayPause", 1.03),
                        control("next", "Next track", "", "media-skip-forward", "Next", 1.02),
                        control("previous", "Previous track", "", "media-skip-backward", "Previous", 1.01),
                        restart("", 1.005),
                    ];
                    query.send(
                        controls
                            .into_iter()
                            .filter(|item| text.is_empty() || matcher::score(text, &[(&item.title, 1.0)]).is_some())
                            .collect(),
                    );
                } else if text.is_empty() {
                    let open = Effect::Spawn { argv: vec!["spotify".into()], cwd: None, id: "spotify".into() };
                    query.send(vec![
                        Item::new("start", "Start Spotify", "", Icon::Theme(ICON.into()))
                            .action(Action::new("run", "Start", open))
                            .score(1.0),
                    ]);
                }
                if !text.is_empty() {
                    self.search_results(query, text, false);
                }
            }
        }
    }
}

fn warning(id: &str, title: &str, subtitle: &str) -> Item {
    Item::new(id, title, subtitle, Icon::Theme("dialog-warning".into())).score(0.1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn challenge_matches_the_rfc_example() {
        // RFC 7636, appendix B.
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    #[test]
    fn authorize_url_has_what_spotify_wants() {
        let url = authorize_url("id", "ch", "st");
        assert!(url.starts_with("https://accounts.spotify.com/authorize?response_type=code&client_id=id&"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127%2E0%2E0%2E1%3A47813%2Fcallback"));
        assert!(url.contains("scope=user%2Dmodify%2Dplayback%2Dstate"));
        assert!(url.contains("code_challenge_method=S256&code_challenge=ch&state=st"));
        assert!(!url.contains(char::is_whitespace));
    }

    #[test]
    fn callbacks_are_checked() {
        let line = |target: &str| format!("GET {target} HTTP/1.1\r\n");
        assert_eq!(parse_callback(&line("/callback?code=abc&state=st"), "st"), Ok(Some("abc".to_string())));
        assert_eq!(parse_callback(&line("/callback?state=st&error=access_denied"), "st"), Err("access_denied".to_string()));
        // Someone else's request isn't an answer, nor a refusal.
        assert_eq!(parse_callback(&line("/callback?code=abc&state=other"), "st"), Ok(None));
        assert_eq!(parse_callback(&line("/callback?error=x&state=other"), "st"), Ok(None));
        assert_eq!(parse_callback(&line("/favicon.ico"), "st"), Ok(None));
        assert_eq!(parse_callback("", "st"), Ok(None));
    }

    #[test]
    fn refreshing_can_keep_the_old_refresh_token() {
        let answer = json!({"access_token": "a", "expires_in": 3600});
        let tokens = parse_tokens(&answer, Some("old"), 1000).unwrap();
        assert_eq!((tokens.refresh_token.as_str(), tokens.expires_at), ("old", 1000 + 3540));
        assert!(parse_tokens(&answer, None, 1000).is_none());
        let rotated = json!({"access_token": "a", "refresh_token": "new"});
        assert_eq!(parse_tokens(&rotated, Some("old"), 0).unwrap().refresh_token, "new");
    }

    #[test]
    fn results_are_ordered_and_nulls_skipped() {
        let answer = json!({
            "tracks": {"items": [{
                "uri": "spotify:track:1", "name": "Song",
                "artists": [{"name": "A"}, {"name": "B"}],
                "album": {"name": "Disc", "images": [{"url": "big"}, {"url": "small"}]},
            }]},
            "artists": {"items": [{"uri": "spotify:artist:2", "name": "A", "images": []}]},
            "playlists": {"items": [null]},
        });
        let items = parse_results(&answer);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].subtitle, "Track · A, B · Disc");
        assert_eq!(items[0].icon, Icon::Url("small".into()));
        assert_eq!(items[1].icon, Icon::Theme(ICON.into()));
        assert!(items[0].score > items[1].score);
    }

    #[test]
    fn only_stopped_counts_as_stopped() {
        assert!(is_stopped(r#"{"type":"s","data":"Stopped"}"#));
        assert!(!is_stopped(r#"{"type":"s","data":"Paused"}"#));
        assert!(!is_stopped(r#"{"type":"s","data":"Playing"}"#));
        assert!(!is_stopped("garbage"));
    }

    #[test]
    fn track_ids_must_be_object_paths() {
        let answer = |id: &str| json!({"type": "a{sv}", "data": {"mpris:trackid": {"type": "o", "data": id}}}).to_string();
        assert_eq!(track_id(&answer("/com/spotify/track/abc")).as_deref(), Some("/com/spotify/track/abc"));
        assert!(track_id(&answer("spotify:track:abc")).is_none());
        assert!(track_id(r#"{"type":"a{sv}","data":{}}"#).is_none());
        assert!(track_id("garbage").is_none());
    }

    #[test]
    fn now_playing_reads_busctl_json() {
        let metadata = json!({
            "xesam:title": {"type": "s", "data": "Song"},
            "xesam:artist": {"type": "as", "data": ["A", "B"]},
        });
        assert_eq!(describe("Playing", &metadata).unwrap(), "Playing · Song · A, B");
        assert_eq!(describe("Paused", &metadata).unwrap(), "Paused · Song · A, B");
        assert!(describe("Stopped", &json!({})).is_none());
    }
}
