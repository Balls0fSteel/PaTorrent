@echo off
rem Runs one cargo-fuzz target on Windows: fuzz.bat <target> [seconds]   (default 60 s)
rem fuzz.bat with no arguments lists the targets. Needs the nightly toolchain and cargo-fuzz.
rem The AddressSanitizer runtime DLL comes from Visual Studio; this script puts it on PATH.
rem Crashes land in fuzz\artifacts\<target>\, the growing corpus in fuzz\corpus\<target>\.

setlocal
cd /d "%~dp0"

if "%~1"=="" (
    cargo fuzz list
    exit /b 0
)
set "SECONDS=%~2"
if "%SECONDS%"=="" set "SECONDS=60"

set "VSWHERE=%ProgramFiles(x86)%\Microsoft Visual Studio\Installer\vswhere.exe"
for /f "delims=" %%d in ('call "%VSWHERE%" -latest -products * -find "VC\Tools\MSVC\*\bin\Hostx64\x64\clang_rt.asan_dynamic-x86_64.dll"') do set "ASAN_DLL=%%d"
if not defined ASAN_DLL (
    echo clang_rt.asan_dynamic-x86_64.dll not found. Install the "C++ AddressSanitizer" component of Visual Studio.
    exit /b 1
)
for %%f in ("%ASAN_DLL%") do set "PATH=%%~dpf;%PATH%"

rem Seed the metadata targets with the test .torrent files on the first run.
if /i "%~1"=="torrent_info" call :seed torrent_info
if /i "%~1"=="bdecode" call :seed bdecode

cargo +nightly fuzz run %1 -- -max_total_time=%SECONDS%
exit /b %errorlevel%

:seed
if exist fuzz\corpus\%1 exit /b 0
mkdir fuzz\corpus\%1
copy /y crates\core\tests\fixtures\torrents\*.torrent fuzz\corpus\%1\ >nul
exit /b 0
