@echo off
rem Lillith voice (offline, local named pipe \\.\pipe\lillith-voice). See INSTALL-windows.md
setlocal
set "LV=%~dp0"
if "%LV:~-1%"=="\" set "LV=%LV:~0,-1%"
rem Runs in this console (Ctrl+C stops it). The installer autostarts it hidden instead:
rem   python\pythonw.exe -m lillith_voice.winlaunch   (HKCU\Software\Microsoft\Windows\CurrentVersion\Run\LillithVoice)
rem --ping / --stop talk to a running daemon. Other args go to lillith-voiced (e.g. --threads 4).
if /i "%~1"=="--ping" goto :ctl
if /i "%~1"=="--stop" goto :ctl
"%LV%\python\python.exe" -m lillith_voice.daemon %*
exit /b %errorlevel%
:ctl
"%LV%\python\python.exe" -m lillith_voice.winlaunch %*
