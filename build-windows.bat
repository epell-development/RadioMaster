@echo off
setlocal
cd /d "%~dp0"
where cargo >nul 2>&1
if errorlevel 1 (
  echo Rust Cargo is not installed or is not on PATH.
  echo Install Rust from https://rustup.rs/ then reopen Command Prompt.
  pause
  exit /b 1
)
cargo build --release
if errorlevel 1 (
  echo Build failed. See the errors above.
  pause
  exit /b 1
)
echo.
echo Release executable: "%~dp0target\release\RadioMaster.exe"
pause
endlocal
