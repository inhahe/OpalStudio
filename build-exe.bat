@echo off
rem Builds the release version of Opal Studio and puts opal-studio.exe in this folder.
setlocal
cd /d "%~dp0"
cargo build --release -p opal-app
if errorlevel 1 (
    echo The build failed; opal-studio.exe was not updated.
    exit /b 1
)
set "SRC=target\x86_64-pc-windows-msvc\release\opal-studio.exe"
set "DST=opal-studio.exe"
rem A copy that is running cannot be overwritten, but it can be renamed out of the way.
if exist "opal-studio.old.exe" del "opal-studio.old.exe" 2>nul
copy /y "%SRC%" "%DST%" >nul 2>nul
if errorlevel 1 (
    move /y "%DST%" "opal-studio.old.exe" >nul
    copy /y "%SRC%" "%DST%" >nul
)
if errorlevel 1 (
    echo Could not copy the new opal-studio.exe here.
    exit /b 1
)
echo opal-studio.exe is up to date.
