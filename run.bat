@echo off
rem Starts the PaTorrent window from dist\, building it first if needed.
rem Arguments are passed on: run.bat file.torrent, run.bat --demo 2000, ...
rem Settings and torrents live in %APPDATA%\PaTorrent unless dist\patorrent.ini exists
rem (then everything stays in dist\, portable).

setlocal
cd /d "%~dp0"

if not exist dist\patorrent.exe (
    call build.bat
    if errorlevel 1 exit /b 1
)

start "" "%CD%\dist\patorrent.exe" %*
endlocal
