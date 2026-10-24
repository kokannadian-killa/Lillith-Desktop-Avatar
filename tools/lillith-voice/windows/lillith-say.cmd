@echo off
rem Lillith voice (offline, local named pipe \\.\pipe\lillith-voice). See INSTALL-windows.md
setlocal
set "LV=%~dp0"
if "%LV:~-1%"=="\" set "LV=%LV:~0,-1%"
"%LV%\python\python.exe" -m lillith_voice.cli %*
