@echo off
rem Publish Rust Maya (full commit history from Rust_Maya.bundle) to GitHub.
rem Needs Git for Windows; the first push opens a GitHub sign-in window.
cd /d "%~dp0"
where git >nul 2>nul || (echo Git is not installed: https://git-scm.com/download/win & pause & exit /b 1)
if exist .git (
  echo This folder is already a git repository - skipping the import.
) else (
  git init -b main || goto :fail
  git fetch Rust_Maya.bundle main || goto :fail
  git update-ref refs/heads/main FETCH_HEAD || goto :fail
  git reset -q || goto :fail
)
git remote get-url origin >nul 2>nul || git remote add origin https://github.com/rholmes69/Rust_Maya.git
echo.
echo Files that differ from the last commit (should be empty apart from this script and the bundle):
git status --short
echo.
git push -u origin main || goto :fail
echo.
echo Done: https://github.com/rholmes69/Rust_Maya
pause
exit /b 0
:fail
echo Something went wrong - see the message above.
pause
exit /b 1
