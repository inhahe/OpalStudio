@echo off
rem Builds the release version of Opal Studio and puts opal-studio.exe in this folder, with
rem opal-studio.com, its launcher for command prompts.
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
rem The console launcher, which a command prompt finds before the exe and waits for.
set "SRC=target\x86_64-pc-windows-msvc\release\opal-studio-console.exe"
set "DST=opal-studio.com"
if exist "opal-studio.old.com" del "opal-studio.old.com" 2>nul
copy /y "%SRC%" "%DST%" >nul 2>nul
if errorlevel 1 (
    move /y "%DST%" "opal-studio.old.com" >nul
    copy /y "%SRC%" "%DST%" >nul
)
if errorlevel 1 (
    echo Could not copy the new opal-studio.com here.
    exit /b 1
)
echo opal-studio.exe and opal-studio.com are up to date.
