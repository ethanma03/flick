Flick
=====

Flick is a simple, fast video player for Windows.
It is part of openwith: https://openwith.dev

Installing
----------
1. Unzip this folder anywhere, for example to your Downloads folder.
2. Double-click Install.cmd.

The installer copies Flick to %LOCALAPPDATA%\Programs\Flick, adds the `flick`
command to your PATH and registers Flick for videos. It installs for your
user account only and does not need administrator rights.

At the end, Windows shows one UAC prompt. If you approve it, Flick becomes the
default app for videos the next time you sign in. If you decline it,
Flick is still installed and appears under "Open with".

Flick keeps running in the system tray after you close its window, so the next
file opens instantly. It also starts at sign-in. To turn this off, run the
installer from a terminal with -NoBackground:

    powershell -ExecutionPolicy Bypass -File scripts\install.ps1 -NoBackground

Windows SmartScreen
-------------------
Flick is not code-signed yet, so Windows may say that it "protected your PC"
the first time you run something from this folder. Click "More info", then
"Run anyway". The source code is public at https://github.com/ethanma03/flick.

Uninstalling
------------
Double-click Uninstall.cmd in this folder, or run
%LOCALAPPDATA%\Programs\Flick\scripts\uninstall.ps1. Your settings stay in
%LOCALAPPDATA%\Flick in case you reinstall.

Press F1 inside Flick to see every keyboard shortcut.
