@echo off
setlocal

rem Persist Ollama tuning for the current user (picked up by new processes).
setx OLLAMA_FLASH_ATTENTION 1 >nul
setx OLLAMA_KEEP_ALIVE -1 >nul
setx OLLAMA_NUM_PARALLEL 1 >nul
setx OLLAMA_CONTEXT_LENGTH 4096 >nul

rem setx does not touch this shell, so mirror the values for the child we launch.
set OLLAMA_FLASH_ATTENTION=1
set OLLAMA_KEEP_ALIVE=-1
set OLLAMA_NUM_PARALLEL=1
set OLLAMA_CONTEXT_LENGTH=4096

echo Stopping Ollama...
taskkill /F /IM "ollama app.exe" >nul 2>&1
taskkill /F /IM ollama.exe >nul 2>&1
ping -n 4 127.0.0.1 >nul

echo Starting Ollama...
start "" "%LOCALAPPDATA%\Programs\Ollama\ollama app.exe"

endlocal
echo Done.
