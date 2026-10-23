//! `flick` — command-line front end. Opens videos in the (single) player window,
//! remote-controls it, and runs headless ffprobe/ffmpeg operations.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};

use flick::ipc::{self, ClientError};
use flick::media::{clock, is_url, parse_time, timestamp};
use flick::ops;

#[derive(Parser)]
#[command(
    name = "flick",
    version,
    about = "Flick — simple video player",
    args_conflicts_with_subcommands = true,
    after_help = "Run `flick <command> --help` for details. `flick ctl help` lists player commands."
)]
struct Cli {
    /// Files, folders or URLs to play (reuses the running window).
    files: Vec<String>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Play files, a folder or a URL.
    Open {
        files: Vec<String>,
        /// Start at this time (e.g. 83.5, 1:23, 1m23s).
        #[arg(long)]
        at: Option<String>,
        /// Open paused.
        #[arg(long)]
        paused: bool,
    },
    /// Send a command to the running player (e.g. `flick ctl seek 1:30`). `flick ctl help` lists them.
    Ctl {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        argv: Vec<String>,
    },
    /// Print the player's state as JSON (file, position, duration, clip marks, region, tracks...).
    State,
    /// Save the frame on screen as a PNG (--ui: the whole window with controls).
    Snap {
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Capture the window, controls and all, instead of the bare video frame.
        #[arg(long)]
        ui: bool,
        /// Burn in the visible subtitles.
        #[arg(long)]
        subs: bool,
    },
    /// Describe a media file: container, duration, streams, chapters (via ffprobe).
    Info {
        file: PathBuf,
        #[arg(long)]
        json: bool,
        /// Full ffprobe output.
        #[arg(long)]
        raw: bool,
    },
    /// Extract one frame as an image, without the player.
    Frame {
        file: PathBuf,
        /// Time: 83.5, 1:23, 1m23s, or N% of the duration.
        #[arg(long, default_value = "0", allow_hyphen_values = true)]
        at: String,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Scale to this width (keeps aspect).
        #[arg(long)]
        width: Option<u32>,
    },
    /// Evenly spaced frames tiled into one image: a quick look at a whole video.
    Contact {
        file: PathBuf,
        #[arg(short, long)]
        out: Option<PathBuf>,
        /// Columns x rows.
        #[arg(long, default_value = "4x4")]
        grid: String,
        /// Width of each tile in pixels.
        #[arg(long, default_value_t = 320)]
        width: u32,
    },
    /// Close the player.
    Quit,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => code,
        Err(e) => {
            if let Some(ClientError::NotRunning) = e.downcast_ref::<ClientError>() {
                eprintln!("flick: no player window is running (start one with `flick <file>`)");
                return ExitCode::from(2);
            }
            eprintln!("flick: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn abs(p: &Path) -> PathBuf {
    std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf())
}

fn print(v: &Value) {
    println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
}

/// Send a player command; error if the player reports failure.
fn ctl(argv: Vec<String>) -> Result<Value> {
    let r = ipc::send(&argv).map_err(|e| match e {
        ClientError::NotRunning => anyhow::Error::new(ClientError::NotRunning),
        e => anyhow!("{e}"),
    })?;
    if r.ok { Ok(r.result) } else { bail!("{}", r.error) }
}

/// Make sure a player is running, starting `flickw.exe` detached if needed.
fn ensure_player() -> Result<()> {
    if ipc::is_running() {
        return Ok(());
    }
    let exe = std::env::current_exe()?.with_file_name("flickw.exe");
    if !exe.exists() {
        bail!("flickw.exe not found next to {}", exe.display());
    }
    flick::win::spawn_detached(&exe, &[]).with_context(|| format!("starting {}", exe.display()))?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if ipc::is_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    bail!("the player didn't start")
}

/// Wait until the player has loaded (or failed to load) what it was asked to open.
fn wait_loaded() -> Result<Value> {
    let start = Instant::now();
    loop {
        let st = ctl(vec!["state".into()])?;
        if let Some(e) = st["error"].as_str() {
            bail!("{e}");
        }
        let loaded = st["loading"] == json!(false) && (st["duration"].is_number() || st["position"].is_number());
        if loaded || start.elapsed() > Duration::from_secs(15) {
            return Ok(st);
        }
        std::thread::sleep(Duration::from_millis(40));
    }
}

fn open(files: Vec<String>, at: Option<String>, paused: bool) -> Result<ExitCode> {
    if files.is_empty() {
        ensure_player()?;
        ctl(vec!["focus".into()])?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut argv = vec!["open".to_string()];
    for f in &files {
        if is_url(f) {
            argv.push(f.clone());
        } else {
            let p = abs(Path::new(f));
            if !p.exists() {
                bail!("not found: {}", p.display());
            }
            argv.push(p.display().to_string());
        }
    }
    ensure_player()?;
    ctl(argv)?;
    let st = wait_loaded()?;
    if let Some(t) = at {
        ctl(vec!["seek".into(), t])?;
    }
    if paused {
        ctl(vec!["play".into(), "off".into()])?;
    }
    print(&json!({
        "file": st["file"],
        "duration": st["duration"],
        "video": st["video"],
        "playlist": st["playlist"],
    }));
    Ok(ExitCode::SUCCESS)
}

fn run(cli: Cli) -> Result<ExitCode> {
    let Some(cmd) = cli.cmd else { return open(cli.files, None, false) };
    match cmd {
        Cmd::Open { files, at, paused } => open(files, at, paused),
        Cmd::Ctl { argv } => {
            print(&ctl(argv)?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::State => {
            print(&ctl(vec!["state".into()])?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Snap { out, ui, subs } => {
            let out = abs(&out.unwrap_or_else(|| std::env::temp_dir().join("flick").join("snap.png")));
            let mut argv = vec!["snap".to_string(), out.display().to_string()];
            if ui {
                argv.push("--ui".into());
            }
            if subs {
                argv.push("--subs".into());
            }
            print(&ctl(argv)?);
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Info { file, json, raw } => {
            if raw {
                print(&ops::ffprobe_raw(&file).map_err(|e| anyhow!(e))?);
                return Ok(ExitCode::SUCCESS);
            }
            let v = ops::info(&file).map_err(|e| anyhow!(e))?;
            if json {
                print(&v);
            } else {
                print_info(&v);
            }
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Frame { file, at, out, width } => {
            let t = match at.strip_suffix('%') {
                Some(p) => {
                    let p: f64 = p.parse().map_err(|_| anyhow!("bad percent '{at}'"))?;
                    ops::duration(&file).map_err(|e| anyhow!(e))? * p.clamp(0.0, 100.0) / 100.0
                }
                None => parse_time(&at).ok_or_else(|| anyhow!("bad time '{at}'"))?,
            };
            let out = abs(&out.unwrap_or_else(|| std::env::temp_dir().join("flick").join("frame.png")));
            ops::frame(&file, t, &out, width).map_err(|e| anyhow!(e))?;
            print(&json!({ "path": out, "at": t, "timestamp": timestamp(t) }));
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Contact { file, out, grid, width } => {
            let (c, r) = grid
                .split_once(['x', 'X', '×'])
                .and_then(|(a, b)| Some((a.trim().parse::<u32>().ok()?, b.trim().parse::<u32>().ok()?)))
                .filter(|(a, b)| *a > 0 && *b > 0 && a * b <= 100)
                .ok_or_else(|| anyhow!("--grid COLSxROWS, e.g. 4x4 (at most 100 tiles)"))?;
            let out = abs(&out.unwrap_or_else(|| std::env::temp_dir().join("flick").join("contact.png")));
            let times = ops::contact(&file, &out, c, r, width).map_err(|e| anyhow!(e))?;
            print(&json!({
                "path": out,
                "grid": [c, r],
                "times": times,
                "timestamps": times.iter().map(|t| timestamp(*t)).collect::<Vec<_>>(),
            }));
            Ok(ExitCode::SUCCESS)
        }
        Cmd::Quit => {
            match ipc::send(&["quit".into()]) {
                Ok(_) | Err(ClientError::NotRunning) => {}
                Err(e) => bail!("{e}"),
            }
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn print_info(v: &Value) {
    let s = |v: &Value| match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Number(n) if n.as_f64().is_some_and(|f| f.fract() == 0.0) => format!("{}", n.as_f64().unwrap() as i64),
        o => o.to_string(),
    };
    let lang_of = |x: &Value| x["language"].as_str().filter(|l| *l != "und").map(|l| format!(" [{l}]")).unwrap_or_default();
    println!("{}", s(&v["path"]));
    let dur = v["duration"].as_f64().map(clock).unwrap_or_else(|| "?".into());
    let rate = v["bit_rate"].as_f64().map(|b| format!(", {:.1} Mb/s", b / 1e6)).unwrap_or_default();
    // ffprobe names demuxers by family ("mov,mp4,m4a,..."): the extension is what people expect.
    let container = Path::new(&s(&v["path"]))
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_else(|| s(&v["container"]).split(',').next().unwrap_or("").to_string());
    println!("  {container}  ·  {dur}  ·  {}{rate}", s(&v["size"]));
    for x in v["video"].as_array().into_iter().flatten() {
        let fps = x["fps"].as_f64().map(|f| format!(" @ {f} fps")).unwrap_or_default();
        let rot = x["rotation"].as_f64().filter(|r| *r != 0.0).map(|r| format!(", rotated {r}°")).unwrap_or_default();
        println!("  video #{}: {} {}x{}{fps} {}{rot}", s(&x["index"]), s(&x["codec"]), s(&x["width"]), s(&x["height"]), s(&x["pix_fmt"]));
    }
    for x in v["audio"].as_array().into_iter().flatten() {
        let lang = lang_of(x);
        println!("  audio #{}: {} {} {} Hz{lang}", s(&x["index"]), s(&x["codec"]), s(&x["layout"]), s(&x["sample_rate"]));
    }
    for x in v["subtitles"].as_array().into_iter().flatten() {
        let lang = lang_of(x);
        println!("  subs  #{}: {}{lang}", s(&x["index"]), s(&x["codec"]));
    }
    if let Some(n) = v["chapters"].as_array().map(Vec::len).filter(|n| *n > 0) {
        println!("  {n} chapters");
    }
}

