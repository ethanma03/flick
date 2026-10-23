//! Minimal libmpv binding, loaded at runtime from `libmpv-2.dll` next to the exe.
//!
//! Only what Flick needs: the client API (commands, properties, events) and the
//! OpenGL render API. Loading dynamically means no import library at build time
//! and a readable error instead of a missing-DLL crash if the file is absent.

use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

type Handle = *mut c_void;
type RenderCtx = *mut c_void;

pub const FORMAT_STRING: c_int = 1;
pub const FORMAT_FLAG: c_int = 3;
pub const FORMAT_INT64: c_int = 4;
pub const FORMAT_DOUBLE: c_int = 5;

pub const EVENT_NONE: c_int = 0;
pub const EVENT_SHUTDOWN: c_int = 1;
pub const EVENT_START_FILE: c_int = 6;
pub const EVENT_END_FILE: c_int = 7;
pub const EVENT_FILE_LOADED: c_int = 8;
pub const EVENT_VIDEO_RECONFIG: c_int = 17;
pub const EVENT_PLAYBACK_RESTART: c_int = 21;
pub const EVENT_PROPERTY_CHANGE: c_int = 22;

const RENDER_PARAM_API_TYPE: c_int = 1;
const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const RENDER_PARAM_OPENGL_FBO: c_int = 3;
const RENDER_PARAM_FLIP_Y: c_int = 4;
const RENDER_UPDATE_FRAME: u64 = 1;

/// End-file reasons (`mpv_end_file_reason`).
pub const END_EOF: c_int = 0;
pub const END_ERROR: c_int = 4;

#[repr(C)]
struct RawEvent {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}

#[repr(C)]
struct RawEventProperty {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct RawEventEndFile {
    reason: c_int,
    error: c_int,
}

#[repr(C)]
struct RenderParam {
    ty: c_int,
    data: *mut c_void,
}

#[repr(C)]
struct OpenGlInitParams {
    get_proc_address: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
    get_proc_address_ctx: *mut c_void,
}

#[repr(C)]
struct OpenGlFbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

struct Api {
    _lib: libloading::Library,
    create: unsafe extern "C" fn() -> Handle,
    initialize: unsafe extern "C" fn(Handle) -> c_int,
    terminate_destroy: unsafe extern "C" fn(Handle),
    error_string: unsafe extern "C" fn(c_int) -> *const c_char,
    free: unsafe extern "C" fn(*mut c_void),
    set_option_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    command: unsafe extern "C" fn(Handle, *mut *const c_char) -> c_int,
    command_async: unsafe extern "C" fn(Handle, u64, *mut *const c_char) -> c_int,
    set_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    set_property_string: unsafe extern "C" fn(Handle, *const c_char, *const c_char) -> c_int,
    set_property_async: unsafe extern "C" fn(Handle, u64, *const c_char, c_int, *mut c_void) -> c_int,
    get_property: unsafe extern "C" fn(Handle, *const c_char, c_int, *mut c_void) -> c_int,
    get_property_string: unsafe extern "C" fn(Handle, *const c_char) -> *mut c_char,
    observe_property: unsafe extern "C" fn(Handle, u64, *const c_char, c_int) -> c_int,
    wait_event: unsafe extern "C" fn(Handle, f64) -> *mut RawEvent,
    set_wakeup_callback: unsafe extern "C" fn(Handle, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    render_context_create: unsafe extern "C" fn(*mut RenderCtx, Handle, *mut RenderParam) -> c_int,
    render_context_set_update_callback:
        unsafe extern "C" fn(RenderCtx, Option<unsafe extern "C" fn(*mut c_void)>, *mut c_void),
    render_context_update: unsafe extern "C" fn(RenderCtx) -> u64,
    render_context_render: unsafe extern "C" fn(RenderCtx, *mut RenderParam) -> c_int,
    render_context_report_swap: unsafe extern "C" fn(RenderCtx),
    render_context_free: unsafe extern "C" fn(RenderCtx),
}

static API: OnceLock<Result<Api, String>> = OnceLock::new();

pub const DLL: &str = "libmpv-2.dll";

/// Where we look for libmpv: next to the exe, then `vendor/mpv` for `cargo run` from the repo.
fn dll_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        v.push(dir.join(DLL));
    }
    v.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor").join("mpv").join(DLL));
    v
}

fn api() -> Result<&'static Api, String> {
    API.get_or_init(load).as_ref().map_err(Clone::clone)
}

fn load() -> Result<Api, String> {
    let path = dll_candidates()
        .into_iter()
        .find(|p| p.exists())
        .ok_or_else(|| format!("{DLL} not found next to Flick (run scripts/fetch-mpv.ps1)"))?;
    // SAFETY: libmpv is a plain C library with no unusual init requirements.
    let lib = unsafe { libloading::Library::new(&path) }.map_err(|e| format!("couldn't load {}: {e}", path.display()))?;
    macro_rules! sym {
        ($name:literal) => {
            *unsafe { lib.get(concat!($name, "\0").as_bytes()) }.map_err(|e| format!("{DLL}: missing {}: {e}", $name))?
        };
    }
    Ok(Api {
        create: sym!("mpv_create"),
        initialize: sym!("mpv_initialize"),
        terminate_destroy: sym!("mpv_terminate_destroy"),
        error_string: sym!("mpv_error_string"),
        free: sym!("mpv_free"),
        set_option_string: sym!("mpv_set_option_string"),
        command: sym!("mpv_command"),
        command_async: sym!("mpv_command_async"),
        set_property: sym!("mpv_set_property"),
        set_property_string: sym!("mpv_set_property_string"),
        set_property_async: sym!("mpv_set_property_async"),
        get_property: sym!("mpv_get_property"),
        get_property_string: sym!("mpv_get_property_string"),
        observe_property: sym!("mpv_observe_property"),
        wait_event: sym!("mpv_wait_event"),
        set_wakeup_callback: sym!("mpv_set_wakeup_callback"),
        render_context_create: sym!("mpv_render_context_create"),
        render_context_set_update_callback: sym!("mpv_render_context_set_update_callback"),
        render_context_update: sym!("mpv_render_context_update"),
        render_context_render: sym!("mpv_render_context_render"),
        render_context_report_swap: sym!("mpv_render_context_report_swap"),
        render_context_free: sym!("mpv_render_context_free"),
        _lib: lib,
    })
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

fn err(api: &Api, code: c_int) -> String {
    unsafe { CStr::from_ptr((api.error_string)(code)) }.to_string_lossy().into_owned()
}

fn check(api: &Api, code: c_int, what: &str) -> Result<(), String> {
    if code < 0 { Err(format!("{what}: {}", err(api, code))) } else { Ok(()) }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropValue {
    Flag(bool),
    Int(i64),
    Double(f64),
    Str(String),
    /// The property became unavailable (e.g. `duration` while nothing is loaded).
    None,
}

#[derive(Debug)]
pub enum Event {
    Property { id: u64, value: PropValue },
    StartFile,
    FileLoaded,
    /// `error` is set when the file failed to open or decode.
    EndFile { eof: bool, error: Option<String> },
    VideoReconfig,
    PlaybackRestart,
    CommandReply { id: u64, error: Option<String> },
    Shutdown,
    Other,
}

/// An mpv player instance. All methods are thread-safe on the mpv side.
pub struct Mpv {
    api: &'static Api,
    h: Handle,
}

unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

impl Mpv {
    /// Create and configure an instance. `options` are applied before initialization
    /// (things like `vo` and `hwdec` only work there).
    pub fn new(options: &[(&str, &str)]) -> Result<Self, String> {
        let api = api()?;
        let h = unsafe { (api.create)() };
        if h.is_null() {
            return Err("mpv_create failed".into());
        }
        let mpv = Self { api, h };
        for (k, v) in options {
            let (k2, v2) = (cstr(k), cstr(v));
            let r = unsafe { (api.set_option_string)(h, k2.as_ptr(), v2.as_ptr()) };
            // A newer/older libmpv may lack an option; that's not fatal.
            if r < 0 {
                eprintln!("flick: mpv option {k}={v}: {}", err(api, r));
            }
        }
        check(api, unsafe { (api.initialize)(h) }, "mpv_initialize")?;
        Ok(mpv)
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).chain(Some(std::ptr::null())).collect();
        let r = unsafe { (self.api.command)(self.h, ptrs.as_mut_ptr()) };
        check(self.api, r, args.first().copied().unwrap_or("command"))
    }

    /// Fire-and-forget command; the result arrives later as `Event::CommandReply { id }`.
    pub fn command_async(&self, id: u64, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).chain(Some(std::ptr::null())).collect();
        let r = unsafe { (self.api.command_async)(self.h, id, ptrs.as_mut_ptr()) };
        check(self.api, r, args.first().copied().unwrap_or("command"))
    }

    pub fn set(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        check(self.api, unsafe { (self.api.set_property_string)(self.h, n.as_ptr(), v.as_ptr()) }, name)
    }

    /// Queue a property change without waiting for mpv's core (the value is copied).
    pub fn set_async(&self, name: &str, value: &str) -> Result<(), String> {
        let (n, v) = (cstr(name), cstr(value));
        let mut p: *const c_char = v.as_ptr();
        let r = unsafe { (self.api.set_property_async)(self.h, 0, n.as_ptr(), FORMAT_STRING, &mut p as *mut _ as _) };
        check(self.api, r, name)
    }

    pub fn set_flag(&self, name: &str, value: bool) -> Result<(), String> {
        let n = cstr(name);
        let mut v: c_int = value as c_int;
        check(self.api, unsafe { (self.api.set_property)(self.h, n.as_ptr(), FORMAT_FLAG, &mut v as *mut _ as _) }, name)
    }

    pub fn set_double(&self, name: &str, value: f64) -> Result<(), String> {
        let n = cstr(name);
        let mut v = value;
        check(self.api, unsafe { (self.api.set_property)(self.h, n.as_ptr(), FORMAT_DOUBLE, &mut v as *mut _ as _) }, name)
    }

    pub fn get_string(&self, name: &str) -> Option<String> {
        let n = cstr(name);
        let p = unsafe { (self.api.get_property_string)(self.h, n.as_ptr()) };
        if p.is_null() {
            return None;
        }
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        unsafe { (self.api.free)(p as _) };
        Some(s)
    }

    pub fn get_double(&self, name: &str) -> Option<f64> {
        let n = cstr(name);
        let mut v = 0f64;
        let r = unsafe { (self.api.get_property)(self.h, n.as_ptr(), FORMAT_DOUBLE, &mut v as *mut _ as _) };
        (r >= 0).then_some(v)
    }

    pub fn get_flag(&self, name: &str) -> Option<bool> {
        let n = cstr(name);
        let mut v: c_int = 0;
        let r = unsafe { (self.api.get_property)(self.h, n.as_ptr(), FORMAT_FLAG, &mut v as *mut _ as _) };
        (r >= 0).then_some(v != 0)
    }

    /// Node-valued properties (track-list, metadata...) as JSON.
    pub fn get_json(&self, name: &str) -> Option<serde_json::Value> {
        self.get_string(name).and_then(|s| serde_json::from_str(&s).ok())
    }

    pub fn observe(&self, id: u64, name: &str, format: c_int) {
        let n = cstr(name);
        unsafe { (self.api.observe_property)(self.h, id, n.as_ptr(), format) };
    }

    /// Call `f` (from an mpv thread) whenever new events are queued.
    pub fn set_wakeup(&self, f: Box<dyn Fn() + Send + Sync>) {
        // Leaked on purpose: lives as long as the player, which lives as long as the process.
        let raw = Box::into_raw(Box::new(f));
        unsafe { (self.api.set_wakeup_callback)(self.h, Some(trampoline), raw as _) };
    }

    /// Non-blocking: the next queued event, or `None`.
    pub fn next_event(&self) -> Option<Event> {
        let ev = unsafe { &*(self.api.wait_event)(self.h, 0.0) };
        let err_of = |code: c_int| (code < 0).then(|| err(self.api, code));
        Some(match ev.event_id {
            EVENT_NONE => return None,
            EVENT_SHUTDOWN => Event::Shutdown,
            EVENT_START_FILE => Event::StartFile,
            EVENT_FILE_LOADED => Event::FileLoaded,
            EVENT_VIDEO_RECONFIG => Event::VideoReconfig,
            EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
            5 => Event::CommandReply { id: ev.reply_userdata, error: err_of(ev.error) },
            EVENT_END_FILE => {
                let ef = unsafe { &*(ev.data as *const RawEventEndFile) };
                Event::EndFile {
                    eof: ef.reason == END_EOF,
                    error: (ef.reason == END_ERROR).then(|| err(self.api, ef.error)),
                }
            }
            EVENT_PROPERTY_CHANGE => {
                let p = unsafe { &*(ev.data as *const RawEventProperty) };
                let value = unsafe {
                    if p.data.is_null() {
                        PropValue::None
                    } else {
                        match p.format {
                            FORMAT_FLAG => PropValue::Flag(*(p.data as *const c_int) != 0),
                            FORMAT_INT64 => PropValue::Int(*(p.data as *const i64)),
                            FORMAT_DOUBLE => PropValue::Double(*(p.data as *const f64)),
                            FORMAT_STRING => {
                                let s = *(p.data as *const *const c_char);
                                PropValue::Str(if s.is_null() { String::new() } else { CStr::from_ptr(s).to_string_lossy().into_owned() })
                            }
                            _ => PropValue::None,
                        }
                    }
                };
                Event::Property { id: ev.reply_userdata, value }
            }
            _ => Event::Other,
        })
    }

    /// Create the OpenGL renderer. Must be called with the GL context current.
    pub fn create_renderer(
        &self,
        get_proc: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void,
        on_update: Box<dyn Fn() + Send + Sync>,
    ) -> Result<Renderer, String> {
        let api_type = cstr("opengl");
        let mut init = OpenGlInitParams { get_proc_address: get_proc, get_proc_address_ctx: std::ptr::null_mut() };
        let mut params = [
            RenderParam { ty: RENDER_PARAM_API_TYPE, data: api_type.as_ptr() as _ },
            RenderParam { ty: RENDER_PARAM_OPENGL_INIT_PARAMS, data: &mut init as *mut _ as _ },
            RenderParam { ty: 0, data: std::ptr::null_mut() },
        ];
        let mut ctx: RenderCtx = std::ptr::null_mut();
        check(self.api, unsafe { (self.api.render_context_create)(&mut ctx, self.h, params.as_mut_ptr()) }, "render context")?;
        let raw = Box::into_raw(Box::new(on_update));
        unsafe { (self.api.render_context_set_update_callback)(ctx, Some(trampoline), raw as _) };
        Ok(Renderer { api: self.api, ctx, callback: raw })
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe { (self.api.terminate_destroy)(self.h) };
    }
}

unsafe extern "C" fn trampoline(data: *mut c_void) {
    let f = unsafe { &*(data as *const Box<dyn Fn() + Send + Sync>) };
    f();
}

/// The OpenGL render context. Every call must happen on the GL thread with the context current.
pub struct Renderer {
    api: &'static Api,
    ctx: RenderCtx,
    callback: *mut Box<dyn Fn() + Send + Sync>,
}

unsafe impl Send for Renderer {}
unsafe impl Sync for Renderer {}

impl Renderer {
    /// True if mpv has a new frame to draw.
    pub fn wants_frame(&self) -> bool {
        unsafe { (self.api.render_context_update)(self.ctx) & RENDER_UPDATE_FRAME != 0 }
    }

    /// Render the current frame into `fbo` (an OpenGL framebuffer name) of size w×h.
    pub fn render(&self, fbo: u32, w: i32, h: i32, flip_y: bool) {
        let mut target = OpenGlFbo { fbo: fbo as c_int, w, h, internal_format: 0 };
        let mut flip: c_int = flip_y as c_int;
        let mut params = [
            RenderParam { ty: RENDER_PARAM_OPENGL_FBO, data: &mut target as *mut _ as _ },
            RenderParam { ty: RENDER_PARAM_FLIP_Y, data: &mut flip as *mut _ as _ },
            RenderParam { ty: 0, data: std::ptr::null_mut() },
        ];
        unsafe { (self.api.render_context_render)(self.ctx, params.as_mut_ptr()) };
    }

    pub fn report_swap(&self) {
        unsafe { (self.api.render_context_report_swap)(self.ctx) };
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            (self.api.render_context_free)(self.ctx);
            drop(Box::from_raw(self.callback));
        }
    }
}

/// mpv's OpenGL loader: WGL extensions first, then the GL 1.1 exports of opengl32.dll.
pub unsafe extern "C" fn wgl_get_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
    use windows_sys::Win32::Graphics::OpenGL::wglGetProcAddress;
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress, LoadLibraryA};
    unsafe {
        if let Some(f) = wglGetProcAddress(name as _) {
            let v = f as usize;
            // wglGetProcAddress signals failure with a few small sentinel values besides 0.
            if !matches!(v, 1 | 2 | 3 | usize::MAX) {
                return v as _;
            }
        }
        let mut m = GetModuleHandleA(c"opengl32.dll".as_ptr() as _);
        if m.is_null() {
            m = LoadLibraryA(c"opengl32.dll".as_ptr() as _);
        }
        GetProcAddress(m, name as _).map_or(std::ptr::null_mut(), |f| f as _)
    }
}

/// Load libmpv now, to surface a missing DLL early with a clear message.
pub fn available() -> Result<(), String> {
    api().map(|_| ())
}

