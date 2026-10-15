@echo off
rem Starts the LibreTorrent window on the simulated engine with 2,000 fake torrents, to look
rem at the interface. Nothing is downloaded. Pass a number for a different count:
rem run-demo.bat 10000

setlocal
set COUNT=%~1
if "%COUNT%"=="" set COUNT=2000
call "%~dp0run.bat" --demo %COUNT%
endlocal
