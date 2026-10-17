@echo off
setlocal
cd /d "%~dp0"
rem Builds dist\RadioMaster-Setup-<version>.exe. Needs Rust and Inno Setup 6 (https://jrsoftware.org/isdl.php).
set ISCC=iscc
where iscc >nul 2>&1 || set ISCC="%ProgramFiles(x86)%\Inno Setup 6\ISCC.exe"
if not exist %ISCC% if not "%ISCC%"=="iscc" (
  echo Inno Setup 6 was not found. Install it from https://jrsoftware.org/isdl.php
  pause
  exit /b 1
)
for /f "tokens=3 delims= " %%v in ('findstr /b /c:"version = " Cargo.toml') do set VER=%%~v
cargo build --release || (echo Build failed. & pause & exit /b 1)
%ISCC% /DAppVersion=%VER% installer\RadioMaster.iss || (echo Installer build failed. & pause & exit /b 1)
echo.
echo Installer: "%~dp0dist\RadioMaster-Setup-%VER%.exe"
pause
endlocal
