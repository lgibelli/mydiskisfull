//! Sign in with ChatGPT, for Ask AI ([`crate::ask`]): a question goes from
//! this Mac to OpenAI, on the user's own ChatGPT plan (Plus or Pro), with no
//! server of ours in between. OpenAI's flow for open-source apps that run
//! locally (developers.openai.com/siwc/token-sharing-open-source): OAuth 2
//! with PKCE in the browser, back to a listener on 127.0.0.1 that takes the
//! one request, MyDiskIsFull registered by OpenAI at the first sign-in (the
//! issued client id, kept after signing out, as the host id is). All of it
//! is kept in `chatgpt.json` beside the history, readable by its owner only
//! and out of Time Machine. Requests go through curl, given their tokens on
//! its standard input: arguments show in `ps`.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::watch::unix_now;

/// Where OpenAI is: [`Endpoints::openai`], or for the tests a stand-in on
/// this Mac.
#[derive(Debug, Clone)]
pub struct Endpoints {
    /// Signing in (`/api/accounts/…`); also the ID token's issuer.
    pub auth: String,
    /// The models and the Responses API.
    pub api: String,
}

impl Endpoints {
    pub fn openai() -> Self {
        Endpoints {
            auth: "https://auth.openai.com".into(),
            api: "https://api.openai.com/v1".into(),
        }
    }
}

const FILE: &str = "chatgpt.json";
const LOCK: &str = ".chatgpt.lock";
/// What OpenAI shows at the first sign-in.
const NAME: &str = "MyDiskIsFull";
/// The client id of a first sign-in: OpenAI issues ours in its answer.
const FIRST: &str = "dynamic_agent_client";
const SCOPE: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// What lets a request use the plan: being signed in alone doesn't.
const PLAN: &str = "chatgpt.tokens.use.direct";
const RESOURCE: &str = "https://api.openai.com/v1";
/// Where people see, and limit, what apps use of their plan.
pub const USAGE: &str = "https://chatgpt.com/settings/usage";
/// An access token is renewed when it has less than this left.
const EARLY: i64 = 300;
/// Refreshes that end the sign-in: the refresh token is spent or revoked.
const ENDED: &[&str] = &[
    "invalid_grant",
    "invalid_refresh_token",
    "token_expired",
    "refresh_token_expired",
    "refresh_token_invalidated",
    "refresh_token_reused",
];
const SIGNED_OUT: &str =
    "not signed in with ChatGPT: sign in first (in Settings, or `mdif knowledge sign-in`)";
const ENDED_SAY: &str = "the sign-in with ChatGPT has ended: sign in again";
/// The usage-limit error, said as OpenAI asks it to be.
pub const LIMIT: &str =
    "Usage limit reached. Review your plan or this app's limit in ChatGPT settings";
const FORM: &str = "application/x-www-form-urlencoded";

/// What's kept.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Saved {
    /// This Mac, to OpenAI: chosen once, opaque.
    host: String,
    /// The registration OpenAI issued at the first sign-in (`oaiapp_…`).
    client: Option<String>,
    /// Whose it is: `sub` is the account, the e-mail what's shown.
    sub: Option<String>,
    email: Option<String>,
    /// The last sign-in's ID token, a hint at the next one.
    id_token: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Tokens {
    access: String,
    refresh: String,
    /// When the access token expires (Unix seconds).
    expires: i64,
    /// Not refreshed before this (Unix seconds; 0: whenever).
    #[serde(default)]
    earliest_refresh: i64,
    scopes: Vec<String>,
}

/// Who's signed in, as the app shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Account {
    pub signed_in: bool,
    pub email: Option<String>,
    /// Whether ChatGPT lets requests use the plan.
    pub plan: bool,
    /// Where to manage what apps use of it.
    pub usage: &'static str,
}

fn load(dir: &Path) -> Saved {
    std::fs::read(dir.join(FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Atomically, readable by its owner only, and kept out of the backups
/// (the mark is the file's, so each new one gets it).
fn save(dir: &Path, s: &Saved) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{FILE}.new"));
    let _ = std::fs::remove_file(&tmp);
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(s)?)?;
    f.sync_all()?;
    std::fs::rename(&tmp, dir.join(FILE))?;
    let _ = mdif_macos::fs::set_excluded_from_backup(&dir.join(FILE), true);
    Ok(())
}

/// `f` while no other process does: a refresh changes the refresh token,
/// and two at once would spend it twice, which ends the sign-in.
fn locked<T>(dir: &Path, f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::fs::create_dir_all(dir)?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join(LOCK))?;
    mdif_macos::fs::lock(&lock)?;
    f()
}

pub fn account(dir: &Path) -> Account {
    let s = load(dir);
    Account {
        signed_in: s.tokens.is_some(),
        email: s.tokens.as_ref().and(s.email),
        plan: s.tokens.is_some_and(|t| t.scopes.iter().any(|x| x == PLAN)),
        usage: USAGE,
    }
}

fn random(n: usize) -> Result<Vec<u8>> {
    let mut b = vec![0; n];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b)
}

fn secret(n: usize) -> Result<String> {
    Ok(URL_SAFE_NO_PAD.encode(random(n)?))
}

/// A random UUID, as OpenAI's examples write a host id.
fn host_id() -> Result<String> {
    let mut b = random(16)?;
    b[6] = b[6] & 0x0f | 0x40;
    b[8] = b[8] & 0x3f | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!(
        "urn:uuid:{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}

/// PKCE's S256: what's sent first for the verifier sent last.
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode(k), decode(v))
        })
        .collect()
}

/// A string for curl's configuration, between its double quotes.
fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

struct Reply {
    status: u16,
    body: String,
}

/// One HTTPS request through curl, everything but the options on its
/// standard input. Plain HTTP only to the tests' stand-in on 127.0.0.1.
fn http(url: &str, bearer: Option<&str>, body: Option<(&str, &str)>, secs: u64) -> Result<Reply> {
    let mut cfg = String::from("silent\nshow-error\n");
    let mut set = |k: &str, v: &str| cfg.push_str(&format!("{k} = \"{}\"\n", quoted(v)));
    set("url", url);
    let local = url.starts_with("http://127.0.0.1:");
    set("proto", if local { "=http" } else { "=https" });
    set("max-time", &secs.to_string());
    set("max-filesize", "4000000");
    set("write-out", "\n%{http_code}");
    if let Some(t) = bearer {
        set("header", &format!("Authorization: Bearer {t}"));
    }
    if let Some((kind, data)) = body {
        // No "Expect: 100-continue" round trip before a body.
        set("header", "Expect:");
        set("header", &format!("Content-Type: {kind}"));
        set("data-binary", data);
    }
    let mut curl = Command::new("/usr/bin/curl");
    curl.args(["--config", "-"]);
    let out = crate::space::run_fed(curl, Some(cfg.into_bytes()), Duration::from_secs(secs + 10))
        .ok_or_else(|| anyhow!("OpenAI didn't answer in time"))?;
    if !out.status.success() {
        bail!(
            "couldn't reach OpenAI: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let (body, code) = text.rsplit_once('\n').unwrap_or(("", &text));
    Ok(Reply {
        status: code.trim().parse().unwrap_or(0),
        body: body.to_string(),
    })
}

/// The error's code and the field it's about, as OpenAI says them
/// (`{"error": {"code", "param"}}`), or as OAuth does (`{"error": "…"}`).
fn error_of(body: &str) -> (Option<String>, Option<String>) {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = &v["error"];
    let code = e["code"]
        .as_str()
        .or(e.as_str())
        .or(v["code"].as_str())
        .map(String::from);
    (code, e["param"].as_str().map(String::from))
}

// ---------------------------------------------------------------------------
// Signing in.

fn authorize_url(
    ends: &Endpoints,
    s: &Saved,
    redirect: &str,
    state: &str,
    nonce: &str,
    challenge: &str,
) -> String {
    let mut p: Vec<(&str, &str)> = vec![("client_id", s.client.as_deref().unwrap_or(FIRST))];
    match &s.client {
        None => p.push(("agent_name_hint", NAME)),
        Some(_) => {
            if let Some(t) = &s.id_token {
                p.push(("id_token_hint", t));
            }
            if let Some(e) = &s.email {
                p.push(("login_hint", e));
            }
        }
    }
    p.extend([
        ("ext_agent_host_id", s.host.as_str()),
        ("response_type", "code"),
        ("redirect_uri", redirect),
        ("scope", SCOPE),
        ("resource", RESOURCE),
        ("state", state),
        ("nonce", nonce),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ]);
    format!("{}/api/accounts/authorize?{}", ends.auth, form(&p))
}

#[derive(Debug)]
struct Callback {
    code: String,
    client: Option<String>,
    scope: Option<String>,
}

const DONE: &str = "Signed in to ChatGPT. You can close this tab and go back to MyDiskIsFull.";

fn page(stream: &mut TcpStream, status: &str, text: &str) {
    let html = format!(
        "<!doctype html><meta charset=\"utf-8\"><title>MyDiskIsFull</title>\
         <body style=\"font: 15px -apple-system, sans-serif; margin: 5em auto; max-width: 32em; text-align: center\">\
         <p>{text}</p></body>"
    );
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}",
        html.len()
    );
}

/// One request to the listener. The callback with this attempt's state ends
/// the wait; anything else is answered and waited past.
fn take(mut stream: TcpStream, state: &str) -> Option<Result<Callback>> {
    stream.set_nonblocking(false).ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut head = Vec::new();
    let mut buf = [0; 2048];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16_384 {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => head.extend_from_slice(&buf[..n]),
        }
    }
    let head = String::from_utf8_lossy(&head);
    let mut line = head.lines().next().unwrap_or("").split(' ');
    let target = (line.next() == Some("GET")).then(|| line.next()).flatten();
    let (path, q) = target.map_or(("", ""), |t| t.split_once('?').unwrap_or((t, "")));
    if path != "/auth/callback" {
        page(&mut stream, "404 Not Found", "Nothing here.");
        return None;
    }
    let q = query(q);
    if q.get("state").map(String::as_str) != Some(state) {
        page(
            &mut stream,
            "400 Bad Request",
            "This isn't the sign-in MyDiskIsFull started.",
        );
        return None;
    }
    if let Some(e) = q.get("error") {
        page(
            &mut stream,
            "200 OK",
            "Not signed in. You can close this tab.",
        );
        return Some(Err(if e == "access_denied" {
            anyhow!("signing in was cancelled")
        } else {
            anyhow!("OpenAI didn't sign you in ({e})")
        }));
    }
    let Some(code) = q.get("code").filter(|c| !c.is_empty()) else {
        page(&mut stream, "400 Bad Request", "OpenAI sent no code back.");
        return Some(Err(anyhow!("OpenAI sent no code back: try again")));
    };
    page(&mut stream, "200 OK", DONE);
    Some(Ok(Callback {
        code: code.clone(),
        client: q.get("client_id").cloned(),
        scope: q.get("scope").cloned(),
    }))
}

fn wait_back(listener: &TcpListener, state: &str, wait: Duration) -> Result<Callback> {
    listener.set_nonblocking(true)?;
    let until = Instant::now() + wait;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if let Some(r) = take(stream, state) {
                    return r;
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= until {
                    bail!(
                        "the sign-in wasn't finished within {} minutes",
                        wait.as_secs() / 60
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    expires_in: Option<i64>,
    scope: Option<String>,
    earliest_refresh_at: Option<Value>,
}

impl TokenReply {
    fn tokens(
        self,
        refresh: Option<String>,
        scopes: Option<Vec<String>>,
        now: i64,
    ) -> Result<Tokens> {
        Ok(Tokens {
            access: self.access_token,
            refresh: self
                .refresh_token
                .or(refresh)
                .ok_or_else(|| anyhow!("OpenAI gave no refresh token"))?,
            expires: now + self.expires_in.unwrap_or(3600),
            earliest_refresh: self
                .earliest_refresh_at
                .as_ref()
                .and_then(Value::as_i64)
                .unwrap_or(0),
            scopes: self
                .scope
                .map(|s| s.split_whitespace().map(String::from).collect())
                .or(scopes)
                .unwrap_or_default(),
        })
    }
}

struct Claims {
    sub: String,
    email: Option<String>,
}

/// The ID token's claims, checked: from OpenAI, for MyDiskIsFull's
/// registration, current, and this attempt's. Its signature isn't: it came
/// straight from the token endpoint over TLS, which says it's OpenAI's
/// (OpenID Connect Core, 3.1.3.7).
fn claims(jwt: &str, issuer: &str, client: &str, nonce: &str, now: i64) -> Result<Claims> {
    let bad = || anyhow!("OpenAI's sign-in answer didn't check out: try again");
    let payload = jwt.split('.').nth(1).ok_or_else(bad)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| bad())?;
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| bad())?;
    let aud = &v["aud"];
    let ours = aud.as_str() == Some(client)
        || aud
            .as_array()
            .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(client)));
    let fresh = v["exp"].as_i64().is_some_and(|e| e > now - 60);
    if v["iss"].as_str() != Some(issuer) || !ours || !fresh || v["nonce"].as_str() != Some(nonce) {
        return Err(bad());
    }
    let sub = v["sub"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(bad)?;
    Ok(Claims {
        sub: sub.to_string(),
        email: v["email"].as_str().map(String::from),
    })
}

/// Signs in: opens OpenAI's page with `open` and waits up to `wait` for the
/// browser to come back.
pub fn sign_in(
    dir: &Path,
    ends: &Endpoints,
    open: &dyn Fn(&str) -> Result<()>,
    wait: Duration,
) -> Result<Account> {
    let mut saved = load(dir);
    if saved.host.is_empty() {
        saved.host = host_id()?;
        locked(dir, || save(dir, &saved))?;
    }
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .context("couldn't wait for the browser to come back")?;
    let redirect = format!(
        "http://127.0.0.1:{}/auth/callback",
        listener.local_addr()?.port()
    );
    let (verifier, state, nonce) = (secret(48)?, secret(24)?, secret(24)?);
    open(&authorize_url(
        ends,
        &saved,
        &redirect,
        &state,
        &nonce,
        &challenge(&verifier),
    ))?;
    let back = wait_back(&listener, &state, wait)?;
    drop(listener);
    let client = match (&saved.client, back.client) {
        (None, Some(c)) if !c.is_empty() => c,
        (None, _) => bail!("OpenAI didn't say how it registered MyDiskIsFull: try again"),
        (Some(c), Some(b)) if *c != b => bail!("OpenAI answered for another app: try again"),
        (Some(c), _) => c.clone(),
    };
    let r = http(
        &format!("{}/api/accounts/oauth/token", ends.auth),
        None,
        Some((
            FORM,
            &form(&[
                ("grant_type", "authorization_code"),
                ("client_id", &client),
                ("code", &back.code),
                ("code_verifier", &verifier),
                ("redirect_uri", &redirect),
                ("resource", RESOURCE),
            ]),
        )),
        60,
    )?;
    if r.status != 200 {
        let (code, _) = error_of(&r.body);
        bail!(
            "OpenAI didn't finish the sign-in ({}): try again",
            code.unwrap_or_else(|| r.status.to_string())
        );
    }
    let reply: TokenReply =
        serde_json::from_str(&r.body).context("OpenAI's sign-in answer was unexpected")?;
    let id_token = reply
        .id_token
        .clone()
        .ok_or_else(|| anyhow!("OpenAI's sign-in answer had no ID token: try again"))?;
    let now = unix_now();
    let who = claims(&id_token, &ends.auth, &client, &nonce, now)?;
    let scopes = back
        .scope
        .map(|s| s.split_whitespace().map(String::from).collect());
    let tokens = reply.tokens(None, scopes, now)?;
    locked(dir, || {
        let mut s = load(dir);
        if saved.client.is_some() && s.sub.as_ref().is_some_and(|x| *x != who.sub) {
            // Another account than this registration's: never one's
            // registration with another's tokens. The next sign-in
            // registers afresh.
            save(
                dir,
                &Saved {
                    host: s.host,
                    ..Saved::default()
                },
            )?;
            bail!(
                "that's another ChatGPT account than the one signed in before: sign in again to use it"
            );
        }
        s.host = saved.host.clone();
        s.client = Some(client.clone());
        s.sub = Some(who.sub);
        s.email = who.email;
        s.id_token = Some(id_token);
        s.tokens = Some(tokens);
        save(dir, &s)?;
        Ok(account(dir))
    })
}

/// Signs out: the refresh token revoked at OpenAI, then the tokens
/// forgotten here (the registration and the host id stay, for the next
/// sign-in). False when OpenAI didn't confirm the revocation.
pub fn sign_out(dir: &Path, ends: &Endpoints) -> Result<bool> {
    locked(dir, || {
        let mut s = load(dir);
        let Some(t) = s.tokens.take() else {
            return Ok(true);
        };
        let revoked = revoke(ends, &t.refresh, s.client.as_deref().unwrap_or_default());
        s.id_token = None;
        save(dir, &s)?;
        Ok(revoked)
    })
}

fn revoke(ends: &Endpoints, refresh: &str, client: &str) -> bool {
    let Some(at) = http(
        &format!("{}/.well-known/openid-configuration", ends.auth),
        None,
        None,
        30,
    )
    .ok()
    .filter(|r| r.status == 200)
    .and_then(|r| serde_json::from_str::<Value>(&r.body).ok())
    .and_then(|v| v["revocation_endpoint"].as_str().map(String::from))
    // The token goes to OpenAI's sign-in and nowhere else.
    .filter(|u| u.starts_with(&format!("{}/", ends.auth))) else {
        return false;
    };
    let body = form(&[
        ("token", refresh),
        ("token_type_hint", "refresh_token"),
        ("client_id", client),
    ]);
    for pause in [0, 1, 3] {
        std::thread::sleep(Duration::from_secs(pause));
        match http(&at, None, Some((FORM, &body)), 30) {
            Ok(r) if r.status == 200 => return true,
            Ok(r) if r.status < 500 => return false,
            _ => {}
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Asking.

/// A token to send, renewed first when it's about to expire, or when a
/// request was refused with it (`force`).
fn access(dir: &Path, ends: &Endpoints, force: bool) -> Result<String> {
    locked(dir, || {
        let mut s = load(dir);
        let t = s.tokens.clone().ok_or_else(|| anyhow!(SIGNED_OUT))?;
        let client = s.client.clone().ok_or_else(|| anyhow!(SIGNED_OUT))?;
        let now = unix_now();
        if (!force && t.expires - now > EARLY) || (now < t.earliest_refresh && t.expires > now) {
            return Ok(t.access);
        }
        let r = http(
            &format!("{}/api/accounts/oauth/token", ends.auth),
            None,
            Some((
                FORM,
                &form(&[
                    ("grant_type", "refresh_token"),
                    ("client_id", &client),
                    ("refresh_token", &t.refresh),
                    ("resource", RESOURCE),
                ]),
            )),
            60,
        )?;
        if r.status == 200 {
            let reply: TokenReply = serde_json::from_str(&r.body)
                .context("OpenAI's answer to renewing the sign-in was unexpected")?;
            if let Some(id) = &reply.id_token {
                s.id_token = Some(id.clone());
            }
            let fresh = reply.tokens(Some(t.refresh), Some(t.scopes), now)?;
            let access = fresh.access.clone();
            s.tokens = Some(fresh);
            save(dir, &s)?;
            return Ok(access);
        }
        let (code, _) = error_of(&r.body);
        let code = code.unwrap_or_default();
        if ENDED.contains(&code.as_str()) || code == "invalid_client" {
            s.tokens = None;
            if code == "invalid_client" {
                // This registration isn't OpenAI's any more: a new one.
                s.client = None;
                s.sub = None;
                s.id_token = None;
            }
            save(dir, &s)?;
            bail!(ENDED_SAY);
        }
        bail!(
            "couldn't renew the sign-in with ChatGPT ({}): try again later",
            if code.is_empty() {
                r.status.to_string()
            } else {
                code
            }
        )
    })
}

/// A request with the access token, again with a renewed one if it was
/// refused (401).
fn authed(dir: &Path, ends: &Endpoints, call: impl Fn(&str) -> Result<Reply>) -> Result<Reply> {
    let r = call(&access(dir, ends, false)?)?;
    if r.status != 401 {
        return Ok(r);
    }
    call(&access(dir, ends, true)?)
}

/// What OpenAI's refusal means to someone, as its guide says to put it.
fn refused(status: u16, code: Option<&str>) -> String {
    match code {
        Some("subscription_sharing_usage_limit_exceeded") => LIMIT.into(),
        Some("subscription_sharing_user_not_eligible") => {
            "your ChatGPT plan can't be used in other apps: Ask AI needs ChatGPT Plus or Pro".into()
        }
        Some(
            "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable",
        ) => "ChatGPT can't answer right now: try again later".into(),
        Some("chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context") => {
            "ChatGPT doesn't let MyDiskIsFull use your plan: sign out and sign in again".into()
        }
        Some("subscription_sharing_invalid_user") => {
            "ChatGPT didn't accept the sign-in: sign out and sign in again".into()
        }
        _ => match status {
            401 => "ChatGPT didn't accept the sign-in: sign out and sign in again".into(),
            403 => {
                "ChatGPT refused the question: plan usage may not be available where you are".into()
            }
            429 => "too many questions at once: try again in a minute".into(),
            503 => "ChatGPT can't answer right now: try again later".into(),
            s => format!(
                "ChatGPT answered {s}{}",
                code.map(|c| format!(" ({c})")).unwrap_or_default()
            ),
        },
    }
}

/// The model to ask: the first OpenAI lists for this account.
fn model(dir: &Path, ends: &Endpoints) -> Result<(String, String)> {
    let r = authed(dir, ends, |t| {
        http(&format!("{}/models", ends.api), Some(t), None, 30)
    })?;
    if r.status != 200 {
        let (code, _) = error_of(&r.body);
        bail!(refused(r.status, code.as_deref()));
    }
    let v: Value =
        serde_json::from_str(&r.body).context("ChatGPT's list of models was unexpected")?;
    v["models"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| m["visibility"].as_str() == Some("list"))
        .and_then(|m| {
            let slug = m["slug"].as_str()?;
            Some((
                slug.to_string(),
                m["display_name"].as_str().unwrap_or(slug).to_string(),
            ))
        })
        .ok_or_else(|| anyhow!("ChatGPT has no model for MyDiskIsFull to ask"))
}

/// The text of a streamed answer, once it says it's complete.
fn streamed(body: &str) -> Result<String> {
    let mut text = String::new();
    let mut done = None;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
        match v["type"].as_str() {
            Some("response.output_text.delta") => text.push_str(v["delta"].as_str().unwrap_or("")),
            Some("response.completed") => done = Some(v),
            Some("response.failed") => {
                bail!(refused(0, v["response"]["error"]["code"].as_str()))
            }
            Some("response.incomplete") => bail!("ChatGPT's answer was cut short: try again"),
            Some("error") => bail!(refused(
                0,
                v["code"].as_str().or(v["error"]["code"].as_str())
            )),
            _ => {}
        }
    }
    let done =
        done.ok_or_else(|| anyhow!("ChatGPT stopped before it finished answering: try again"))?;
    if text.is_empty() {
        for item in done["response"]["output"].as_array().into_iter().flatten() {
            for c in item["content"].as_array().into_iter().flatten() {
                if c["type"].as_str() == Some("output_text") {
                    text.push_str(c["text"].as_str().unwrap_or(""));
                }
            }
        }
    }
    Ok(text)
}

/// What's asked of `model`, minus what this route turned down.
fn request(model: &str, instructions: &str, input: &str, schema: &Value, drop: &[&str]) -> Value {
    let mut body = json!({
        "model": model,
        "instructions": instructions,
        "input": [{"role": "user", "content": input}],
        // Plan usage takes nothing kept at OpenAI, and only streamed.
        "store": false,
        "stream": true,
        "text": {"format": {"type": "json_schema", "name": "answer", "strict": true, "schema": schema}},
        "reasoning": {"effort": "low"},
    });
    if let Some(o) = body.as_object_mut() {
        for d in drop {
            o.remove(*d);
        }
    }
    body
}

/// What a model said, and which.
#[derive(Debug)]
pub struct Said {
    pub text: String,
    pub model: String,
}

/// Asks with the plan: `input` under `instructions`, the answer in JSON as
/// `schema` says (unless this route won't take a schema: then as the
/// instructions say).
pub fn respond(
    dir: &Path,
    ends: &Endpoints,
    instructions: &str,
    input: &str,
    schema: &Value,
) -> Result<Said> {
    let a = account(dir);
    if !a.signed_in {
        bail!(SIGNED_OUT);
    }
    if !a.plan {
        bail!("ChatGPT doesn't let MyDiskIsFull use your plan: sign out and sign in again");
    }
    let (slug, name) = model(dir, ends)?;
    // What it may do without, if the route turns it down.
    let mut dropped: Vec<&str> = Vec::new();
    loop {
        let body = request(&slug, instructions, input, schema, &dropped).to_string();
        let r = authed(dir, ends, |t| {
            http(
                &format!("{}/responses", ends.api),
                Some(t),
                Some(("application/json", &body)),
                180,
            )
        })?;
        if r.status == 200 {
            return Ok(Said {
                text: streamed(&r.body)?,
                model: name,
            });
        }
        let (code, param) = error_of(&r.body);
        let optional = param.as_deref().and_then(|p| {
            ["reasoning", "text"]
                .into_iter()
                .find(|o| p == *o || p.starts_with(&format!("{o}.")))
        });
        match optional {
            Some(o)
                if code.as_deref() == Some("subscription_sharing_unsupported_capability")
                    && !dropped.contains(&o) =>
            {
                dropped.push(o)
            }
            _ => bail!(refused(r.status, code.as_deref())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn pkce_is_rfc_7636s() {
        // RFC 7636, appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn queries_and_forms_go_both_ways() {
        assert_eq!(encode("a b/c?d=é~"), "a%20b%2Fc%3Fd%3D%C3%A9~");
        let q = query("code=a%2Bb&state=x+y&scope=openid%20email&e=%ZZ&f=%4");
        assert_eq!(q["code"], "a+b");
        assert_eq!(q["state"], "x y");
        assert_eq!(q["scope"], "openid email");
        assert_eq!(q["e"], "%ZZ");
        assert_eq!(q["f"], "%4");
        assert_eq!(quoted("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
        let id = host_id().unwrap();
        assert_eq!(id.len(), "urn:uuid:".len() + 36);
        assert_eq!(&id[9 + 14..9 + 15], "4");
    }

    #[test]
    fn a_first_sign_in_registers_and_a_later_one_hints() {
        let ends = Endpoints::openai();
        let first = Saved {
            host: "urn:uuid:h".into(),
            ..Saved::default()
        };
        let u = authorize_url(
            &ends,
            &first,
            "http://127.0.0.1:9/auth/callback",
            "s",
            "n",
            "c",
        );
        let q = query(u.split_once('?').unwrap().1);
        assert!(u.starts_with("https://auth.openai.com/api/accounts/authorize?"));
        assert_eq!(q["client_id"], FIRST);
        assert_eq!(q["agent_name_hint"], NAME);
        assert_eq!(q["ext_agent_host_id"], "urn:uuid:h");
        assert_eq!(q["scope"], SCOPE);
        assert_eq!(q["resource"], RESOURCE);
        assert_eq!(q["redirect_uri"], "http://127.0.0.1:9/auth/callback");
        assert_eq!(q["code_challenge_method"], "S256");
        assert!(!q.contains_key("id_token_hint"));
        let later = Saved {
            client: Some("oaiapp_1".into()),
            email: Some("me@example.com".into()),
            id_token: Some("a.b.c".into()),
            ..first
        };
        let q = query(
            authorize_url(
                &ends,
                &later,
                "http://127.0.0.1:9/auth/callback",
                "s",
                "n",
                "c",
            )
            .split_once('?')
            .unwrap()
            .1,
        );
        assert_eq!(q["client_id"], "oaiapp_1");
        assert!(!q.contains_key("agent_name_hint"));
        assert_eq!(q["id_token_hint"], "a.b.c");
        assert_eq!(q["login_hint"], "me@example.com");
        assert_eq!(q["ext_agent_host_id"], "urn:uuid:h");
    }

    #[test]
    fn an_answer_counts_only_once_its_complete() {
        let ok = "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"known\\\":\"}\n\n\
                  data: {\"type\":\"response.output_text.delta\",\"delta\":\"true}\"}\n\n\
                  data: {\"type\":\"response.completed\",\"response\":{}}\n\n";
        assert_eq!(streamed(ok).unwrap(), "{\"known\":true}");
        let whole = "data: {\"type\":\"response.completed\",\"response\":{\"output\":[{\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}]}}\n";
        assert_eq!(streamed(whole).unwrap(), "hi");
        let cut = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"half\"}\n";
        assert!(
            streamed(cut)
                .unwrap_err()
                .to_string()
                .contains("stopped before")
        );
        let limit = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_limit_exceeded\"}}}\n";
        assert_eq!(streamed(limit).unwrap_err().to_string(), LIMIT);
    }

    fn jwt(claims: Value) -> String {
        format!(
            "{}.{}.sig",
            URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#),
            URL_SAFE_NO_PAD.encode(claims.to_string())
        )
    }

    #[test]
    fn an_id_token_must_be_this_attempts() {
        let good = json!({"iss": "https://i", "aud": "c", "exp": 2000, "nonce": "n", "sub": "u", "email": "e"});
        assert_eq!(
            claims(&jwt(good.clone()), "https://i", "c", "n", 1000)
                .unwrap()
                .sub,
            "u"
        );
        let mut list = good.clone();
        list["aud"] = json!(["x", "c"]);
        assert!(claims(&jwt(list), "https://i", "c", "n", 1000).is_ok());
        for (k, v) in [
            ("iss", json!("https://other")),
            ("aud", json!("d")),
            ("exp", json!(500)),
            ("nonce", json!("m")),
            ("sub", json!("")),
        ] {
            let mut bad = good.clone();
            bad[k] = v;
            assert!(
                claims(&jwt(bad), "https://i", "c", "n", 1000).is_err(),
                "{k}"
            );
        }
        assert!(claims("nonsense", "https://i", "c", "n", 1000).is_err());
    }

    /// What the stand-in for OpenAI was asked, and how it answers.
    #[derive(Default)]
    struct World {
        nonce: String,
        /// Refreshes asked for, with the refresh token each sent.
        refreshes: Vec<String>,
        refresh_error: Option<&'static str>,
        responses: Vec<Value>,
        respond_error: Option<(u16, Value)>,
        revoked: Vec<String>,
        /// Access tokens the API refuses (401).
        stale: Vec<String>,
    }

    fn reply(stream: &mut TcpStream, status: u16, kind: &str, body: &str) {
        let _ = write!(
            stream,
            "HTTP/1.1 {status} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    }

    /// OpenAI's sign-in and API on 127.0.0.1, answering as its guide says.
    fn standin(world: Arc<Mutex<World>>) -> Endpoints {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let iss = base.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut raw = Vec::new();
                let mut buf = [0; 4096];
                let (head, body) = loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw).into_owned();
                    if let Some((h, b)) = text.split_once("\r\n\r\n") {
                        let len = h
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if b.len() >= len || n == 0 {
                            break (h.to_string(), b.to_string());
                        }
                    }
                    if n == 0 {
                        break (text, String::new());
                    }
                };
                let path = head.split(' ').nth(1).unwrap_or("").to_string();
                let bearer = head
                    .lines()
                    .find_map(|l| l.strip_prefix("Authorization: Bearer "))
                    .map(String::from);
                let mut w = world.lock().unwrap();
                let f = query(&body);
                let token = |n: u32, w: &World| {
                    json!({
                        "access_token": format!("access-{n}"),
                        "refresh_token": format!("refresh-{n}"),
                        "id_token": jwt(json!({"iss": iss, "aud": "oaiapp_1", "exp": unix_now() + 3600,
                                               "nonce": w.nonce, "sub": "user-1", "email": "me@example.com"})),
                        "expires_in": 3600,
                        "scope": SCOPE,
                    })
                    .to_string()
                };
                match path.as_str() {
                    "/api/accounts/oauth/token" if f["grant_type"] == "authorization_code" => {
                        assert_eq!(f["client_id"], "oaiapp_1");
                        assert_eq!(f["code"], "code-1");
                        assert!(f["code_verifier"].len() >= 43);
                        let t = token(1, &w);
                        reply(&mut stream, 200, "application/json", &t);
                    }
                    "/api/accounts/oauth/token" => {
                        assert_eq!(f["grant_type"], "refresh_token");
                        assert_eq!(f["client_id"], "oaiapp_1");
                        w.refreshes.push(f["refresh_token"].clone());
                        if let Some(e) = w.refresh_error {
                            reply(
                                &mut stream,
                                400,
                                "application/json",
                                &json!({"error": e}).to_string(),
                            );
                        } else {
                            let t = token(1 + w.refreshes.len() as u32, &w);
                            reply(&mut stream, 200, "application/json", &t);
                        }
                    }
                    "/.well-known/openid-configuration" => {
                        let v = json!({"revocation_endpoint": format!("{iss}/api/accounts/oauth/revoke")});
                        reply(&mut stream, 200, "application/json", &v.to_string());
                    }
                    "/api/accounts/oauth/revoke" => {
                        w.revoked.push(f["token"].clone());
                        reply(&mut stream, 200, "text/plain", "");
                    }
                    _ if bearer.as_ref().is_some_and(|b| w.stale.contains(b)) => {
                        reply(&mut stream, 401, "application/json", "{}");
                    }
                    "/v1/models" => {
                        let v = json!({"models": [
                            {"slug": "hidden", "display_name": "Hidden", "visibility": "hide"},
                            {"slug": "gpt-test", "display_name": "GPT Test", "visibility": "list"},
                        ]});
                        reply(&mut stream, 200, "application/json", &v.to_string());
                    }
                    "/v1/responses" => {
                        let v: Value = serde_json::from_str(&body).unwrap();
                        w.responses.push(v.clone());
                        if let Some((status, e)) = w.respond_error.take() {
                            reply(&mut stream, status, "application/json", &e.to_string());
                            continue;
                        }
                        assert_eq!(v["store"], false);
                        assert_eq!(v["stream"], true);
                        assert_eq!(v["model"], "gpt-test");
                        let sse = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"{\\\"known\\\": true}\"}\n\n\
                                   data: {\"type\":\"response.completed\",\"response\":{}}\n\n";
                        reply(&mut stream, 200, "text/event-stream", sse);
                    }
                    _ => reply(&mut stream, 404, "text/plain", "no"),
                }
            }
        });
        Endpoints {
            auth: base.clone(),
            api: format!("{base}/v1"),
        }
    }

    /// The browser: back to the app's listener as OpenAI would send it,
    /// once with the wrong state first.
    fn browser(world: Arc<Mutex<World>>) -> impl Fn(&str) -> Result<()> {
        move |url: &str| {
            let q = query(url.split_once('?').unwrap().1);
            world.lock().unwrap().nonce = q["nonce"].clone();
            let back = q["redirect_uri"].clone();
            let state = q["state"].clone();
            std::thread::spawn(move || {
                let get = |query: String| {
                    let rest = back.strip_prefix("http://").unwrap();
                    let (host, path) = rest.split_once('/').unwrap();
                    let mut s = TcpStream::connect(host).unwrap();
                    write!(s, "GET /{path}?{query} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
                    let mut page = String::new();
                    let _ = s.read_to_string(&mut page);
                    page
                };
                assert!(get("code=evil&state=wrong".into()).contains("400"));
                let page = get(format!(
                    "code=code-1&state={}&client_id=oaiapp_1&scope={}",
                    encode(&state),
                    encode(SCOPE)
                ));
                assert!(page.contains(DONE));
            });
            Ok(())
        }
    }

    fn set_expiry(dir: &Path, at: i64) {
        let mut s = load(dir);
        s.tokens.as_mut().unwrap().expires = at;
        save(dir, &s).unwrap();
    }

    #[test]
    fn signing_in_asking_renewing_and_signing_out() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let world = Arc::new(Mutex::new(World::default()));
        let ends = standin(world.clone());
        let schema = json!({"type": "object"});

        assert!(!account(d).signed_in);
        assert!(
            respond(d, &ends, "i", "q", &schema)
                .unwrap_err()
                .to_string()
                .contains("not signed in")
        );

        let a = sign_in(d, &ends, &browser(world.clone()), Duration::from_secs(20)).unwrap();
        assert_eq!(
            a,
            Account {
                signed_in: true,
                email: Some("me@example.com".into()),
                plan: true,
                usage: USAGE
            }
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(d.join(FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let host = load(d).host;
        assert!(host.starts_with("urn:uuid:"));

        let said = respond(d, &ends, "instructions", "Folder: ~/.x", &schema).unwrap();
        assert_eq!(said.text, "{\"known\": true}");
        assert_eq!(said.model, "GPT Test");
        {
            let w = world.lock().unwrap();
            assert_eq!(w.responses[0]["instructions"], "instructions");
            assert_eq!(w.responses[0]["input"][0]["content"], "Folder: ~/.x");
            assert!(w.refreshes.is_empty());
        }

        // About to expire: renewed first, and the new refresh token kept.
        set_expiry(d, unix_now() + 10);
        respond(d, &ends, "i", "q", &schema).unwrap();
        assert_eq!(world.lock().unwrap().refreshes, ["refresh-1"]);
        assert_eq!(load(d).tokens.unwrap().refresh, "refresh-2");

        // Refused with a current token: renewed and asked again.
        world.lock().unwrap().stale.push("access-2".into());
        respond(d, &ends, "i", "q", &schema).unwrap();
        assert_eq!(world.lock().unwrap().refreshes, ["refresh-1", "refresh-2"]);

        // The route turns down a field: asked again without it, once.
        world.lock().unwrap().respond_error = Some((
            400,
            json!({"error": {"code": "subscription_sharing_unsupported_capability", "param": "reasoning.effort"}}),
        ));
        respond(d, &ends, "i", "q", &schema).unwrap();
        {
            let w = world.lock().unwrap();
            let last = w.responses.last().unwrap();
            assert!(last.get("reasoning").is_none());
            assert!(last.get("text").is_some());
        }

        // The plan's limit: said as OpenAI asks.
        world.lock().unwrap().respond_error = Some((
            429,
            json!({"error": {"code": "subscription_sharing_usage_limit_exceeded"}}),
        ));
        assert_eq!(
            respond(d, &ends, "i", "q", &schema)
                .unwrap_err()
                .to_string(),
            LIMIT
        );

        // Signing out revokes the refresh token and keeps the registration.
        assert!(sign_out(d, &ends).unwrap());
        assert_eq!(world.lock().unwrap().revoked, ["refresh-3"]);
        let s = load(d);
        assert!(s.tokens.is_none() && s.id_token.is_none());
        assert_eq!(s.client.as_deref(), Some("oaiapp_1"));
        assert_eq!(s.host, host);
        assert!(!account(d).signed_in);

        // Signed in again on the same registration; then a spent refresh
        // token ends the sign-in, and only it.
        sign_in(d, &ends, &browser(world.clone()), Duration::from_secs(20)).unwrap();
        world.lock().unwrap().refresh_error = Some("refresh_token_reused");
        set_expiry(d, unix_now() - 1);
        assert_eq!(
            respond(d, &ends, "i", "q", &schema)
                .unwrap_err()
                .to_string(),
            ENDED_SAY
        );
        let s = load(d);
        assert!(s.tokens.is_none());
        assert_eq!(s.client.as_deref(), Some("oaiapp_1"));
    }

    #[test]
    fn a_sign_in_nobody_finishes_ends() {
        let dir = tempfile::tempdir().unwrap();
        let e = sign_in(
            dir.path(),
            &Endpoints::openai(),
            &|_| Ok(()),
            Duration::from_millis(300),
        )
        .unwrap_err();
        assert!(e.to_string().contains("wasn't finished"), "{e}");
        assert!(!account(dir.path()).signed_in);
    }
}
