//! Single-instance + remote control over a per-user named pipe.
//!
//! Protocol: the client writes one JSON line `{"argv": [...], "cwd": "..."}`
//! and reads one JSON line back `{"ok": bool, "result": ..., "error": "..."}`.

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use windows_sys::Win32::Foundation::{ERROR_PIPE_CONNECTED, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_FIRST_PIPE_INSTANCE, FlushFileBuffers, PIPE_ACCESS_DUPLEX};
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Request {
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Response {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub result: Value,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

impl Response {
    pub fn ok(result: Value) -> Self {
        Self { ok: true, result, error: String::new() }
    }
    pub fn err(e: impl Into<String>) -> Self {
        Self { ok: false, result: Value::Null, error: e.into() }
    }
}

impl From<Result<Value, String>> for Response {
    fn from(r: Result<Value, String>) -> Self {
        match r {
            Ok(v) => Self::ok(v),
            Err(e) => Self::err(e),
        }
    }
}

/// A request delivered to the GUI thread, with a channel for the answer.
pub struct Incoming {
    pub req: Request,
    pub reply: Sender<Response>,
}

pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let user: String = user.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!(r"\\.\pipe\flick-{user}")
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create_instance(first: bool) -> Option<HANDLE> {
    let name = wide(&pipe_name());
    let mut open_mode = PIPE_ACCESS_DUPLEX;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let h = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            1 << 16,
            1 << 16,
            0,
            std::ptr::null(),
        )
    };
    (h != INVALID_HANDLE_VALUE).then_some(h)
}

/// The listening side, owned by the GUI process.
pub struct Server {
    first: HANDLE,
}

unsafe impl Send for Server {}

impl Server {
    /// Claim the pipe name. Fails if another Flick window already owns it.
    pub fn bind() -> Option<Self> {
        create_instance(true).map(|first| Self { first })
    }

    /// Accept connections on a background thread, forwarding requests to the
    /// returned channel. `ctx` is filled in once the GUI exists and is poked so
    /// the event loop wakes up for each request.
    pub fn spawn(self, ctx: Arc<OnceLock<egui::Context>>) -> Receiver<Incoming> {
        let (tx, rx) = mpsc::channel();
        // Raw handles aren't Send; move it across as an integer.
        let first = self.first as isize;
        std::thread::Builder::new()
            .name("flick-ipc".into())
            .spawn(move || {
                let mut handle = first as HANDLE;
                loop {
                    let connected =
                        unsafe { ConnectNamedPipe(handle, std::ptr::null_mut()) != 0 || GetLastError() == ERROR_PIPE_CONNECTED };
                    let conn = handle;
                    // Open the next instance right away so concurrent clients don't see "busy".
                    handle = loop {
                        if let Some(h) = create_instance(false) {
                            break h;
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    };
                    let file = unsafe { File::from_raw_handle(conn as _) };
                    if !connected {
                        continue;
                    }
                    let tx = tx.clone();
                    let ctx = ctx.clone();
                    std::thread::spawn(move || serve_one(file, tx, ctx));
                }
            })
            .expect("spawn ipc thread");
        rx
    }
}

fn serve_one(file: File, tx: Sender<Incoming>, ctx: Arc<OnceLock<egui::Context>>) {
    let mut reader = BufReader::new(&file);
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }
    let resp = match serde_json::from_str::<Request>(&line) {
        Ok(req) => {
            let (rtx, rrx) = mpsc::channel();
            if tx.send(Incoming { req, reply: rtx }).is_err() {
                Response::err("player is shutting down")
            } else {
                if let Some(c) = ctx.get() {
                    c.request_repaint();
                }
                rrx.recv_timeout(Duration::from_secs(30))
                    .unwrap_or_else(|_| Response::err("player did not respond (is the window minimized or busy?)"))
            }
        }
        Err(e) => Response::err(format!("bad request: {e}")),
    };
    let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| r#"{"ok":false,"error":"encode"}"#.into());
    out.push('\n');
    let mut f = &file;
    let _ = f.write_all(out.as_bytes());
    unsafe { FlushFileBuffers(file.as_raw_handle() as _) };
}

#[derive(Debug)]
pub enum ClientError {
    /// No Flick window is running.
    NotRunning,
    Io(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotRunning => write!(f, "no Flick window is running"),
            ClientError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ClientError {}

fn connect(timeout: Duration) -> Result<File, ClientError> {
    let start = Instant::now();
    loop {
        match std::fs::OpenOptions::new().read(true).write(true).open(pipe_name()) {
            Ok(f) => return Ok(f),
            // All instances busy: the server is between accepts. Retry briefly.
            Err(e) if e.raw_os_error() == Some(231) && start.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(15));
            }
            Err(e) if e.raw_os_error() == Some(2) => return Err(ClientError::NotRunning),
            Err(e) => return Err(ClientError::Io(e.to_string())),
        }
    }
}

/// Send one request to the running player.
pub fn send(argv: &[String]) -> Result<Response, ClientError> {
    let file = connect(Duration::from_secs(3))?;
    // Let the player take focus; Windows otherwise blocks background windows from raising themselves.
    let mut pid = 0u32;
    unsafe {
        if GetNamedPipeServerProcessId(file.as_raw_handle() as _, &mut pid) != 0 {
            AllowSetForegroundWindow(pid);
        }
    }
    let req = Request { argv: argv.to_vec(), cwd: std::env::current_dir().ok() };
    let mut line = serde_json::to_string(&req).map_err(|e| ClientError::Io(e.to_string()))?;
    line.push('\n');
    (&file).write_all(line.as_bytes()).map_err(|e| ClientError::Io(e.to_string()))?;
    let mut resp = String::new();
    BufReader::new(&file).read_line(&mut resp).map_err(|e| ClientError::Io(e.to_string()))?;
    serde_json::from_str(&resp).map_err(|e| ClientError::Io(format!("bad response from player: {e}")))
}

pub fn is_running() -> bool {
    matches!(send(&["ping".into()]), Ok(r) if r.ok)
}
