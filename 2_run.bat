@echo off
rem Starts the Rookparcel window from dist\, building it first if needed.
rem Arguments are passed on: 2_run.bat file.torrent, 2_run.bat --demo 2000, ...
rem Settings and torrents live in %APPDATA%\Rookparcel unless dist\rookparcel.ini exists
rem (then everything stays in dist\, portable).

setlocal
cd /d "%~dp0"

if not exist dist\rookparcel.exe (
    call "%~dp01_build.bat"
    if errorlevel 1 exit /b 1
)

start "" "%CD%\dist\rookparcel.exe" %*
endlocal
