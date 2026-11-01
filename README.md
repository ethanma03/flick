# Flick

A simple, fast video player for Windows, Glint's sibling. It plays everything (it's built on libmpv), has a minimal control bar that stays out of the way, and is easy to drive from the command line. That makes it a good viewer to keep open next to Claude Code when you're cutting, cropping or converting videos.

- **Plays anything**: every format mpv and FFmpeg handle, with hardware decoding. Subtitles next to the video load automatically, and you can drop an `.srt` onto the window to add one. Audio files work too. Paste a link with Ctrl+V; links like YouTube need `yt-dlp` on PATH.
- **Calm UI**: the controls fade out while a video plays and come back when you move the mouse. It uses the same dark, amber-accented look as Glint.
- **Daily-use niceties**:
  - resumes where you left off;
  - `N`/`P` move through the folder, and the next file plays when one ends;
  - frame stepping, speed control, loop;
  - always-on-top;
  - screenshots;
  - the display stays awake while a video plays.
- **Made for editing alongside Claude**:
  - `[` and `]` mark a clip range, which loops while you check it;
  - `C` lets you drag out a crop region in video pixels;
  - `flick state` reports both as ready-to-use `ffmpeg` arguments (`-ss … -to …`, `crop=w:h:x:y`).
  - When a file changes on disk, Flick reloads it at the same position, so a re-encode shows up on its own.
- **Instant**: Flick runs in the tray after you close it, so double-clicking a video opens it right away. Closing the window stops playback.

## Download
Get the latest build from **[openwith.dev](https://openwith.dev)** or the [Releases page](https://github.com/ethanma03/flick/releases/latest). Unzip `Flick-win-x64.zip` and double-click `Install.cmd`. It installs for your user account only, and one optional UAC prompt makes Flick your default app. Flick isn't code-signed yet, so SmartScreen may ask you to click **More info**, then **Run anyway**.

## Build from source
```powershell
pwsh scripts\install.ps1     # fetch libmpv, build, install to %LOCALAPPDATA%\Programs\Flick, register file types
pwsh scripts\update.ps1      # later: pull, rebuild, reinstall, resume what was playing
```
`install.ps1` also runs `scripts\set-default.ps1`. That script makes Flick the default for video files through Windows' default-associations policy: you approve one UAC prompt, and the change applies at your next sign-in. The policy also holds Glint's image associations, which are kept. Audio files and `.ts` are only added to "Open with". Undo with `set-default.ps1 -Remove`. Pass `-NoBackground` to install if you don't want the tray/autostart.

Building needs Rust and the GitHub CLI (`gh`). The CLI fetches libmpv from shinchiro's mpv-winbuild releases into `vendor\mpv`. `flick info`, `frame` and `contact` need `ffmpeg`/`ffprobe` on PATH.

## Keys
Press **F1** in the player for the full list. The basics:
- `Space` / click: play or pause. Double-click or `F`: fullscreen.
- `←/→`: 5 s (with Shift: 1 s, with Ctrl: 1 min). `,`/`.` step one frame.
- `↑/↓` or the wheel: volume. `M`: mute.
- `-`/`=`: speed. `L`: loop.
- `[`/`]`/`\`: clip marks. `C`: region.
- `N`/`P`: next or previous file.
- `S`/`A`: subtitle or audio track.
- `Ctrl+C`: copy the timestamp. `Ctrl+P`: command palette.

## Releases
Run `pwsh scripts\package.ps1` to build a release zip. It writes `dist\Flick-win-x64.zip`, which contains the binaries, the install scripts, `Install.cmd` and `Uninstall.cmd`.

## License
MIT. See [LICENSE](LICENSE). Release packages bundle `libmpv-2.dll` from [shinchiro/mpv-winbuild-cmake](https://github.com/shinchiro/mpv-winbuild-cmake), which is GPL-2.0-or-later as built. `THIRD-PARTY-NOTICES.txt` in the package has the details.
