# Flick

Personal video player for Windows (Rust, eframe/egui on glow + libmpv). It's a sibling of Glint (`..\glint`) with the same look, structure and CLI conventions. Edits happen elsewhere (ffmpeg, usually run by Claude Code); Flick is the viewer, and it's built to be driven from the command line.

## Layout
- `src/main.rs`: `flick.exe`, a console CLI. It remote-controls the player over a named pipe and runs headless ffprobe/ffmpeg ops.
- `src/gui_main.rs`: `flickw.exe`, the windowed player (no console). File associations point here. It is single-instance: a second launch forwards its files to the first window and exits.
- `src/ipc.rs`: the named pipe `\\.\pipe\flick-<user>`. The protocol is one JSON line per request and one per reply (same as Glint).
- `src/mpv.rs`: a minimal libmpv binding, loaded at runtime with libloading from `libmpv-2.dll` next to the exe (or `vendor/mpv`). It covers the client API and the OpenGL render API.
- `src/app/`: the player.
  - `video.rs`: mpv renders into an offscreen FBO inside an egui paint callback, which is then blitted into the frame. egui overlays draw on top, and UI-only repaints re-blit the cached frame.
  - `player.rs`: the mpv options and the `Playback` mirror of observed properties (ids in `OBSERVED`).
  - `commands.rs`: the command language shared by `flick ctl`, the Ctrl+P palette and keybindings. Add new commands here and to `COMMANDS`. `state_json` is what `flick state` prints.
  - `controls.rs`: the auto-hiding control bar, seek bar, empty/error screens and the region overlay.
  - `scrub.rs`: seek-bar scrubbing. It keeps one seek in flight (the next goes out on mpv's playback-restart) and picks keyframe or exact seeks from a per-file keyframe index (`ops::keyframes`, read in the background) and a learned decode cost, so the picture never steps away from the pointer. The `live` test drives it against headless mpv. `FLICK_SCRUB_LOG=path` logs each seek and its latency.
  - `preview.rs`: seek-bar hover/drag previews. On the first hover, a below-normal-priority ffmpeg (`ops::thumbnails`, which decodes keyframes only) streams up to about 300 small frames, and the one nearest the pointer is drawn above the time label.
  - `drive.rs`: test-only scripted seek-bar input (`flick ctl drive ms:x:down:shift,...`, only when the player runs with `FLICK_DEV` set). It injects through egui's own input and never touches the real mouse.
  - `hud.rs`: toasts, the details panel, palette, help, context menu and modals.
- `src/ops.rs`: headless `info` (ffprobe summary), `frame` and `contact` (tiled frames).
- `src/media.rs`: extensions, folder playlists (natural sort), time formatting and parsing.

## Build / test / install
- First time: `pwsh scripts/fetch-mpv.ps1` downloads libmpv into `vendor/mpv` (gitignored). The pinned tag is in the script, and `-Latest` bumps it. `build.rs` copies the DLL next to the built exes.
- `cargo build` (debug; deps are optimized via profile override). `cargo test` runs the unit tests plus `tests/ops.rs`, which generates a clip with ffmpeg and skips itself if ffmpeg is missing.
- Run a dev build: `target\debug\flick.exe <video>`. Quit it with `target\debug\flick.exe quit` before rebuilding, because the running exe is locked.
- Install: `pwsh scripts/install.ps1`. It builds release, copies the exes and the DLL to `%LOCALAPPDATA%\Programs\Flick`, adds that folder to PATH and registers per-user file associations.
- Update after code changes: `pwsh scripts/update.ps1`. It rebuilds, reinstalls and reopens what was playing at the same position.
- Default app: `scripts/set-default.ps1` (one UAC prompt) points the HKLM DefaultAssociationsConfiguration policy at `%ProgramData%\Flick\default-associations.xml`. It **merges** the entries of whatever file the policy pointed at before (Glint's), because there is only one policy per machine.
- Settings live in `%LOCALAPPDATA%\Flick\settings.json`, not in eframe storage, because mpv needs the volume before the window exists. Resume positions go in `%LOCALAPPDATA%\Flick\watch_later`.
- Background mode (on by default): closing the window stops playback and hides it to the tray. `flick quit` really exits.

## Driving the player (for Claude)
All commands print JSON. Exit code 2 means no player is running.
```
flick <file|folder|url>                # play (starts the player if needed; waits until loaded)
flick open clip.mp4 --at 1:23 --paused
flick state                            # file, position(_ts), duration, paused, clip{start,end,ffmpeg}, region{x,y,w,h,ffmpeg}, video, tracks, playlist
flick snap -o out.png [--subs]         # the current video frame at native resolution -> Read it
flick snap -o out.png --ui             # the whole window, controls and all
flick ctl seek 1:23 | seek +5 | seek 50% | play on|off | frame next|prev | speed 0.5
flick ctl clip 12.5 18 | clip start | clip end 0:20 | clip off
flick ctl select 100,50,640,360 | select off      # region in video pixels (display orientation)
flick ctl audio next | sub 2 | sub off | sub-add x.srt | rotate 90 | loop on | volume 60
flick ctl help                         # every player command
flick info video.mp4 [--json|--raw]    # container, duration, size, streams, chapters (ffprobe)
flick frame video.mp4 --at 12.5 -o f.png [--width 640]   # without the player
flick contact video.mp4 -o sheet.png --grid 4x4          # whole video at a glance
```
- The user marks things in the player: `[` and `]` for a clip, `C`+drag for a region. Read them back with `flick state` and use `clip.ffmpeg` and `region.ffmpeg` directly in ffmpeg commands.
- Write edits to a new file and open it with `flick out.mp4`. Overwriting the file being shown also works: Flick reloads it at the same position once the writes settle.
