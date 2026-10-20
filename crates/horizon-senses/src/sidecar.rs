//! A sidecar server process: audio.cpp's `audiocpp_server` or the classifier's `llama-server`.
//!
//! It listens on 127.0.0.1 on a free port picked here, runs at low priority (nice 10, or
//! BELOW_NORMAL on Windows) with the thread count it is given, writes its log to a file, and dies with
//! the game even when the game is killed:
//! - Unix: it is started through `sh`, which holds the read end of a pipe whose write end only the game
//!   has; when the game exits for any reason the pipe closes and `sh` kills the server. Its process
//!   group is killed on drop.
//! - Windows: it is assigned to a job object that kills its processes when the game's handle closes.
use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

/// What to start.
#[derive(Clone, Debug)]
pub struct Launch {
    pub exe: PathBuf,
    /// The arguments; `{port}` in any of them is replaced with the chosen port.
    pub args: Vec<OsString>,
    /// The working directory (audio.cpp resolves its `model_specs/` from it).
    pub dir: PathBuf,
    /// Its standard output and error go here.
    pub log: PathBuf,
    /// Files written before the start, with `{port}` replaced too (audio.cpp's server config).
    pub files: Vec<(PathBuf, String)>,
}

/// A running sidecar; stopped (with everything it started) on drop.
pub struct Sidecar {
    child: Child,
    port: u16,
    log: PathBuf,
    /// The write end of the pipe that keeps the server alive (Unix).
    #[cfg(unix)]
    lifeline: Option<std::process::ChildStdin>,
    #[cfg(windows)]
    _job: windows::Job,
}

/// A port nobody listens on right now. The sidecar binds it a moment later; if something else took it
/// meanwhile, the start fails and [`Sidecar::start`] tries another.
pub fn free_port() -> io::Result<u16> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(listener.local_addr()?.port())
}

fn with_port(s: &str, port: u16) -> String {
    s.replace("{port}", &port.to_string())
}

impl Sidecar {
    /// Start it and wait until `GET <health>` answers 200, up to `timeout` (model loading included
    /// when the server loads eagerly). Tries three ports.
    pub fn start(launch: &Launch, health: &str, timeout: Duration) -> io::Result<Sidecar> {
        let mut last = io::Error::other("not started");
        for _ in 0..3 {
            let port = free_port()?;
            let mut sidecar = Self::spawn(launch, port)?;
            match sidecar.wait_ready(health, timeout) {
                Ok(()) => return Ok(sidecar),
                Err(e) => last = e,
            }
        }
        Err(last)
    }

    fn spawn(launch: &Launch, port: u16) -> io::Result<Sidecar> {
        for (path, text) in &launch.files {
            fs::write(path, with_port(text, port))?;
        }
        if let Some(dir) = launch.log.parent() {
            fs::create_dir_all(dir)?;
        }
        let log = fs::File::create(&launch.log)?;
        let args: Vec<OsString> = launch
            .args
            .iter()
            .map(|a| match a.to_str() {
                Some(s) => with_port(s, port).into(),
                None => a.clone(),
            })
            .collect();
        Self::spawn_os(launch, &args, port, log)
    }

    #[cfg(unix)]
    fn spawn_os(
        launch: &Launch,
        args: &[OsString],
        port: u16,
        log: fs::File,
    ) -> io::Result<Sidecar> {
        use std::os::unix::process::CommandExt;
        // `$0` is the server, the rest its arguments. A background reader holds the lifeline pipe
        // (as fd 3: a background job's stdin is /dev/null) and kills the server at end of file, when
        // the game is gone; sh itself waits for the server, so it exits when the server does.
        let script = r#"exec 3<&0 </dev/null
"$0" "$@" 3<&- & server=$!
( while read -r _ <&3; do :; done; kill $server 2>/dev/null ) &
wait $server
"#;
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(script)
            .arg(&launch.exe)
            .args(args)
            .current_dir(&launch.dir)
            .stdin(Stdio::piped())
            .stdout(log.try_clone()?)
            .stderr(log);
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                libc::setpgid(0, 0);
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let lifeline = child.stdin.take().expect("piped stdin");
        Ok(Sidecar {
            child,
            port,
            log: launch.log.clone(),
            lifeline: Some(lifeline),
        })
    }

    #[cfg(windows)]
    fn spawn_os(
        launch: &Launch,
        args: &[OsString],
        port: u16,
        log: fs::File,
    ) -> io::Result<Sidecar> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        let job = windows::Job::kill_on_close()?;
        let child = Command::new(&launch.exe)
            .args(args)
            .current_dir(&launch.dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS)
            .spawn()?;
        job.assign(&child)?;
        Ok(Sidecar {
            child,
            port,
            log: launch.log.clone(),
            _job: job,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn log(&self) -> &Path {
        &self.log
    }

    /// The process id of the server's launcher (`sh` on Unix, the server on Windows).
    pub fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Whether it is still running.
    pub fn alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Poll `GET <health>` until it answers 200.
    pub fn wait_ready(&mut self, health: &str, timeout: Duration) -> io::Result<()> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if !self.alive() {
                return Err(io::Error::other(format!(
                    "the sidecar stopped while starting: {}",
                    self.log_tail(800)
                )));
            }
            if http_ok(self.port, health) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "the sidecar did not answer in {timeout:?}: {}",
                self.log_tail(800)
            ),
        ))
    }

    /// The end of its log, for error messages.
    pub fn log_tail(&self, bytes: usize) -> String {
        let text = fs::read(&self.log).unwrap_or_default();
        let start = text.len().saturating_sub(bytes);
        String::from_utf8_lossy(&text[start..]).trim().to_owned()
    }

    pub fn stop(&mut self) {
        // Closing the lifeline alone stops the server; the signals make sure of it.
        #[cfg(unix)]
        self.lifeline.take();
        #[cfg(unix)]
        // SAFETY: plain syscalls; the group is the one `pre_exec` made.
        unsafe {
            let group = self.child.id() as libc::pid_t;
            libc::kill(-group, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if !self.alive() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        #[cfg(unix)]
        // SAFETY: as above.
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Sidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

/// A bare `GET` answered 200 (no HTTP client: this runs every 50 ms while a model loads).
fn http_ok(port: u16, path: &str) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, Duration::from_millis(200)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_millis(500)));
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    if io::Write::write_all(&mut stream, request.as_bytes()).is_err() {
        return false;
    }
    let mut head = [0u8; 12];
    stream.read_exact(&mut head).is_ok() && head.starts_with(b"HTTP/1.") && &head[9..12] == b"200"
}

#[cfg(windows)]
mod windows {
    use std::{io, os::windows::io::AsRawHandle, process::Child};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        },
    };

    /// A job whose processes die when its last handle (the game's) closes.
    pub struct Job(HANDLE);

    // SAFETY: a job handle can be used and closed from any thread.
    unsafe impl Send for Job {}

    impl Job {
        pub fn kill_on_close() -> io::Result<Job> {
            // SAFETY: plain Win32 calls with valid arguments; the handle is owned by `Job`.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(io::Error::last_os_error());
                }
                let job = Job(handle);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&raw const info).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        pub fn assign(&self, child: &Child) -> io::Result<()> {
            // SAFETY: both handles are valid for the duration of the call.
            if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) } == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle is ours and closed once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("horizon-senses-{name}-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn running(pid: i32) -> bool {
        // SAFETY: signal 0 only checks that the process exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// A server stand-in: `sh` serving one 200 on the port, written as a script file.
    fn fake_server(dir: &Path) -> PathBuf {
        let exe = dir.join("server.sh");
        fs::write(
            &exe,
            "#!/bin/sh\necho \"pid $$ port $1\"\nexec /usr/bin/python3 -c \"
import http.server,sys
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(s): s.send_response(200); s.end_headers(); s.wfile.write(b'ok')
    def log_message(s,*a): pass
http.server.HTTPServer(('127.0.0.1', int(sys.argv[1])), H).serve_forever()\" \"$1\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        exe
    }

    fn server_pid(log: &Path) -> i32 {
        let text = fs::read_to_string(log).unwrap();
        text.split_whitespace().nth(1).unwrap().parse().unwrap()
    }

    #[test]
    fn starts_on_a_free_port_then_stops_with_its_children() {
        if !Path::new("/usr/bin/python3").exists() {
            return;
        }
        let dir = scratch("start");
        let launch = Launch {
            exe: fake_server(&dir),
            args: vec!["{port}".into()],
            dir: dir.clone(),
            log: dir.join("server.log"),
            files: vec![(dir.join("config.json"), "{\"port\": {port}}".into())],
        };
        let sidecar = Sidecar::start(&launch, "/health", Duration::from_secs(10)).unwrap();
        let port = sidecar.port();
        assert!(http_ok(port, "/health"));
        assert_eq!(
            fs::read_to_string(dir.join("config.json")).unwrap(),
            format!("{{\"port\": {port}}}")
        );
        assert!(
            fs::read_to_string(sidecar.log())
                .unwrap()
                .contains(&format!("port {port}"))
        );
        let server = server_pid(sidecar.log());
        // Low priority, inherited from sh.
        // SAFETY: plain syscall.
        assert_eq!(
            unsafe { libc::getpriority(libc::PRIO_PROCESS, server as libc::id_t) },
            10
        );
        drop(sidecar);
        let deadline = Instant::now() + Duration::from_secs(3);
        while running(server) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!running(server));
        assert!(!http_ok(port, "/health"));
    }

    #[test]
    fn the_server_dies_when_the_lifeline_closes() {
        if !Path::new("/usr/bin/python3").exists() {
            return;
        }
        let dir = scratch("lifeline");
        let launch = Launch {
            exe: fake_server(&dir),
            args: vec!["{port}".into()],
            dir: dir.clone(),
            log: dir.join("server.log"),
            files: Vec::new(),
        };
        let sidecar = Sidecar::start(&launch, "/health", Duration::from_secs(10)).unwrap();
        let server = server_pid(sidecar.log());
        // What happens when the game is killed: the pipe closes, nothing else runs.
        let mut sidecar = sidecar;
        sidecar.lifeline.take();
        let deadline = Instant::now() + Duration::from_secs(3);
        while running(server) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!running(server));
        // sh exits with the server.
        let deadline = Instant::now() + Duration::from_secs(3);
        while sidecar.alive() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!sidecar.alive());
    }

    #[test]
    fn a_server_that_exits_fails_the_start_with_its_log() {
        let dir = scratch("fail");
        let launch = Launch {
            exe: "/bin/sh".into(),
            args: vec!["-c".into(), "echo model file missing; exit 3".into()],
            dir: dir.clone(),
            log: dir.join("server.log"),
            files: Vec::new(),
        };
        let error = Sidecar::start(&launch, "/health", Duration::from_secs(5))
            .err()
            .unwrap();
        assert!(error.to_string().contains("model file missing"), "{error}");
    }
}
