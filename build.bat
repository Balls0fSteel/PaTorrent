@echo off
rem Builds PaTorrent (release) and copies the programs into dist\.
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

rem The version the programs show (D37): the crate version plus -dev.<commit>, and .dirty when
rem the tree has uncommitted changes, so test builds can be told apart. A tagged release gets
rem no suffix. Without git, cargo's own fallback applies (crates\frontend\build.rs).
set PATORRENT_BUILD=
set VERSION=
set DIRTY=
for /f "tokens=3" %%v in ('findstr /b /c:"version = " Cargo.toml') do if not defined VERSION set VERSION=%%~v
where git >nul 2>nul || goto :versioned
for /f %%h in ('git rev-parse --short=7 HEAD 2^>nul') do set PATORRENT_BUILD=-dev.%%h
for /f %%t in ('git tag --points-at HEAD 2^>nul') do if "%%t"=="v%VERSION%" set PATORRENT_BUILD=
if not defined PATORRENT_BUILD goto :versioned
for /f "delims=" %%s in ('git status --porcelain --untracked-files=no 2^>nul') do set DIRTY=1
if defined DIRTY set PATORRENT_BUILD=%PATORRENT_BUILD%.dirty
:versioned

echo Building PaTorrent %VERSION%%PATORRENT_BUILD% (%PROFILE%)...
cargo build %FLAGS% --workspace
if errorlevel 1 (
    echo.
    echo Build failed.
    exit /b 1
)

if not exist dist mkdir dist
for %%f in (patorrent.exe patorrent-nox.exe lt-swarm.exe) do (
    copy /y "target\%PROFILE%\%%f" dist\ >nul
    if errorlevel 1 (
        echo Could not copy %%f to dist\. Is PaTorrent still running? Exit it and try again.
        exit /b 1
    )
)
copy /y LICENSE.md dist\ >nul
copy /y THIRD-PARTY-NOTICES.md dist\ >nul

echo.
echo Done. Programs are in %CD%\dist:
dir /b dist\*.exe
endlocal
