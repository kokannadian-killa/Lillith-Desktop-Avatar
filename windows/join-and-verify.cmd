@echo off
rem Joins Lillith-main-windows.zip.part1, .part2, ... (in this folder) back into
rem Lillith-main-windows.zip and checks it against Lillith-main-windows.zip.sha256.
rem Usage: double-click it in the folder with the parts.
setlocal EnableExtensions EnableDelayedExpansion
cd /d "%~dp0"
set "Z=Lillith-main-windows.zip"
if not exist "%Z%.sha256" (echo error: %Z%.sha256 is missing; put every release file in this folder & goto :fail)
if not exist "%Z%.part1" (echo error: no %Z%.part1 here & goto :fail)
set "LIST="
set /a N=0
for /l %%i in (1,1,99) do (
  if exist "%Z%.part%%i" (
    set /a N+=1
    if defined LIST (set "LIST=!LIST!+"%Z%.part%%i"") else (set "LIST="%Z%.part%%i"")
  )
)
echo joining %N% parts -^> %Z%
copy /b /y !LIST! "%Z%.joining" >nul || (echo error: joining failed & goto :fail)
move /y "%Z%.joining" "%Z%" >nul || goto :fail
set "WANT="
for /f "usebackq tokens=1" %%h in ("%Z%.sha256") do if not defined WANT set "WANT=%%h"
set "GOT="
for /f "skip=1 tokens=* delims=" %%h in ('certutil -hashfile "%Z%" SHA256') do if not defined GOT set "GOT=%%h"
set "GOT=%GOT: =%"
if /i "%GOT%"=="%WANT%" (
  echo OK: %Z% is complete ^(sha256 %WANT%^). You can delete the .part files now.
  echo Unzip it to a short folder such as C:\Lillith, then run install.bat there.
  pause
  exit /b 0
)
echo error: checksum mismatch ^(got %GOT%, want %WANT%^); download the parts again
:fail
pause
exit /b 1
