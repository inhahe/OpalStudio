@echo off
rem Publishes a release of Opal Studio at github.com/inhahe/OpalStudio: the commit checked out,
rem tagged v<version> (the version in Cargo.toml's [workspace.package]), with a zip of the
rem release build and what goes with it. Nothing is published if that version is released
rem already, if the work is not all committed and pushed, or if the build fails.
setlocal EnableExtensions
cd /d "%~dp0"
set "REPO=inhahe/OpalStudio"

rem ---- The version: the first "version = " line of Cargo.toml, its workspace's.
set "VERSION="
for /f "tokens=2 delims==" %%a in ('findstr /b /c:"version = " Cargo.toml') do (
    if not defined VERSION set "VERSION=%%a"
)
if not defined VERSION (
    echo Cargo.toml says no version.
    exit /b 1
)
set "VERSION=%VERSION: =%"
set "VERSION=%VERSION:"=%"
set "PYVERSION="
for /f "tokens=2 delims==" %%a in ('findstr /b /c:"version = " python\pyproject.toml') do (
    if not defined PYVERSION set "PYVERSION=%%a"
)
set "PYVERSION=%PYVERSION: =%"
set "PYVERSION=%PYVERSION:"=%"
if not "%PYVERSION%"=="%VERSION%" (
    echo Cargo.toml says %VERSION% but python\pyproject.toml says %PYVERSION%: make them the same.
    exit /b 1
)
set "TAG=v%VERSION%"

rem ---- Not released already.
gh release view "%TAG%" -R %REPO% >nul 2>nul
if not errorlevel 1 (
    echo %TAG% is released already: bump the version first.
    exit /b 1
)

rem ---- What is released is what is on GitHub: nothing uncommitted, and the commit pushed.
git diff --quiet HEAD
if errorlevel 1 (
    echo There are changes not committed: commit them first.
    exit /b 1
)
for /f %%c in ('git rev-parse HEAD') do set "COMMIT=%%c"
git fetch -q origin
set "PUSHED="
for /f %%b in ('git branch -r --contains %COMMIT%') do set "PUSHED=1"
if not defined PUSHED (
    echo %COMMIT% is not pushed: push it first.
    exit /b 1
)

rem ---- The release build, and what goes with it, zipped.
call build-exe.bat
if errorlevel 1 exit /b 1
set "NAME=OpalStudio-%VERSION%-windows-x64"
set "STAGE=target\release-package"
set "OUT=%STAGE%\%NAME%"
if exist "%STAGE%" rd /s /q "%STAGE%"
mkdir "%OUT%\python\opal"
copy /y opal-studio.exe "%OUT%\" >nul || goto :gather_failed
copy /y opal-studio.com "%OUT%\" >nul || goto :gather_failed
copy /y README.md "%OUT%\" >nul || goto :gather_failed
copy /y LICENSE "%OUT%\" >nul || goto :gather_failed
copy /y assets\fonts\LICENSE-FluentSystemIcons.txt "%OUT%\" >nul || goto :gather_failed
copy /y python\API.md "%OUT%\python\" >nul || goto :gather_failed
copy /y python\pyproject.toml "%OUT%\python\" >nul || goto :gather_failed
copy /y python\opal\*.py "%OUT%\python\opal\" >nul || goto :gather_failed
copy /y python\opal\*.pyi "%OUT%\python\opal\" >nul || goto :gather_failed
copy /y python\opal\py.typed "%OUT%\python\opal\" >nul || goto :gather_failed
powershell -NoProfile -Command "Compress-Archive -Path '%OUT%' -DestinationPath '%OUT%.zip' -Force"
if errorlevel 1 (
    echo Could not zip the files to release.
    exit /b 1
)

rem ---- The notes: what was committed since the last release, or that it is the first.
set "NOTES=%STAGE%\notes.md"
set "LAST="
for /f %%t in ('git describe --tags --abbrev^=0 2^>nul') do set "LAST=%%t"
if defined LAST (
    echo Changes since %LAST%:> "%NOTES%"
    echo.>> "%NOTES%"
    git log --format="- %%s" %LAST%..%COMMIT% >> "%NOTES%"
) else (
    echo The first release of Opal Studio. README.md says what it does, and how to drive it from Python.> "%NOTES%"
)
echo.>> "%NOTES%"
echo Unzip it anywhere and run opal-studio.exe. For scripts, put its python folder on PYTHONPATH, or pip install it.>> "%NOTES%"

gh release create "%TAG%" "%OUT%.zip" -R %REPO% --target %COMMIT% --title "Opal Studio %VERSION%" --notes-file "%NOTES%"
if errorlevel 1 (
    echo The release was not published.
    exit /b 1
)
git fetch -q origin --tags
rd /s /q "%STAGE%"
echo Released %TAG%.
exit /b 0

:gather_failed
echo Could not gather the files to release.
exit /b 1
