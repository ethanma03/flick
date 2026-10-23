<#
.SYNOPSIS
  Build Flick, install it to %LOCALAPPDATA%\Programs\Flick, put `flick` on PATH and
  register it for video and audio files (per-user, no admin needed).

.PARAMETER NoBuild       Use the existing target\release binaries.
.PARAMETER NoSettings    Don't run set-default.ps1 (the default-app policy, one UAC prompt).
.PARAMETER NoBackground  Don't start Flick at login / keep it in the tray.
#>
param([switch]$NoBuild, [switch]$NoSettings, [switch]$NoBackground)
$ErrorActionPreference = 'Stop'

$repo = Split-Path $PSScriptRoot -Parent
$dest = Join-Path $env:LOCALAPPDATA 'Programs\Flick'
$gui = Join-Path $dest 'flickw.exe'
$cli = Join-Path $dest 'flick.exe'

# Keep in sync with media::VIDEO_EXTENSIONS / AUDIO_EXTENSIONS.
$video = @('mp4','m4v','mkv','webm','mov','avi','wmv','flv','mpg','mpeg','m2ts','mts','ts','3gp','ogv',
           'vob','divx','asf','rm','rmvb','f4v','y4m','gifv')
$audio = @('mp3','m4a','aac','flac','wav','ogg','oga','opus','wma','aiff','aif','ape','wv','mka','ac3','dts')

if (-not (Test-Path (Join-Path $repo 'vendor\mpv\libmpv-2.dll'))) {
    & (Join-Path $PSScriptRoot 'fetch-mpv.ps1')
}
if (-not $NoBuild) {
    Write-Host 'Building release...' -ForegroundColor Cyan
    Push-Location $repo
    try { cargo build --release; if ($LASTEXITCODE) { throw 'cargo build failed' } } finally { Pop-Location }
}

# Close a running player so its files can be replaced.
if (Test-Path $cli) { & $cli quit 2>$null | Out-Null }
$deadline = (Get-Date).AddSeconds(5)
while ((Get-Process flickw -ErrorAction SilentlyContinue) -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
Get-Process flickw -ErrorAction SilentlyContinue | Stop-Process -Force

New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item (Join-Path $repo 'target\release\flick.exe') $cli -Force
Copy-Item (Join-Path $repo 'target\release\flickw.exe') $gui -Force
$dll = Join-Path $repo 'vendor\mpv\libmpv-2.dll'
$installedDll = Join-Path $dest 'libmpv-2.dll'
if (-not (Test-Path $installedDll) -or (Get-Item $dll).LastWriteTime -ne (Get-Item $installedDll).LastWriteTime) {
    Copy-Item $dll $installedDll -Force
}
Write-Host "Installed to $dest" -ForegroundColor Green

# PATH (user scope).
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (($userPath -split ';') -notcontains $dest) {
    [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ";$dest"), 'User')
    Write-Host "Added $dest to your PATH (open a new terminal to use 'flick')."
}

function Set-Key([string]$path, [hashtable]$values) {
    $full = "HKCU:\$path"
    if (-not (Test-Path $full)) { New-Item -Path $full -Force | Out-Null }
    foreach ($k in $values.Keys) {
        Set-ItemProperty -Path $full -Name $k -Value $values[$k]
    }
}

$open = "`"$gui`" `"%1`""
$icon = "`"$gui`",0"

foreach ($kind in @(@{ id = 'Flick.Video'; name = 'Video' }, @{ id = 'Flick.Audio'; name = 'Audio' })) {
    Set-Key "Software\Classes\$($kind.id)" @{ '(default)' = "$($kind.name) (Flick)"; 'FriendlyTypeName' = $kind.name }
    Set-Key "Software\Classes\$($kind.id)\DefaultIcon" @{ '(default)' = $icon }
    Set-Key "Software\Classes\$($kind.id)\shell\open" @{ 'FriendlyAppName' = 'Flick' }
    Set-Key "Software\Classes\$($kind.id)\shell\open\command" @{ '(default)' = $open }
}

# Application registration (shows up in "Open with" and Default Apps).
Set-Key 'Software\Classes\Applications\flickw.exe' @{ 'FriendlyAppName' = 'Flick' }
Set-Key 'Software\Classes\Applications\flickw.exe\DefaultIcon' @{ '(default)' = $icon }
Set-Key 'Software\Classes\Applications\flickw.exe\shell\open\command' @{ '(default)' = $open }
Set-Key 'Software\Flick\Capabilities' @{
    'ApplicationName' = 'Flick'
    'ApplicationDescription' = 'Simple, fast video player'
    'ApplicationIcon' = $icon
}
Set-Key 'Software\RegisteredApplications' @{ 'Flick' = 'Software\Flick\Capabilities' }

foreach ($e in $video + $audio) {
    $progId = if ($video -contains $e) { 'Flick.Video' } else { 'Flick.Audio' }
    Set-Key "Software\Classes\.$e\OpenWithProgids" @{ $progId = '' }
    Set-Key 'Software\Flick\Capabilities\FileAssociations' @{ ".$e" = $progId }
    Set-Key 'Software\Classes\Applications\flickw.exe\SupportedTypes' @{ ".$e" = '' }
}
# Folders: "Play with Flick" plays every video inside, in order.
Set-Key 'Software\Classes\Directory\shell\Flick.Play' @{ '(default)' = 'Play with Flick'; 'Icon' = $icon }
Set-Key 'Software\Classes\Directory\shell\Flick.Play\command' @{ '(default)' = $open }

# Tell Explorer that associations changed.
Add-Type -Namespace Flick -Name Shell -MemberDefinition @'
[DllImport("shell32.dll")] public static extern void SHChangeNotify(int eventId, uint flags, System.IntPtr a, System.IntPtr b);
'@ -ErrorAction SilentlyContinue
[Flick.Shell]::SHChangeNotify(0x08000000, 0, [IntPtr]::Zero, [IntPtr]::Zero)

Write-Host "Registered Flick for $($video.Count) video and $($audio.Count) audio types." -ForegroundColor Green

# Background mode: start hidden in the tray at login so opening a video is instant.
$run = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
if ($NoBackground) {
    Remove-ItemProperty $run -Name 'Flick' -ErrorAction SilentlyContinue
} else {
    Set-ItemProperty $run -Name 'Flick' -Value "`"$gui`" --background"
    Start-Process $gui -ArgumentList '--background'
    Write-Host 'Flick is running in the tray and will start at login.'
}

$policy = (Get-ItemProperty 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\System' -ErrorAction SilentlyContinue).DefaultAssociationsConfiguration
$hasFlick = $policy -and (Test-Path $policy) -and ((Get-Content -Raw $policy) -match 'Flick\.Video')
if (-not $NoSettings -and -not $hasFlick) {
    & (Join-Path $PSScriptRoot 'set-default.ps1')
}
