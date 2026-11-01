@echo off
rem Installs Flick for the current user. No admin rights are needed, apart from one optional UAC prompt that makes Flick your default app.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\install.ps1" %*
echo.
pause
