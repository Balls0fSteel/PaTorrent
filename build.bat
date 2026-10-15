@echo off
rem Builds LibreTorrent (release) and copies the programs into dist\.
rem   build.bat          release build (small, optimized; takes about a minute)
rem   build.bat debug    debug build (faster to compile; the GUI gets a console window)

setlocal
cd /d "%~dp0"

where cargo >nul 2>nul
if errorlevel 1 (
    echo cargo was not found. Install Rust from https://rustup.rs and open a new terminal.
    exit /b 1
)

set PROFILE=release
set FLAGS=--release
if /i "%~1"=="debug" (
    set PROFILE=debug
    set FLAGS=
)

echo Building LibreTorrent (%PROFILE%)...
cargo build %FLAGS% --workspace
if errorlevel 1 (
    echo.
    echo Build failed.
    exit /b 1
)

if not exist dist mkdir dist
for %%f in (libretorrent.exe libretorrent-nox.exe lt-swarm.exe) do (
    copy /y "target\%PROFILE%\%%f" dist\ >nul
    if errorlevel 1 (
        echo Could not copy %%f to dist\. Is LibreTorrent still running? Exit it and try again.
        exit /b 1
    )
)
copy /y THIRD-PARTY-NOTICES.md dist\ >nul

echo.
echo Done. Programs are in %CD%\dist:
dir /b dist\*.exe
endlocal
