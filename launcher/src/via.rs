//! Join a server that runs another Minecraft version: the launcher runs ViaProxy for the game.
//!
//! ViaProxy (github.com/ViaVersion/ViaProxy) is a separate Java program that listens on this
//! computer, speaks one Minecraft version to the game and another to the real server, and
//! translates between them. The launcher
//!
//! 1. finds Java 17 or newer (`horizon_prepare::java`),
//! 2. downloads ViaProxy's newest release into the launcher's data folder the first time it is
//!    needed (never bundled: it is someone else's program, under its own license),
//! 3. writes a one-run `viaproxy.yml` that listens on a free port of **127.0.0.1 only**, so nobody
//!    else on the network can reach it, and forwards to the server in the version the player named,
//! 4. starts it and waits until it accepts connections, and
//! 5. gives the game `127.0.0.1:<port>` in place of the server's address.
//!
//! The [`Proxy`] it returns stops ViaProxy when dropped: keep it alive until the game exits.
//!
//! Accounts: a server in online mode needs a Microsoft account, and ViaProxy is the one that joins
//! it, so the account has to be added to ViaProxy itself (run its jar once and use the Accounts
//! tab). The launcher's own sign-in is not shared with it, on purpose: ViaProxy keeps its accounts
//! in plain text in its own folder, and Horizon's are in the system's keychain. When ViaProxy has
//! an account, runs use it (`auth-method: account`); `HORIZON_VIA_AUTH=none|account` overrides.
use crate::backend;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

/// ViaProxy's newest release, from GitHub.
const LATEST: &str = "https://api.github.com/repos/ViaVersion/ViaProxy/releases/latest";
/// Where the jar of a release may come from.
const DOWNLOAD_PREFIX: &str = "https://github.com/ViaVersion/ViaProxy/";
/// ViaProxy's own notes: older than Java 17 needs its separate Java 8 jar, which this doesn't use.
const JAVA_MAJOR: i64 = 17;
/// How long ViaProxy gets to start listening (the first start unpacks its libraries).
const READY: Duration = Duration::from_secs(60);
/// The biggest download taken for ViaProxy's jar (it is about 30 MB).
const MAX_JAR: u64 = 128 * 1024 * 1024;
/// The environment variable that forces the authentication: `none` or `account`.
pub const AUTH_ENV: &str = "HORIZON_VIA_AUTH";

/// ViaProxy's folder in the launcher's data: the jar, its log, its saved accounts.
pub fn folder() -> PathBuf {
    backend::data_dir().join("viaproxy")
}

/// The downloaded jar.
pub fn jar() -> PathBuf {
    folder().join("ViaProxy.jar")
}

fn log_file() -> PathBuf {
    folder().join("viaproxy.log")
}

/// A server address split into its host and port (25565 when there is none). IPv6 addresses
/// are written in brackets, `[::1]:25565`, as the game reads them.
pub fn split_address(address: &str) -> Result<(String, u16), String> {
    let address = address.trim();
    if address.is_empty() {
        return Err("The server address is empty.".into());
    }
    let (host, port): (&str, Option<&str>) = if let Some(rest) = address.strip_prefix('[') {
        let (host, tail) = rest
            .split_once(']')
            .ok_or_else(|| format!("{address} has no closing ]."))?;
        if tail.is_empty() {
            (host, None)
        } else {
            let port = tail
                .strip_prefix(':')
                .ok_or_else(|| format!("{address} is not a server address."))?;
            (host, Some(port))
        }
    } else {
        match address.rsplit_once(':') {
            Some((host, port)) if !host.contains(':') => (host, Some(port)),
            _ => (address, None),
        }
    };
    // The host goes into a YAML file: only what an address is made of.
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':');
    if host.is_empty() || !host.chars().all(plain) {
        return Err(format!("{address} is not a server address."));
    }
    let port = match port {
        None => 25565,
        Some(text) => text
            .parse::<u16>()
            .ok()
            .filter(|p| *p != 0)
            .ok_or_else(|| format!("{text} is not a port."))?,
    };
    Ok((host.to_string(), port))
}

/// A Minecraft version as ViaProxy names them (`1.21.4`, `26.2`, `1.8.9`, `b1.7.3`, `auto`), or
/// why it can't be one. It ends up in a YAML file, so only a version's own characters pass.
pub fn check_version(version: &str) -> Result<String, String> {
    let version = version.trim();
    let plain = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_');
    if version.is_empty() || version.len() > 32 || !version.chars().all(plain) {
        return Err(format!("{version:?} is not a Minecraft version."));
    }
    Ok(version.to_string())
}

/// The `viaproxy.yml` of one run: listen on `bind_port` of this computer only, forward to
/// `host:port` as `version`. The values are quoted: unquoted, YAML reads `26.2` as a number.
pub fn config_yaml(bind_port: u16, host: &str, port: u16, version: &str, account: bool) -> String {
    let target = if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    };
    format!(
        "bind-address: \"127.0.0.1:{bind_port}\"\n\
         target-address: \"{target}\"\n\
         target-version: \"{version}\"\n\
         auth-method: {}\n",
        if account { "account" } else { "none" }
    )
}

/// Whether ViaProxy has a Microsoft account saved (`saves.json` in its folder).
pub fn has_account() -> bool {
    fs::read(folder().join("saves.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .is_some_and(|saves| saves["accounts"].as_array().is_some_and(|a| !a.is_empty()))
}

fn use_account() -> bool {
    match std::env::var(AUTH_ENV).ok().as_deref() {
        Some("none") => false,
        Some("account") => true,
        _ => has_account(),
    }
}

/// The jar to download in a GitHub release: the one `.jar` that isn't the Java 8 build, sources or
/// documentation, with its SHA-256 when GitHub lists one. Only files of ViaProxy's own repository.
pub fn pick_asset(release: &Value) -> Option<(String, Option<String>)> {
    release["assets"].as_array()?.iter().find_map(|asset| {
        let name = asset["name"].as_str()?.to_ascii_lowercase();
        let wanted = name.ends_with(".jar")
            && !["java8", "sources", "javadoc"].iter().any(|w| name.contains(w));
        let url = asset["browser_download_url"].as_str()?;
        if !wanted || !url.starts_with(DOWNLOAD_PREFIX) {
            return None;
        }
        let sha256 = asset["digest"]
            .as_str()
            .and_then(|d| d.strip_prefix("sha256:"))
            .map(str::to_string);
        Some((url.to_string(), sha256))
    })
}

/// Downloads ViaProxy's newest release over the jar in [`folder`]; its tag.
pub fn update(log: &dyn Fn(String)) -> Result<String, String> {
    log("Looking for the newest ViaProxy…".into());
    let release: Value = backend::get(LATEST)?
        .into_json()
        .map_err(|e| format!("ViaProxy's release list could not be read: {e}"))?;
    let tag = release["tag_name"].as_str().unwrap_or("unknown").to_string();
    let (url, sha256) = pick_asset(&release)
        .ok_or_else(|| "ViaProxy's newest release has no jar to download.".to_string())?;
    log(format!("Downloading ViaProxy {tag}…"));
    let mut body = Vec::new();
    backend::get(&url)?
        .into_reader()
        .take(MAX_JAR + 1)
        .read_to_end(&mut body)
        .map_err(|e| format!("The ViaProxy download failed: {e}"))?;
    if body.len() as u64 > MAX_JAR {
        return Err("The ViaProxy download is larger than expected; it was dropped.".into());
    }
    match sha256 {
        Some(expected) => {
            let got = format!("{:x}", Sha256::digest(&body));
            if !got.eq_ignore_ascii_case(&expected) {
                return Err("The ViaProxy download does not match GitHub's checksum.".into());
            }
        }
        None => log("GitHub lists no checksum for this file; it came over HTTPS from ViaProxy's repository.".into()),
    }
    let dir = folder();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let part = dir.join("ViaProxy.jar.part");
    fs::write(&part, &body).map_err(|e| e.to_string())?;
    fs::rename(&part, jar()).map_err(|e| e.to_string())?;
    let _ = fs::write(dir.join("version.txt"), &tag);
    Ok(tag)
}

/// The jar, downloaded first when there is none.
pub fn ensure_jar(log: &dyn Fn(String)) -> Result<PathBuf, String> {
    if !jar().is_file() {
        update(log)?;
    }
    Ok(jar())
}

fn java() -> Result<PathBuf, String> {
    let jdk = horizon_prepare::java::find_runtime(JAVA_MAJOR, &[])?;
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    Ok(jdk.home().join("bin").join(exe))
}

fn free_port() -> Result<u16, String> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|e| format!("No free port for ViaProxy: {e}"))
}

/// The last lines of ViaProxy's log, for an error message.
fn tail(path: &Path) -> String {
    let text = fs::read(path)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default();
    let lines: Vec<&str> = text.lines().rev().take(8).collect();
    if lines.is_empty() {
        return String::new();
    }
    let lines: Vec<&str> = lines.into_iter().rev().collect();
    format!("\nviaproxy.log ends:\n{}", lines.join("\n"))
}

/// A running ViaProxy. Dropping it stops the process and removes the run's config file.
pub struct Proxy {
    child: Child,
    port: u16,
    config: PathBuf,
}

impl Proxy {
    /// The address the game joins instead of the server's.
    pub fn address(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }
}

impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.config);
    }
}

/// Starts ViaProxy for `target` (an address) as Minecraft `version`, and returns it once it
/// accepts connections.
pub fn start(target: &str, version: &str, log: &dyn Fn(String)) -> Result<Proxy, String> {
    let (host, port) = split_address(target)?;
    let version = check_version(version)?;
    let java = java()?;
    let jar = ensure_jar(log)?;
    let bind = free_port()?;
    let dir = folder();
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = format!("run-{bind}.yml");
    let account = use_account();
    fs::write(
        dir.join(&name),
        config_yaml(bind, &host, port, &version, account),
    )
    .map_err(|e| format!("ViaProxy's settings could not be written: {e}"))?;
    if !account {
        log(format!(
            "ViaProxy has no Microsoft account, so only servers in offline mode can be joined. \
             For an online-mode server run `java -jar \"{}\"` once and add one in its Accounts tab.",
            jar.display()
        ));
    }
    let out = fs::File::create(log_file()).map_err(|e| e.to_string())?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut command = crate::process::command(java);
    command
        .current_dir(&dir)
        .arg("-jar")
        .arg(&jar)
        .arg("config")
        .arg(&name)
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err);
    log(format!("Starting ViaProxy for {target} as Minecraft {version}…"));
    let child = command
        .spawn()
        .map_err(|e| format!("Java could not be started for ViaProxy: {e}"))?;
    let mut proxy = Proxy {
        child,
        port: bind,
        config: dir.join(&name),
    };
    wait_ready(&mut proxy)?;
    Ok(proxy)
}

fn wait_ready(proxy: &mut Proxy) -> Result<(), String> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, proxy.port));
    let started = Instant::now();
    loop {
        if let Ok(Some(status)) = proxy.child.try_wait() {
            return Err(format!("ViaProxy stopped ({status}).{}", tail(&log_file())));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(250)).is_ok() {
            return Ok(());
        }
        if started.elapsed() > READY {
            return Err(format!(
                "ViaProxy did not start listening within {} seconds.{}",
                READY.as_secs(),
                tail(&log_file())
            ));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
mod tests;
