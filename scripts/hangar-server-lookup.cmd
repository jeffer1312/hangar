@echo off
rem A mesma regra do scripts/hangar-server-lookup, em cmd puro, para os atalhos .cmd do Windows.
rem   call hangar-server-lookup.cmd <raiz da copia> [quem fala na mensagem]
rem Poe o executavel em HANGAR_SERVER_BIN e sai 0; sem ele, diz o porque no stderr e sai 126.
rem Sem blocos entre parenteses: caminho com ")" (Program Files (x86)) quebraria o bloco.
set "HANGAR_SERVER_BIN="
set "HSL_ROOT=%~1"
set "HSL_WHO=%~2"
if not defined HSL_WHO set "HSL_WHO=hangar"
if not defined CP_RUST_SERVER_BIN goto hsl_current
rem A variavel vence e nunca cai para outro: escolha explicita errada nao vira outro binario calado.
if exist "%CP_RUST_SERVER_BIN%\" goto hsl_bad_env
if exist "%CP_RUST_SERVER_BIN%" goto hsl_env
:hsl_bad_env
echo %HSL_WHO%: CP_RUST_SERVER_BIN nao e um executavel 1>&2
exit /b 126
:hsl_env
set "HANGAR_SERVER_BIN=%CP_RUST_SERVER_BIN%"
exit /b 0
:hsl_current
set "HSL_CURRENT=%USERPROFILE%\.hangar\current"
rem A troca de versao deixa so o current.old por um instante: espera o novo por 1 s.
if exist "%HSL_CURRENT%\" goto hsl_package
if exist "%HSL_CURRENT%.old\" ping -n 2 127.0.0.1 >nul
:hsl_package
if not exist "%HSL_CURRENT%\bin\hangar-server.exe" goto hsl_newest
set "HANGAR_SERVER_BIN=%HSL_CURRENT%\bin\hangar-server.exe"
exit /b 0
:hsl_newest
set "HSL_BUILD=%HSL_ROOT%\crates\target\release\hangar-server.exe"
set "HSL_HOME=%USERPROFILE%\.hangar\bin\hangar-server.exe"
if not exist "%HSL_BUILD%" goto hsl_home
if not exist "%HSL_HOME%" goto hsl_build
rem O mais novo vence e o empate fica com a copia: o xcopy /d /l so lista o baixado mais novo.
xcopy /d /l /y "%HSL_HOME%" "%HSL_BUILD%" 2>nul | find /i "hangar-server.exe" >nul && goto hsl_home
:hsl_build
set "HANGAR_SERVER_BIN=%HSL_BUILD%"
exit /b 0
:hsl_home
if not exist "%HSL_HOME%" goto hsl_none
set "HANGAR_SERVER_BIN=%HSL_HOME%"
exit /b 0
:hsl_none
echo %HSL_WHO%: hangar-server nao encontrado; instale o Hangar ou configure CP_RUST_SERVER_BIN 1>&2
exit /b 126
