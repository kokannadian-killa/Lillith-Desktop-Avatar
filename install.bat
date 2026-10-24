@echo off
rem Lillith installer for Windows: double-click me (or run: install.bat -Uninstall)
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
echo.
pause
