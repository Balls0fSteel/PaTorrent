@echo off
rem Builds Rookparcel (release, as 1_build.bat), makes the Windows installer from it and runs
rem it, to upgrade the Rookparcel installed on this PC to the code here. The setup finds the
rem installation and installs over it (settings, torrents and associations stay); exit Rookparcel
rem first, or answer the setup's Retry when it asks.
rem   3_build_and_setup.bat        the setup's usual pages
rem   3_build_and_setup.bat /S     silently (other setup switches are passed on too:
rem                                packaging\windows\README.md)
rem Needs NSIS 3 (makensis on PATH): scoop install nsis, or https://nsis.sourceforge.io.
rem The setup is left in dist\.

setlocal
cd /d "%~dp0"

where makensis >nul 2>nul
if errorlevel 1 (
    echo makensis was not found. Install NSIS 3 ^(scoop install nsis, or
    echo https://nsis.sourceforge.io^) and open a new terminal.
    exit /b 1
)

call "%~dp01_build.bat"
if errorlevel 1 exit /b 1

rem The version as the build names it (1.1.35-dev.81f1480, .dirty for uncommitted changes)
set VERSION=
for /f "tokens=2" %%v in ('dist\rookparcel-nox.exe --version') do if not defined VERSION set VERSION=%%v
if not defined VERSION (
    echo Could not read the version from dist\rookparcel-nox.exe.
    exit /b 1
)
set /p FAMILY=<assets\patina-icons\app-icon.txt
set SETUP=dist\rookparcel-%VERSION%-windows-x64-setup.exe

echo.
echo Making %SETUP%...
makensis /NOCD /WX /V2 /DVERSION=%VERSION% /DBINDIR=target\release "/DICON=assets\patina-icons\exports\app\%FAMILY%.ico" "/DOUTFILE=%SETUP%" packaging\windows\rookparcel.nsi
if errorlevel 1 (
    echo.
    echo Making the setup failed.
    exit /b 1
)

echo.
echo Running %SETUP% %*
start "" /wait "%CD%\%SETUP%" %*
if errorlevel 1 (
    echo The setup ended with code %errorlevel% ^(cancelled, or it failed^).
    exit /b 1
)
echo Done.
endlocal
