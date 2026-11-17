@echo off
rem A regra unica de qual hangar-server rodar mora no hangar-server-command.ps1 (o cmd nao compara datas).
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0hangar-server-command.ps1" engine %*
exit /b %errorlevel%
