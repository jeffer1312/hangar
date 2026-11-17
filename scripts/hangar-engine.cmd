@echo off
setlocal
rem A regra de qual hangar-server rodar mora no hangar-server-lookup.cmd, em cmd puro.
call "%~dp0hangar-server-lookup.cmd" "%~dp0.." hangar-engine || exit /b 126
"%HANGAR_SERVER_BIN%" engine %*
exit /b %ERRORLEVEL%
