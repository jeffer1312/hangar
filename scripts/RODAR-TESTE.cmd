@echo off
REM ============================================================
REM  Prova de fogo do psmux — DE UM DUPLO CLIQUE NESTE ARQUIVO.
REM
REM  Se preferir copiar e colar num PowerShell, o comando e este:
REM
REM    powershell -ExecutionPolicy Bypass -File \\host.lan\Data\Projetos\claude-cockpit\scripts\test-psmux.ps1
REM
REM  O que acontece: instala psmux + Claude Code + Python (pulando o que ja
REM  existe), para pra voce logar no Claude, roda o teste e devolve o arquivo
REM  test-psmux-frames.txt para a raiz da share (= seu $HOME do Linux).
REM ============================================================
pushd "%~dp0"
powershell -ExecutionPolicy Bypass -File "%~dp0test-psmux.ps1"
popd
echo.
echo Terminou. Leia o resumo acima antes de fechar.
pause
