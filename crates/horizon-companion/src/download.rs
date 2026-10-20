//! Downloads and installs under `<Horizon data>/companion/`: `runtime/<build>/` (llama.cpp),
//! `voice-runtime/<build>/` (audio.cpp) and `models/<id>/` (the brain's and the senses' models).
//!
//! A download goes to a `.part` file that survives a cancel, a lost connection or a closed game, and
//! resumes from where it stopped with an HTTP Range request (the bytes already there are hashed first,
//! so the file is read once). It is checked against the pinned size and SHA-256, and only a complete,
//! verified folder is renamed into place, with an `.installed` marker written last: a half-installed
//! runtime or model never looks installed.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::catalogue::{Model, RuntimeBuild};
use crate::senses::{AudioCppBuild, SenseModel};

const INSTALLED: &str = ".installed";
/// Lost connections are resumed this many times before the download fails.
const RETRIES: u32 = 4;
/// Kept free on the disk after a download, so the player's worlds can still save.
const DISK_MARGIN: u64 = 256 << 20;

/// One file to fetch: where from, and what it must be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fetch {
    pub url: String,
    /// Its path inside the install folder.
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Hashing the part already downloaded.
    Resuming,
    Downloading,
    Unpacking,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    /// Bytes of the whole install (every file), done and in total.
    pub done: u64,
    pub total: u64,
}

impl Progress {
    pub fn fraction(&self) -> f32 {
        (self.done as f64 / self.total.max(1) as f64).min(1.0) as f32
    }
}

#[derive(Debug)]
pub enum Error {
    Cancelled,
    /// The server answered with an error or the connection failed `RETRIES` times.
    Http(String),
    Io(io::Error),
    Checksum {
        file: String,
        expected: String,
        actual: String,
    },
    Size {
        file: String,
        expected: u64,
        actual: u64,
    },
    NoSpace {
        needed: u64,
        free: u64,
    },
    Archive(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Cancelled => write!(f, "Download cancelled"),
            Error::Http(e) => write!(f, "Download failed: {e}"),
            Error::Io(e) => write!(f, "Disk error: {e}"),
            Error::Checksum { file, .. } => {
                write!(
                    f,
                    "{file} is damaged (its SHA-256 doesn't match); it will download again"
                )
            }
            Error::Size {
                file,
                expected,
                actual,
            } => {
                write!(f, "{file} has {actual} bytes instead of {expected}")
            }
            Error::NoSpace { needed, free } => write!(
                f,
                "Not enough disk space: {} MB needed, {} MB free",
                needed >> 20,
                free >> 20
            ),
            Error::Archive(e) => write!(f, "The runtime archive can't be unpacked: {e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

/// `<Horizon data>/companion`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(data_dir: &Path) -> Layout {
        Layout {
            root: data_dir.join("companion"),
        }
    }

    pub fn runtime_dir(&self, build: &RuntimeBuild) -> PathBuf {
        self.root.join("runtime").join(build.key())
    }

    pub fn model_dir(&self, model: &Model) -> PathBuf {
        self.root.join("models").join(model.id)
    }

    /// `llama-server` of an installed build.
    pub fn installed_runtime(&self, build: &RuntimeBuild) -> Option<PathBuf> {
        let dir = self.runtime_dir(build);
        dir.join(INSTALLED)
            .exists()
            .then(|| dir.join(build.server_exe()))
    }

    /// The file the runtime loads, once every file of the model is installed.
    pub fn installed_model(&self, model: &Model) -> Option<PathBuf> {
        let dir = self.model_dir(model);
        dir.join(INSTALLED)
            .exists()
            .then(|| dir.join(model.main_file().path))
    }

    pub fn install_runtime(
        &self,
        build: &RuntimeBuild,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<PathBuf, Error> {
        let fetch = Fetch {
            url: build.url(),
            path: build.file.to_owned(),
            size: build.size,
            sha256: build.sha256.to_owned(),
        };
        let dir = self.runtime_dir(build);
        install_archive(&fetch, build.server_exe(), &dir, progress, cancel)?;
        Ok(dir.join(build.server_exe()))
    }

    pub fn install_model(
        &self,
        model: &Model,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<PathBuf, Error> {
        let files: Vec<Fetch> = model
            .files
            .iter()
            .map(|f| Fetch {
                url: model.url(f),
                path: f.path.to_owned(),
                size: f.size,
                sha256: f.sha256.to_owned(),
            })
            .collect();
        let dir = self.model_dir(model);
        install_files(&files, &dir, progress, cancel)?;
        Ok(dir.join(model.main_file().path))
    }

    /// Removes a runtime build, its unfinished download included.
    pub fn delete_runtime(&self, build: &RuntimeBuild) -> io::Result<()> {
        let dir = self.runtime_dir(build);
        remove_all(&partial_dir(&dir))?;
        remove_all(&dir)
    }

    /// Removes a model, its unfinished download included.
    pub fn delete_model(&self, model: &Model) -> io::Result<()> {
        let dir = self.model_dir(model);
        remove_all(&partial_dir(&dir))?;
        remove_all(&dir)
    }

    /// audio.cpp, the voice's sidecar: `voice-runtime/<build>/`.
    pub fn voice_runtime_dir(&self, build: &AudioCppBuild) -> PathBuf {
        self.root.join("voice-runtime").join(build.key())
    }

    /// The folder `audiocpp_server` is in (its `model_specs/` beside it), once installed.
    pub fn installed_voice_runtime(&self, build: &AudioCppBuild) -> Option<PathBuf> {
        let dir = self.voice_runtime_dir(build);
        dir.join(INSTALLED).exists().then_some(dir)
    }

    /// Installs an audio.cpp build. Builds that need a second archive (CUDA's runtime, an Advanced
    /// option on Windows) are not installed here yet.
    pub fn install_voice_runtime(
        &self,
        build: &AudioCppBuild,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<PathBuf, Error> {
        if build.runtime.is_some() {
            return Err(Error::Archive(format!(
                "{} needs a second archive, which setup doesn't install yet",
                build.key()
            )));
        }
        let fetch = Fetch {
            url: AudioCppBuild::url(&build.archive),
            path: build.archive.path.to_owned(),
            size: build.archive.size,
            sha256: build.archive.sha256.to_owned(),
        };
        let dir = self.voice_runtime_dir(build);
        install_archive(&fetch, build.server_exe(), &dir, progress, cancel)?;
        Ok(dir)
    }

    pub fn sense_model_dir(&self, model: &SenseModel) -> PathBuf {
        self.root.join("models").join(model.id)
    }

    /// The folder of a sense model, once every file is installed (the engines find the files in it
    /// by `SenseModel::local_path`).
    pub fn installed_sense_model(&self, model: &SenseModel) -> Option<PathBuf> {
        let dir = self.sense_model_dir(model);
        dir.join(INSTALLED).exists().then_some(dir)
    }

    pub fn install_sense_model(
        &self,
        model: &SenseModel,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<PathBuf, Error> {
        let files: Vec<Fetch> = model
            .files
            .iter()
            .map(|f| Fetch {
                url: model.url(f),
                path: model.local_path(f).to_owned(),
                size: f.size,
                sha256: f.sha256.to_owned(),
            })
            .collect();
        let dir = self.sense_model_dir(model);
        install_files(&files, &dir, progress, cancel)?;
        Ok(dir)
    }

    pub fn delete_voice_runtime(&self, build: &AudioCppBuild) -> io::Result<()> {
        let dir = self.voice_runtime_dir(build);
        remove_all(&partial_dir(&dir))?;
        remove_all(&dir)
    }

    pub fn delete_sense_model(&self, model: &SenseModel) -> io::Result<()> {
        let dir = self.sense_model_dir(model);
        remove_all(&partial_dir(&dir))?;
        remove_all(&dir)
    }

    /// Bytes on disk under `companion/`, downloads in progress included.
    pub fn disk_usage(&self) -> u64 {
        fn walk(path: &Path) -> u64 {
            match fs::symlink_metadata(path) {
                Ok(m) if m.is_dir() => fs::read_dir(path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| walk(&e.path()))
                    .sum(),
                Ok(m) => m.len(),
                Err(_) => 0,
            }
        }
        walk(&self.root)
    }
}

fn remove_all(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// `models/qwen` → `models/.qwen.partial`: beside it, so the final rename stays on one disk.
fn partial_dir(dir: &Path) -> PathBuf {
    let name = dir
        .file_name()
        .map_or_else(Default::default, |n| n.to_string_lossy().into_owned());
    dir.with_file_name(format!(".{name}.partial"))
}

/// Downloads `files` into `dir` (replacing what was there) once all of them are verified.
pub fn install_files(
    files: &[Fetch],
    dir: &Path,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let partial = partial_dir(dir);
    fs::create_dir_all(&partial)?;
    let total: u64 = files.iter().map(|f| f.size).sum();
    check_space(&partial, files.iter().map(|f| remaining(&partial, f)).sum())?;
    let mut base = 0;
    for file in files {
        let part = part_path(&partial, file);
        if let Some(parent) = part.parent() {
            fs::create_dir_all(parent)?;
        }
        fetch(
            file,
            &part,
            &mut |p| progress(offset(p, base, total)),
            cancel,
        )?;
        base += file.size;
    }
    for file in files {
        fs::rename(part_path(&partial, file), partial.join(&file.path))?;
    }
    finish(&partial, dir)
}

/// Downloads an archive, unpacks it and installs the folder that holds `exe` as `dir`.
pub fn install_archive(
    archive: &Fetch,
    exe: &str,
    dir: &Path,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let partial = partial_dir(dir);
    fs::create_dir_all(&partial)?;
    // Unpacked, an archive takes about three times its size.
    check_space(&partial, remaining(&partial, archive) + archive.size * 3)?;
    let part = part_path(&partial, archive);
    let total = archive.size;
    fetch(archive, &part, progress, cancel)?;
    progress(Progress {
        phase: Phase::Unpacking,
        done: total,
        total,
    });
    let unpacked = partial.join("unpacked");
    remove_all(&unpacked)?;
    unpack(&part, &archive.path, &unpacked)?;
    let top = find_dir_with(&unpacked, exe, 3)
        .ok_or_else(|| Error::Archive(format!("it has no {exe}")))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(top.join(exe), fs::Permissions::from_mode(0o755))?;
    }
    let staged = partial.join("staged");
    remove_all(&staged)?;
    fs::rename(&top, &staged)?;
    let _ = fs::remove_file(&part);
    remove_all(&unpacked)?;
    finish(&staged, dir)?;
    remove_all(&partial)?;
    Ok(())
}

/// Moves a verified folder into place and marks it installed.
fn finish(staged: &Path, dir: &Path) -> Result<(), Error> {
    remove_all(dir)?;
    fs::rename(staged, dir)?;
    fs::write(dir.join(INSTALLED), "")?;
    Ok(())
}

fn part_path(partial: &Path, file: &Fetch) -> PathBuf {
    partial.join(format!("{}.part", file.path))
}

fn remaining(partial: &Path, file: &Fetch) -> u64 {
    let have = fs::metadata(part_path(partial, file)).map_or(0, |m| m.len());
    file.size.saturating_sub(have)
}

fn offset(p: Progress, base: u64, total: u64) -> Progress {
    Progress {
        phase: p.phase,
        done: base + p.done,
        total,
    }
}

fn check_space(dir: &Path, needed: u64) -> Result<(), Error> {
    if let Some(free) = free_space(dir)
        && free < needed + DISK_MARGIN
    {
        return Err(Error::NoSpace {
            needed: needed + DISK_MARGIN,
            free,
        });
    }
    Ok(())
}

/// Downloads `file` into `part`, resuming what is already there, and checks it.
pub fn fetch(
    file: &Fetch,
    part: &Path,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let mut out = fs::OpenOptions::new()
        .create(true)
        .read(true)
        .append(true)
        .open(part)?;
    let mut have = out.metadata()?.len();
    if have > file.size {
        out.set_len(0)?;
        have = 0;
    }
    let mut hasher = Sha256::new();
    if have > 0 {
        hash_prefix(
            &mut out,
            have,
            &mut hasher,
            &mut |done| {
                progress(Progress {
                    phase: Phase::Resuming,
                    done,
                    total: file.size,
                })
            },
            cancel,
        )?;
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(30))
        .user_agent(concat!("Horizon/", env!("CARGO_PKG_VERSION")))
        .build();
    let mut failures = 0;
    while have < file.size {
        match fetch_from(
            &agent,
            file,
            &mut out,
            &mut have,
            &mut hasher,
            progress,
            cancel,
        ) {
            Ok(()) => {}
            Err(Error::Http(e)) if failures < RETRIES => {
                failures += 1;
                std::thread::sleep(Duration::from_millis(500 * u64::from(failures)));
                if cancel.load(Ordering::Relaxed) {
                    return Err(Error::Cancelled);
                }
                let _ = e;
            }
            Err(e) => return Err(e),
        }
    }
    out.flush()?;
    drop(out);
    let actual = hex(&hasher.finalize());
    if actual != file.sha256.to_ascii_lowercase() {
        let _ = fs::remove_file(part);
        return Err(Error::Checksum {
            file: file.path.clone(),
            expected: file.sha256.clone(),
            actual,
        });
    }
    Ok(())
}

/// One request from `have` on; a dropped connection returns `Error::Http` with the bytes so far kept.
fn fetch_from(
    agent: &ureq::Agent,
    file: &Fetch,
    out: &mut fs::File,
    have: &mut u64,
    hasher: &mut Sha256,
    progress: &mut dyn FnMut(Progress),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let mut request = agent.get(&file.url);
    if *have > 0 {
        request = request.set("Range", &format!("bytes={have}-"));
    }
    let response = match request.call() {
        Ok(r) => r,
        // 416: the part is already whole or the server's file changed; start over.
        Err(ureq::Error::Status(416, _)) => {
            restart(out, have, hasher)?;
            return Err(Error::Http("the server refused to resume".into()));
        }
        Err(ureq::Error::Status(code, r)) => {
            return Err(Error::Http(format!(
                "{} answered {code} {}",
                host(&file.url),
                r.status_text()
            )));
        }
        Err(e) => return Err(Error::Http(e.to_string())),
    };
    if *have > 0 && response.status() != 206 {
        // The server ignored the range and sends the whole file.
        restart(out, have, hasher)?;
    }
    let mut reader = response.into_reader();
    let mut buf = vec![0u8; 1 << 16];
    let mut last = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::Http(format!("connection lost: {e}"))),
        };
        let n = n.min((file.size - *have) as usize);
        out.write_all(&buf[..n])?;
        hasher.update(&buf[..n]);
        *have += n as u64;
        if last.elapsed() >= Duration::from_millis(100) || *have == file.size {
            last = Instant::now();
            progress(Progress {
                phase: Phase::Downloading,
                done: *have,
                total: file.size,
            });
        }
        if *have == file.size {
            return Ok(());
        }
    }
    if *have < file.size {
        return Err(Error::Http(format!(
            "the connection closed at {} of {} bytes",
            *have, file.size
        )));
    }
    Ok(())
}

fn restart(out: &mut fs::File, have: &mut u64, hasher: &mut Sha256) -> io::Result<()> {
    out.set_len(0)?;
    out.seek(SeekFrom::Start(0))?;
    *have = 0;
    *hasher = Sha256::new();
    Ok(())
}

fn hash_prefix(
    file: &mut fs::File,
    len: u64,
    hasher: &mut Sha256,
    progress: &mut dyn FnMut(u64),
    cancel: &AtomicBool,
) -> Result<(), Error> {
    file.seek(SeekFrom::Start(0))?;
    let mut reader = file.take(len);
    let mut buf = vec![0u8; 1 << 20];
    let mut done = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done);
    }
    Ok(())
}

fn host(url: &str) -> &str {
    url.split("//")
        .nth(1)
        .and_then(|r| r.split('/').next())
        .unwrap_or(url)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of a file on disk, as lowercase hex.
pub fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hex(&hasher.finalize()))
}

/// `.tar.gz` (macOS, Linux: library symlinks kept) or `.zip` (Windows). Both libraries refuse entries
/// that would land outside `to`.
fn unpack(archive: &Path, name: &str, to: &Path) -> Result<(), Error> {
    let file = fs::File::open(archive)?;
    fs::create_dir_all(to)?;
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        tar::Archive::new(flate2::read::GzDecoder::new(file))
            .unpack(to)
            .map_err(|e| Error::Archive(e.to_string()))
    } else if name.ends_with(".zip") {
        zip::ZipArchive::new(file)
            .and_then(|mut z| z.extract(to))
            .map_err(|e| Error::Archive(e.to_string()))
    } else {
        Err(Error::Archive(format!("unknown archive type: {name}")))
    }
}

fn find_dir_with(dir: &Path, file: &str, depth: u32) -> Option<PathBuf> {
    if dir.join(file).is_file() {
        return Some(dir.to_owned());
    }
    if depth == 0 {
        return None;
    }
    let mut subdirs: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    subdirs.sort();
    subdirs
        .into_iter()
        .find_map(|d| find_dir_with(&d, file, depth - 1))
}

/// Free bytes on the disk holding `dir`.
#[cfg(unix)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: `path` is a valid C string and `stat` a writable statvfs.
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

/// Free bytes on the disk holding `dir`.
#[cfg(windows)]
pub fn free_space(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is NUL-terminated; the null pointers are optional outputs.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free)
}

#[cfg(not(any(unix, windows)))]
pub fn free_space(_dir: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    /// A one-file HTTP server: honours `Range` unless `ignore_range`, and the first `drops` responses
    /// close the connection after `drop_after` bytes. It records each request's Range header.
    pub(crate) struct Server {
        pub url: String,
        pub ranges: Arc<Mutex<Vec<Option<String>>>>,
    }

    pub(crate) fn serve(
        body: Vec<u8>,
        ignore_range: bool,
        drops: usize,
        drop_after: usize,
    ) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file.bin", listener.local_addr().unwrap());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let seen = ranges.clone();
        std::thread::spawn(move || {
            let mut drops = drops;
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut range = None;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        range = Some(v.trim().to_owned());
                    }
                }
                seen.lock().unwrap().push(range.clone());
                let start = match &range {
                    Some(r) if !ignore_range => r.trim_end_matches('-').parse::<usize>().unwrap(),
                    _ => 0,
                };
                let part = &body[start.min(body.len())..];
                let head = if start > 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{}/{}\r\nConnection: close\r\n\r\n",
                        part.len(),
                        body.len() - 1,
                        body.len()
                    )
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        part.len()
                    )
                };
                let _ = stream.write_all(head.as_bytes());
                if drops > 0 {
                    drops -= 1;
                    let _ = stream.write_all(&part[..drop_after.min(part.len())]);
                } else {
                    let _ = stream.write_all(part);
                }
            }
        });
        Server { url, ranges }
    }

    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("horizon-companion-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn body(len: usize) -> Vec<u8> {
        (0..len).map(|i| (i * 7 % 251) as u8).collect()
    }

    fn fetch_of(server: &Server, body: &[u8]) -> Fetch {
        Fetch {
            url: server.url.clone(),
            path: "file.bin".into(),
            size: body.len() as u64,
            sha256: hex(&Sha256::digest(body)),
        }
    }

    #[test]
    fn downloads_and_verifies() {
        let data = body(300_000);
        let server = serve(data.clone(), false, 0, 0);
        let dir = scratch("plain");
        let part = dir.join("file.bin.part");
        let mut last = None;
        fetch(
            &fetch_of(&server, &data),
            &part,
            &mut |p| last = Some(p),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(fs::read(&part).unwrap(), data);
        let last = last.unwrap();
        assert_eq!(
            (last.phase, last.done, last.total),
            (Phase::Downloading, 300_000, 300_000)
        );
        assert_eq!(*server.ranges.lock().unwrap(), [None]);
    }

    #[test]
    fn resumes_a_part_with_a_range_request() {
        let data = body(200_000);
        let server = serve(data.clone(), false, 0, 0);
        let dir = scratch("resume");
        let part = dir.join("file.bin.part");
        fs::write(&part, &data[..123_456]).unwrap();
        let mut phases = Vec::new();
        fetch(
            &fetch_of(&server, &data),
            &part,
            &mut |p| phases.push(p.phase),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(fs::read(&part).unwrap(), data);
        assert_eq!(*server.ranges.lock().unwrap(), [Some("123456-".to_owned())]);
        assert_eq!(phases.first(), Some(&Phase::Resuming));
    }

    #[test]
    fn starts_over_when_the_server_ignores_the_range() {
        let data = body(50_000);
        let server = serve(data.clone(), true, 0, 0);
        let dir = scratch("norange");
        let part = dir.join("file.bin.part");
        fs::write(&part, &data[..10_000]).unwrap();
        fetch(
            &fetch_of(&server, &data),
            &part,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(fs::read(&part).unwrap(), data);
    }

    #[test]
    fn a_lost_connection_resumes_on_its_own() {
        let data = body(400_000);
        let server = serve(data.clone(), false, 2, 100_000);
        let dir = scratch("dropped");
        let part = dir.join("file.bin.part");
        fetch(
            &fetch_of(&server, &data),
            &part,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(fs::read(&part).unwrap(), data);
        assert_eq!(
            *server.ranges.lock().unwrap(),
            [None, Some("100000-".to_owned()), Some("200000-".to_owned())]
        );
    }

    #[test]
    fn a_damaged_file_is_rejected_and_removed() {
        let data = body(10_000);
        let server = serve(data.clone(), false, 0, 0);
        let dir = scratch("damaged");
        let part = dir.join("file.bin.part");
        let mut wrong = fetch_of(&server, &data);
        wrong.sha256 = "0".repeat(64);
        let err = fetch(&wrong, &part, &mut |_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(matches!(err, Error::Checksum { .. }), "{err}");
        assert!(!part.exists());
    }

    #[test]
    fn cancel_keeps_the_part_for_later() {
        let data = body(10_000);
        let server = serve(data.clone(), false, 0, 0);
        let dir = scratch("cancel");
        let part = dir.join("file.bin.part");
        fs::write(&part, &data[..5_000]).unwrap();
        let err = fetch(
            &fetch_of(&server, &data),
            &part,
            &mut |_| {},
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        assert_eq!(fs::metadata(&part).unwrap().len(), 5_000);
    }

    #[test]
    fn refuses_to_fill_the_disk() {
        let dir = scratch("space");
        let huge = Fetch {
            url: "http://127.0.0.1:9/never".into(),
            path: "huge.bin".into(),
            size: 1 << 60,
            sha256: "0".repeat(64),
        };
        let err = install_files(
            &[huge],
            &dir.join("models/huge"),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, Error::NoSpace { .. }), "{err}");
    }

    #[test]
    fn installs_model_files_atomically_and_deletes_them() {
        let data = body(70_000);
        let server = serve(data.clone(), false, 0, 0);
        let dir = scratch("model");
        let target = dir.join("companion/models/test-model");
        let file = fetch_of(&server, &data);
        let mut fractions = Vec::new();
        install_files(
            &[file],
            &target,
            &mut |p| fractions.push(p.fraction()),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(fs::read(target.join("file.bin")).unwrap(), data);
        assert!(target.join(INSTALLED).exists());
        assert!(
            !partial_dir(&target).exists()
                || fs::read_dir(partial_dir(&target)).unwrap().next().is_none()
        );
        assert_eq!(fractions.last(), Some(&1.0));
        remove_all(&target).unwrap();
        assert!(!target.exists());
    }

    fn tar_gz(entries: &[(&str, &[u8])], links: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, path, *data).unwrap();
        }
        for (path, target) in links {
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_size(0);
            header.set_cksum();
            builder.append_link(&mut header, path, target).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn installs_a_tar_gz_runtime_with_its_symlinks() {
        let archive = tar_gz(
            &[
                ("llama-b1/llama-server", b"#!/bin/sh\n"),
                ("llama-b1/libllama.0.1.dylib", b"lib"),
            ],
            &[("llama-b1/libllama.dylib", "libllama.0.1.dylib")],
        );
        let server = serve(archive.clone(), false, 0, 0);
        let dir = scratch("tar");
        let target = dir.join("companion/runtime/b1-test");
        let mut fetch = fetch_of(&server, &archive);
        fetch.path = "llama-b1-bin-macos-arm64.tar.gz".into();
        install_archive(
            &fetch,
            "llama-server",
            &target,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(target.join("llama-server").is_file());
        assert!(target.join(INSTALLED).exists());
        assert!(!partial_dir(&target).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(target.join("llama-server"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111);
            assert_eq!(
                fs::read_link(target.join("libllama.dylib")).unwrap(),
                Path::new("libllama.0.1.dylib")
            );
            assert_eq!(fs::read(target.join("libllama.dylib")).unwrap(), b"lib");
        }
    }

    #[test]
    fn installs_a_flat_zip_runtime() {
        let mut zip = zip::ZipWriter::new(io::Cursor::new(Vec::new()));
        for (name, data) in [
            ("llama-server.exe", &b"MZ"[..]),
            ("ggml-vulkan.dll", b"dll"),
        ] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(data).unwrap();
        }
        let archive = zip.finish().unwrap().into_inner();
        let server = serve(archive.clone(), false, 0, 0);
        let dir = scratch("zip");
        let target = dir.join("companion/runtime/b1-win");
        let mut fetch = fetch_of(&server, &archive);
        fetch.path = "llama-b1-bin-win-vulkan-x64.zip".into();
        install_archive(
            &fetch,
            "llama-server.exe",
            &target,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(fs::read(target.join("ggml-vulkan.dll")).unwrap(), b"dll");
        assert!(target.join(INSTALLED).exists());
    }

    #[test]
    fn an_archive_without_the_server_is_not_installed() {
        let archive = tar_gz(&[("llama-b1/llama-cli", b"x")], &[]);
        let server = serve(archive.clone(), false, 0, 0);
        let dir = scratch("noserver");
        let target = dir.join("companion/runtime/b1");
        let mut fetch = fetch_of(&server, &archive);
        fetch.path = "a.tar.gz".into();
        let err = install_archive(
            &fetch,
            "llama-server",
            &target,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Archive(_)), "{err}");
        assert!(!target.exists());
    }

    #[test]
    fn layout_paths() {
        let layout = Layout::new(Path::new("/data"));
        let build = crate::catalogue::runtime_builds("macos", "aarch64")[0];
        assert_eq!(
            layout.runtime_dir(build),
            Path::new("/data/companion/runtime/b11756-macos-aarch64-metal")
        );
        let light = crate::catalogue::Tier::Light.model();
        assert_eq!(
            layout.model_dir(light),
            Path::new("/data/companion/models/qwen3.5-2b-q4_k_m")
        );
        assert_eq!(layout.installed_model(light), None);
        assert!(free_space(&std::env::temp_dir()).unwrap() > 0);
    }
}
