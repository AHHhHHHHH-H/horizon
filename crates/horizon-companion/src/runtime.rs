//! The llama-server supervisor: starts the runtime on 127.0.0.1 with a random port and API key, at low
//! priority, watches it load, and stops it with the world. If the game dies, the runtime dies too: a
//! process group and a watchdog on Unix, a kill-on-close job object on Windows.
//!
//! Several runtimes can run at once (the brain, and the senses' embedding classifier on one CPU
//! thread): each has its own port, key and log. [`Runtime::start`] returns at once and
//! [`Runtime::poll`] never waits, so the game can call both from its tick.

use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use crate::catalogue::Family;
use crate::client::{Endpoint, Provider};
use crate::hardware::{self, Hardware, Offload};

/// The design's context: 8K holds the system prompt, memory, state and six exchanges with room to spare.
pub const CONTEXT: u32 = 8192;
/// The model name the runtime answers to.
pub const ALIAS: &str = "horizon-companion";
/// How long a model may take to load before the runtime counts as failed (a 7 GB model from a hard disk).
const LOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// What a runtime serves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// `/v1/chat/completions` with the model's chat template, thinking off.
    Chat(Family),
    /// `/v1/embeddings` only (`--embeddings`), for the senses' classifier.
    Embeddings,
}

/// How to start a runtime.
#[derive(Clone, Debug, PartialEq)]
pub struct LaunchConfig {
    pub server: PathBuf,
    pub model: PathBuf,
    pub mode: Mode,
    pub threads: usize,
    pub offload: Offload,
    pub context: u32,
    /// stdout and stderr of the runtime; the previous run's log is kept beside it as `.old`.
    pub log: PathBuf,
    /// Advanced: appended as they are.
    pub extra_args: Vec<String>,
}

impl LaunchConfig {
    /// The brain, by the design's rules for this PC: threads from the physical cores, GPU offload from
    /// the devices.
    pub fn for_hardware(
        server: &Path,
        model: &Path,
        family: Family,
        hw: &Hardware,
        log: &Path,
    ) -> LaunchConfig {
        let model_bytes = fs::metadata(model).map_or(0, |m| m.len());
        LaunchConfig {
            server: server.to_owned(),
            model: model.to_owned(),
            mode: Mode::Chat(family),
            threads: hardware::runtime_threads(hw.physical_cores),
            offload: hardware::plan_offload(hw, model_bytes, extra_vram_mb(model_bytes)),
            context: CONTEXT,
            log: log.to_owned(),
            extra_args: Vec::new(),
        }
    }

    /// The brain as [`crate::plan::plan`] placed it: all of it on the picked GPU, or the processor
    /// with the plan's thread count.
    pub fn for_pick(
        server: &Path,
        model: &Path,
        family: Family,
        pick: &crate::plan::Pick,
        log: &Path,
    ) -> LaunchConfig {
        let (threads, offload) = match &pick.device {
            // On a GPU the CPU threads only feed it: two are plenty.
            crate::plan::Device::Gpu { id, .. } => (2, Offload::All { device: id.clone() }),
            crate::plan::Device::Cpu { threads } => (*threads, Offload::None),
        };
        LaunchConfig {
            server: server.to_owned(),
            model: model.to_owned(),
            mode: Mode::Chat(family),
            threads,
            offload,
            context: CONTEXT,
            log: log.to_owned(),
            extra_args: Vec::new(),
        }
    }

    /// An embedding model on `threads` CPU threads, leaving the GPU to the game and the brain.
    pub fn embeddings(
        server: &Path,
        model: &Path,
        threads: usize,
        context: u32,
        log: &Path,
    ) -> LaunchConfig {
        LaunchConfig {
            server: server.to_owned(),
            model: model.to_owned(),
            mode: Mode::Embeddings,
            threads,
            offload: Offload::None,
            context,
            log: log.to_owned(),
            extra_args: Vec::new(),
        }
    }

    /// The command line, without the program.
    pub fn args(&self, port: u16, api_key: &str) -> Vec<String> {
        let mut args: Vec<String> = [
            "--model",
            &self.model.to_string_lossy(),
            "--alias",
            ALIAS,
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--api-key",
            api_key,
            "--ctx-size",
            &self.context.to_string(),
            "--parallel",
            "1",
            "--threads",
            &self.threads.to_string(),
            "--no-webui",
            "--offline",
            "--log-timestamps",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        match self.mode {
            Mode::Chat(family) => {
                args.extend(
                    [
                        // Prompt-cache reuse: the system prompt is a fixed prefix, so later turns only
                        // process what changed.
                        "--cache-prompt",
                        "--cache-reuse",
                        "256",
                        // No extra copies of past prompts in RAM (up to 8 GiB by default): the one
                        // conversation lives in the slot and its checkpoints. Measured on Qwen3.5 4B,
                        // a 20-turn session peaked at 3.9 GB instead of 6.8 GB with the same cache hits.
                        // The default 32 checkpoints stay: with 12 or fewer the one at the end of the
                        // system prompt was lost and every fourth turn processed it again.
                        "--cache-ram",
                        "0",
                        "--jinja",
                        // Thinking off: replies start at once (Qwen3.5 thinks by default).
                        "--reasoning",
                        "off",
                    ]
                    .map(String::from),
                );
                match family {
                    Family::Qwen35 => args.extend([
                        "--chat-template-kwargs".into(),
                        r#"{"enable_thinking":false}"#.into(),
                    ]),
                    // Gemma's sliding-window layers keep only the last window by default, so after
                    // the conversation's start changes the runtime falls back on checkpoints: 32 of
                    // them took a session from 7.5 to 12.4 GB, and turns that dropped old exchanges
                    // reprocessed 600 tokens. The full cache costs 2 GB once and is reused by shifting
                    // (measured: 9.6 GB flat, 170 tokens on those turns).
                    Family::Gemma4 => args.push("--swa-full".into()),
                    Family::None => {}
                }
            }
            Mode::Embeddings => args.push("--embeddings".into()),
        }
        match &self.offload {
            Offload::All { device } => {
                args.extend([
                    "--device".into(),
                    device.clone(),
                    "--gpu-layers".into(),
                    "all".into(),
                ]);
            }
            Offload::Fit { device } => args.extend([
                "--device".into(),
                device.clone(),
                "--fit".into(),
                "on".into(),
                "--fit-target".into(),
                hardware::GAME_VRAM_MB.to_string(),
            ]),
            Offload::None => {
                args.extend([
                    "--device".into(),
                    "none".into(),
                    "--gpu-layers".into(),
                    "0".into(),
                ]);
            }
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// The KV cache and compute buffers at an 8K context on top of the weights, by model size (measured on
/// Metal with b11756; see docs/companions.md).
pub fn extra_vram_mb(model_bytes: u64) -> u64 {
    if model_bytes < 2 << 30 { 512 } else { 1024 }
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Loading,
    Ready {
        load_time: Duration,
    },
    /// It exited or never answered; the text has the reason and the end of its log.
    Failed(String),
}

pub struct Runtime {
    child: Child,
    port: u16,
    api_key: String,
    log: PathBuf,
    state: State,
    health: Receiver<Duration>,
    stop_health: Arc<AtomicBool>,
    started: Instant,
    guard: Guard,
}

impl Runtime {
    /// Starts llama-server and returns while it loads.
    pub fn start(config: &LaunchConfig) -> io::Result<Runtime> {
        let port = free_port()?;
        let api_key = random_key()?;
        if let Some(dir) = config.log.parent() {
            fs::create_dir_all(dir)?;
        }
        if config.log.exists() {
            let _ = fs::rename(&config.log, config.log.with_extension("old.log"));
        }
        let log = fs::File::create(&config.log)?;
        let mut command = Command::new(&config.server);
        command
            .args(config.args(port, &api_key))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        if let Some(dir) = config.server.parent() {
            command.current_dir(dir);
        }
        let (child, guard) = spawn_guarded(&mut command)?;
        let (send, health) = mpsc::channel();
        let stop_health = Arc::new(AtomicBool::new(false));
        let stop = stop_health.clone();
        let (url, key) = (format!("http://127.0.0.1:{port}/health"), api_key.clone());
        let started = Instant::now();
        std::thread::Builder::new()
            .name("companion-runtime-health".into())
            .spawn(move || {
                let agent = ureq::AgentBuilder::new()
                    .timeout_connect(Duration::from_millis(500))
                    .timeout(Duration::from_secs(2))
                    .build();
                while !stop.load(Ordering::Relaxed) && started.elapsed() < LOAD_TIMEOUT {
                    let ok = agent
                        .get(&url)
                        .set("Authorization", &format!("Bearer {key}"))
                        .call()
                        .is_ok_and(|r| r.status() == 200);
                    if ok {
                        let _ = send.send(started.elapsed());
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            })?;
        Ok(Runtime {
            child,
            port,
            api_key,
            log: config.log.clone(),
            state: State::Loading,
            health,
            stop_health,
            started,
            guard,
        })
    }

    /// How it stands, without waiting.
    pub fn poll(&mut self) -> &State {
        if matches!(self.state, State::Failed(_)) {
            return &self.state;
        }
        if let Ok(Some(status)) = self.child.try_wait() {
            self.stop_health.store(true, Ordering::Relaxed);
            self.state = State::Failed(format!(
                "The runtime stopped ({status}).\n{}",
                log_tail(&self.log, 12)
            ));
            return &self.state;
        }
        if let Ok(load_time) = self.health.try_recv() {
            self.state = State::Ready { load_time };
        } else if self.state == State::Loading && self.started.elapsed() > LOAD_TIMEOUT {
            self.state = State::Failed(format!(
                "The model didn't load in {} s.\n{}",
                LOAD_TIMEOUT.as_secs(),
                log_tail(&self.log, 12)
            ));
        }
        &self.state
    }

    /// Waits until it is ready (tools and tests; the game polls).
    pub fn wait_ready(&mut self, timeout: Duration) -> Result<Duration, String> {
        let until = Instant::now() + timeout;
        loop {
            match self.poll().clone() {
                State::Ready { load_time } => return Ok(load_time),
                State::Failed(e) => return Err(e),
                State::Loading if Instant::now() > until => {
                    return Err(format!("still loading after {} s", timeout.as_secs()));
                }
                State::Loading => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }

    /// Where the chat client reaches it.
    pub fn endpoint(&self) -> Endpoint {
        Endpoint {
            provider: Provider::Local,
            base_url: format!("http://127.0.0.1:{}/v1", self.port),
            api_key: Some(self.api_key.clone()),
            model: ALIAS.into(),
        }
    }

    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    pub fn log(&self) -> &Path {
        &self.log
    }

    /// Resident memory of the runtime process, in MB (the live readout of Advanced).
    pub fn memory_mb(&self) -> Option<u64> {
        process_rss_mb(self.child.id())
    }

    /// Stops it: politely first, then for sure. Also done on drop.
    pub fn stop(&mut self) {
        self.stop_health.store(true, Ordering::Relaxed);
        if self.child.try_wait().ok().flatten().is_none() {
            self.guard.terminate(&mut self.child);
        }
        let _ = self.child.wait();
        self.guard.release();
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.stop();
    }
}

fn free_port() -> io::Result<u16> {
    // The OS picks a free port; it stays free for the moment the runtime takes to bind it.
    Ok(std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port())
}

fn random_key() -> io::Result<String> {
    let mut bytes = [0u8; 24];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(crate::download::hex(&bytes))
}

/// The last `lines` lines of a log, for error messages.
pub fn log_tail(path: &Path, lines: usize) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map_or(0, |m| m.len());
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(16 << 10)));
    let mut text = String::new();
    let _ = file.take(16 << 10).read_to_string(&mut text);
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[cfg(unix)]
fn process_rss_mb(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kb: u64 = String::from_utf8(out.stdout).ok()?.trim().parse().ok()?;
    Some(kb / 1024)
}

#[cfg(windows)]
fn process_rss_mb(pid: u32) -> Option<u64> {
    let out = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!("(Get-Process -Id {pid}).WorkingSet64"),
        ])
        .output()
        .ok()?;
    let bytes: u64 = String::from_utf8(out.stdout).ok()?.trim().parse().ok()?;
    Some(bytes >> 20)
}

#[cfg(not(any(unix, windows)))]
fn process_rss_mb(_pid: u32) -> Option<u64> {
    None
}

/// Unix: the runtime leads its own process group at nice 10, and a forked watchdog that holds nothing
/// but the read end of a pipe kills that group when the pipe closes, which happens when the game exits
/// for any reason, a crash or `kill -9` included. Linux's `PR_SET_PDEATHSIG` is not used: it fires when
/// the spawning *thread* exits, and macOS has nothing like it.
#[cfg(unix)]
struct Guard {
    watchdog: Option<(libc::pid_t, std::os::fd::OwnedFd)>,
}

#[cfg(unix)]
fn spawn_guarded(command: &mut Command) -> io::Result<(Child, Guard)> {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    // SAFETY: setpriority is a plain system call, allowed between fork and exec.
    unsafe {
        command.pre_exec(|| {
            libc::setpriority(libc::PRIO_PROCESS, 0, 10);
            Ok(())
        });
    }
    let child = command.spawn()?;
    // Without a watchdog (fork failed) the runtime still stops with the world, just not with a crash.
    let watchdog = watchdog(child.id() as libc::pid_t).ok();
    Ok((child, Guard { watchdog }))
}

#[cfg(unix)]
fn watchdog(group: libc::pid_t) -> io::Result<(libc::pid_t, std::os::fd::OwnedFd)> {
    use std::os::fd::{FromRawFd, OwnedFd};
    let mut fds = [0; 2];
    // SAFETY: `fds` has room for the two descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (read, write) = (fds[0], fds[1]);
    for fd in fds {
        // SAFETY: both are descriptors this process just opened. Close-on-exec keeps them out of every
        // other program the game starts, so only the game holds the write end.
        unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
    }
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a writable rlimit.
    unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
    let max_fd = (limit.rlim_cur as i64).clamp(256, 65536) as libc::c_int;
    // SAFETY: after fork the child only makes async-signal-safe calls (close, read, kill, _exit): it
    // never allocates, locks or returns into Rust code.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        // SAFETY: closing the descriptors opened above.
        unsafe {
            libc::close(read);
            libc::close(write);
        }
        return Err(io::Error::last_os_error());
    }
    if pid == 0 {
        // SAFETY: see above; everything but the pipe's read end and the standard streams is closed, so
        // the watchdog keeps none of the game's files or sockets open.
        unsafe {
            for fd in 3..max_fd {
                if fd != read {
                    libc::close(fd);
                }
            }
            let mut byte = 0u8;
            loop {
                let n = libc::read(read, (&mut byte as *mut u8).cast(), 1);
                if n < 0 && *errno() == libc::EINTR {
                    continue;
                }
                break;
            }
            libc::kill(-group, libc::SIGKILL);
            libc::_exit(0);
        }
    }
    // SAFETY: the parent closes its read end and owns the write end from here.
    unsafe { libc::close(read) };
    Ok((pid, unsafe { OwnedFd::from_raw_fd(write) }))
}

#[cfg(all(
    unix,
    any(target_os = "macos", target_os = "ios", target_os = "freebsd")
))]
unsafe fn errno() -> *mut libc::c_int {
    unsafe { libc::__error() }
}

#[cfg(all(
    unix,
    not(any(target_os = "macos", target_os = "ios", target_os = "freebsd"))
))]
unsafe fn errno() -> *mut libc::c_int {
    unsafe { libc::__errno_location() }
}

#[cfg(unix)]
impl Guard {
    fn terminate(&mut self, child: &mut Child) {
        let group = child.id() as libc::pid_t;
        // SAFETY: signals the runtime's own process group.
        unsafe { libc::kill(-group, libc::SIGTERM) };
        let until = Instant::now() + Duration::from_secs(3);
        while Instant::now() < until {
            if child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // SAFETY: as above.
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }

    fn release(&mut self) {
        if let Some((pid, write)) = self.watchdog.take() {
            drop(write);
            let mut status = 0;
            // SAFETY: reaps the watchdog this process forked; it exits as soon as the pipe closes.
            unsafe { libc::waitpid(pid, &mut status, 0) };
        }
    }
}

/// Windows: a job object that kills the runtime when its last handle, the game's, closes; the runtime
/// starts at BELOW_NORMAL priority without a console window.
#[cfg(windows)]
struct Guard {
    job: windows_sys::Win32::Foundation::HANDLE,
}

#[cfg(windows)]
fn spawn_guarded(command: &mut Command) -> io::Result<(Child, Guard)> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };
    use windows_sys::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW};
    command.creation_flags(BELOW_NORMAL_PRIORITY_CLASS | CREATE_NO_WINDOW);
    let child = command.spawn()?;
    // SAFETY: plain Win32 calls on a job this function creates and the child it just spawned.
    let job = unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if !job.is_null() {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            AssignProcessToJobObject(job, child.as_raw_handle() as _);
        }
        job
    };
    Ok((child, Guard { job }))
}

#[cfg(windows)]
impl Guard {
    fn terminate(&mut self, child: &mut Child) {
        let _ = child.kill();
    }

    fn release(&mut self) {
        if !self.job.is_null() {
            // SAFETY: the job handle this guard owns, closed once.
            unsafe { windows_sys::Win32::Foundation::CloseHandle(self.job) };
            self.job = std::ptr::null_mut();
        }
    }
}

#[cfg(not(any(unix, windows)))]
struct Guard;

#[cfg(not(any(unix, windows)))]
fn spawn_guarded(command: &mut Command) -> io::Result<(Child, Guard)> {
    Ok((command.spawn()?, Guard))
}

#[cfg(not(any(unix, windows)))]
impl Guard {
    fn terminate(&mut self, child: &mut Child) {
        let _ = child.kill();
    }
    fn release(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::{Gpu, GpuKind};

    fn hw(apple: bool, gpus: Vec<Gpu>, cores: usize) -> Hardware {
        Hardware {
            ram_mb: 16384,
            physical_cores: cores,
            logical_cores: cores * 2,
            apple_silicon: apple,
            gpus,
        }
    }

    fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
        args.iter()
            .position(|a| a == name)
            .map(|i| args[i + 1].as_str())
    }

    #[test]
    fn command_line_follows_the_design() {
        let metal = Gpu {
            id: "MTL0".into(),
            name: "Apple M4 Pro".into(),
            total_mb: 53084,
            free_mb: 53083,
            kind: GpuKind::Integrated,
        };
        let mut config = LaunchConfig::for_hardware(
            Path::new("/rt/llama-server"),
            Path::new("/m/model.gguf"),
            Family::Qwen35,
            &hw(true, vec![metal], 14),
            Path::new("/logs/runtime.log"),
        );
        let args = config.args(40123, "k3y");
        assert_eq!(flag(&args, "--host"), Some("127.0.0.1"));
        assert_eq!(flag(&args, "--port"), Some("40123"));
        assert_eq!(flag(&args, "--api-key"), Some("k3y"));
        assert_eq!(flag(&args, "--ctx-size"), Some("8192"));
        assert_eq!(flag(&args, "--parallel"), Some("1"));
        assert_eq!(flag(&args, "--threads"), Some("13"));
        assert_eq!(flag(&args, "--gpu-layers"), Some("all"));
        assert_eq!(flag(&args, "--device"), Some("MTL0"));
        assert_eq!(flag(&args, "--reasoning"), Some("off"));
        assert_eq!(
            flag(&args, "--chat-template-kwargs"),
            Some(r#"{"enable_thinking":false}"#)
        );
        assert!(args.iter().any(|a| a == "--no-webui"));
        assert!(args.windows(2).any(|w| w == ["--cache-ram", "0"]));

        config.offload = Offload::None;
        config.mode = Mode::Chat(Family::Gemma4);
        let args = config.args(1, "k");
        assert_eq!(flag(&args, "--device"), Some("none"));
        assert_eq!(flag(&args, "--gpu-layers"), Some("0"));
        assert_eq!(flag(&args, "--chat-template-kwargs"), None);
        assert!(args.iter().any(|a| a == "--swa-full"));

        let embed = LaunchConfig::embeddings(
            Path::new("s"),
            Path::new("e5.gguf"),
            1,
            512,
            Path::new("e.log"),
        );
        let args = embed.args(2, "k");
        assert!(args.iter().any(|a| a == "--embeddings"));
        assert_eq!(flag(&args, "--threads"), Some("1"));
        assert_eq!(flag(&args, "--device"), Some("none"));
        assert_eq!(flag(&args, "--reasoning"), None);
    }

    #[test]
    fn integrated_gpus_run_on_the_cpu_with_threads_to_spare() {
        let hd620 = Gpu {
            id: "Vulkan0".into(),
            name: "Intel(R) HD Graphics 620 (KBL GT2)".into(),
            total_mb: 23988,
            free_mb: 21589,
            kind: GpuKind::Integrated,
        };
        let config = LaunchConfig::for_hardware(
            Path::new("llama-server"),
            Path::new("m.gguf"),
            Family::Qwen35,
            &hw(false, vec![hd620], 2),
            Path::new("r.log"),
        );
        assert_eq!(config.offload, Offload::None);
        assert_eq!(config.threads, 2);
    }

    #[test]
    fn keys_and_ports_differ_every_time() {
        let (a, b) = (random_key().unwrap(), random_key().unwrap());
        assert_eq!(a.len(), 48);
        assert_ne!(a, b);
        assert_ne!(free_port().unwrap(), 0);
    }

    #[cfg(unix)]
    fn fake_server(dir: &Path) -> LaunchConfig {
        use std::os::unix::fs::PermissionsExt;
        let server = dir.join("llama-server");
        fs::write(&server, "#!/bin/sh\necho \"started $*\"\nexec sleep 30\n").unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
        LaunchConfig {
            server,
            model: dir.join("m.gguf"),
            mode: Mode::Chat(Family::Qwen35),
            threads: 1,
            offload: Offload::None,
            context: CONTEXT,
            log: dir.join("logs/runtime.log"),
            extra_args: Vec::new(),
        }
    }

    #[cfg(unix)]
    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while !ok() {
            assert!(Instant::now() < until, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(unix)]
    fn alive(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 only probes whether the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// A fake runtime (a shell script that sleeps); start, poll, priority and stop are the real ones.
    #[cfg(unix)]
    #[test]
    fn a_runtime_that_never_answers_is_stopped_cleanly() {
        let dir = crate::download::tests::scratch("runtime");
        let config = fake_server(&dir);
        let mut runtime = Runtime::start(&config).unwrap();
        assert_eq!(*runtime.poll(), State::Loading);
        let pid = runtime.pid() as libc::pid_t;
        // SAFETY: reads the scheduling priority of this process and of the child just started.
        let (ours, nice) = unsafe {
            (
                libc::getpriority(libc::PRIO_PROCESS, 0),
                libc::getpriority(libc::PRIO_PROCESS, pid as _),
            )
        };
        // 10, or what it inherited when the game itself runs nicer (raising priority needs root).
        assert_eq!(nice, ours.max(10));
        wait_for("the log", || {
            fs::read_to_string(&config.log)
                .unwrap_or_default()
                .contains("started --model")
        });
        let started = Instant::now();
        runtime.stop();
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(!alive(pid), "the runtime is gone");
    }

    /// The game's side of the next test: starts a runtime, prints its pid and dies without cleaning up.
    #[cfg(unix)]
    #[test]
    #[ignore = "run by a_runtime_dies_with_the_game"]
    fn game_that_crashes() {
        let Ok(dir) = std::env::var("HORIZON_COMPANION_CRASH_DIR") else {
            return;
        };
        let config = fake_server(Path::new(&dir));
        let runtime = Runtime::start(&config).unwrap();
        println!("runtime-pid={}", runtime.pid());
        wait_for("the log", || {
            fs::read_to_string(&config.log)
                .unwrap_or_default()
                .contains("started")
        });
        std::mem::forget(runtime);
        // SAFETY: ends the process at once, like a crash: no destructor runs.
        unsafe { libc::_exit(3) };
    }

    #[cfg(unix)]
    #[test]
    fn a_runtime_dies_with_the_game() {
        let dir = crate::download::tests::scratch("runtime-crash");
        let out = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::tests::game_that_crashes",
                "--ignored",
                "--nocapture",
            ])
            .env("HORIZON_COMPANION_CRASH_DIR", &dir)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let pid: libc::pid_t = stdout
            .lines()
            .find_map(|l| l.trim().strip_prefix("runtime-pid=")?.parse().ok())
            .unwrap_or_else(|| panic!("no pid in {stdout}"));
        assert_eq!(out.status.code(), Some(3));
        // The runtime was re-parented when the game died; the watchdog kills it within moments.
        wait_for("the orphaned runtime to die", || !alive(pid));
    }

    #[cfg(unix)]
    #[test]
    fn a_runtime_that_exits_reports_its_log() {
        use std::os::unix::fs::PermissionsExt;
        let dir = crate::download::tests::scratch("runtime-exit");
        let server = dir.join("llama-server");
        fs::write(
            &server,
            "#!/bin/sh\necho 'error: failed to load model' >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&server, fs::Permissions::from_mode(0o755)).unwrap();
        let config = LaunchConfig {
            server,
            model: dir.join("m.gguf"),
            mode: Mode::Chat(Family::Qwen35),
            threads: 1,
            offload: Offload::None,
            context: CONTEXT,
            log: dir.join("runtime.log"),
            extra_args: Vec::new(),
        };
        let mut runtime = Runtime::start(&config).unwrap();
        let err = runtime.wait_ready(Duration::from_secs(5)).unwrap_err();
        assert!(err.contains("failed to load model"), "{err}");
    }
}
