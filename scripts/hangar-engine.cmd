@echo off
setlocal
if defined CP_RUST_SERVER_BIN if exist "%CP_RUST_SERVER_BIN%" goto explicit
if exist "%~dp0..\bin\hangar-server.exe" (
  "%~dp0..\bin\hangar-server.exe" engine %*
  exit /b
)
if exist "%~dp0..\crates\target\release\hangar-server.exe" (
  "%~dp0..\crates\target\release\hangar-server.exe" engine %*
  exit /b
)
if exist "%USERPROFILE%\.hangar\bin\hangar-server.exe" (
  "%USERPROFILE%\.hangar\bin\hangar-server.exe" engine %*
  exit /b
)
echo hangar-engine: hangar-server ausente; configure CP_RUST_SERVER_BIN 1>&2
exit /b 126
:explicit
"%CP_RUST_SERVER_BIN%" engine %*
exit /b %errorlevel%
