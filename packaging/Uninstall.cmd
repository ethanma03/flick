@echo off
rem Removes Flick, its file associations and its PATH entry.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\uninstall.ps1"
echo.
pause
