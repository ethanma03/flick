// GUI entry point (no console window). Explorer file associations launch this.
#![windows_subsystem = "windows"]

use std::time::{Duration, Instant};

use flick::ipc::{self, ClientError};

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    // `--background`: login autostart. Start hidden in the tray; do nothing if already running.
    let background = args.first().map(String::as_str) == Some("--background");
    if background {
        args.remove(0);
    }
    let startup = startup_commands(&args);
    // Paths are absolute now; don't keep the launch folder locked against rename/delete.
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        let _ = std::env::set_current_dir(dir);
    }

    match ipc::Server::bind() {
        Some(server) => {
            if let Err(e) = flick::mpv::available() {
                flick::win::message_box(&format!("Flick can't play video without libmpv.\n\n{e}"));
                return;
            }
            if let Err(e) = flick::app::run(server, startup, background && args.is_empty()) {
                flick::win::message_box(&format!("Flick couldn't start:\n\n{e}"));
            }
        }
        // Another Flick window owns the pipe: hand the request over and exit immediately.
        None if background => {}
        None => {
            let cmds = if startup.is_empty() { vec![vec!["focus".to_string()]] } else { startup };
            for argv in cmds {
                let start = Instant::now();
                loop {
                    match ipc::send(&argv) {
                        Err(ClientError::NotRunning) if start.elapsed() < Duration::from_secs(3) => {
                            std::thread::sleep(Duration::from_millis(30));
                        }
                        _ => break,
                    }
                }
            }
        }
    }
}

/// `flickw a.mp4 b.mp4` → open them; URLs pass through untouched.
fn startup_commands(args: &[String]) -> Vec<Vec<String>> {
    let files: Vec<String> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(|a| if flick::media::is_url(a) { a.clone() } else { flick::app::absolute(std::path::Path::new(a)).display().to_string() })
        .collect();
    if files.is_empty() {
        return vec![];
    }
    let mut v = vec!["open".to_string()];
    v.extend(files);
    vec![v]
}
