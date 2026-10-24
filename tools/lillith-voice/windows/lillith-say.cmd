@echo off
rem Lillith voice (offline). Untested on Windows; see INSTALL-windows.md
set "LV=%LOCALAPPDATA%\Lillith\voice"
set "PYTHONPATH=%LV%\lib"
set "LILLITH_VOICE_HOME=%LV%"
"%LV%\python\python.exe" -s -m lillith_voice.cli %*
