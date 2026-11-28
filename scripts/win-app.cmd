@echo off
rem Build Iskra for 64-bit Windows, as the zip that is handed out: see win-app.ps1.
rem Run it from cmd, from anywhere:  scripts\win-app.cmd
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0win-app.ps1" %*
exit /b %ERRORLEVEL%
