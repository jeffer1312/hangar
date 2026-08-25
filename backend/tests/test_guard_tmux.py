"""Trava de `tmux kill-server` (hooks/guard_tmux.py) — o incidente de 11/08/2026.

Roda o hook como PROCESSO, do jeito que o Claude o dispara: JSON no stdin, decisao no exit code.
Testar so a funcao deixaria passar erro de contrato (exit 0 quando devia ser 2, stderr vazio).
"""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

HOOK = str((Path(__file__).parent.parent / "hooks" / "guard_tmux.py").resolve())


def roda(comando: str, tool: str = "Bash") -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, HOOK],
        input=json.dumps({"tool_name": tool, "tool_input": {"command": comando}}),
        capture_output=True,
        text=True,
    )


@pytest.mark.parametrize("cmd", [
    "tmux kill-server",
    "tmux kill-server 2>/dev/null",
    # o comando real do subagente em 11/08/2026 13:55:29
    "which tmux && tmux -V\ntmux kill-server 2>/dev/null\ntmux new-session -d -s 'orig-test'",
    "/usr/bin/tmux kill-server",
    "tmux kill-session -a",
    "cd /tmp && tmux kill-server",
    "tmux kill-session",           # sem -t: mata a sessao de quem chamou
    "pkill tmux",
    "pkill -f 'tmux server'",
    "killall tmux",
    "sudo tmux kill-server",              # envelope antes do comando
    "CP_X=1 tmux kill-server",            # env na frente
    "echo oi; tmux kill-server",          # 2o segmento
    "echo $(tmux kill-server)",           # substituicao roda de verdade
    "(tmux kill-server)",                 # subshell
    # Bypasses achados na revisao pre-push de 11/08/2026 (o comando de verdade vem como
    # ARGUMENTO, entao a trava precisa reprocessar o que esta dentro das aspas):
    'eval "tmux kill-server"',
    'bash -c "tmux kill-server"',
    'sh -c "tmux kill-server"',
    'bash -lc "tmux kill-server"',
    'printf "kill-server\\n" | xargs tmux',   # argumento vem da entrada: opaco -> recusa
    "timeout 5 tmux kill-server",
    # `-f` do pkill e REGEX: os dois casam um tmux real sem conter as letras "tmux"
    "pkill -f 't*mux'",
    "pkill -f 'tm.x'",
])
def test_bloqueia(cmd):
    r = roda(cmd)
    # exit 2 + stderr = Claude Code e Kimi Code; JSON no stdout = adaptador do pi (que ignora
    # exit code). Os dois canais sao obrigatorios: sem um deles, um dos motores fica sem trava.
    assert r.returncode == 2, f"passou batido: {cmd!r}"
    assert "BLOQUEADO" in r.stderr
    saida = json.loads(r.stdout)["hookSpecificOutput"]
    assert saida["permissionDecision"] == "deny"
    assert "probe-x" in saida["permissionDecisionReason"]  # ensina o caminho certo


@pytest.mark.parametrize("cmd", [
    "tmux -L teste kill-server",          # socket proprio: exatamente o caminho certo
    "tmux -Lteste kill-server",
    "tmux -S /tmp/sock kill-server",
    "tmux kill-session -t '=orig-test:'",  # matar UMA sessao continua liberado
    "tmux -L probe-x kill-session",
    "pkill -f app.main",                   # reiniciar o backend (documentado no CLAUDE.md)
    "pkill -f 'node .*vite'",
    "tmux new-session -d -s x",
    "tmux ls",
    # FALAR do comando nunca e bloqueio (falso positivo achado pela propria trava, 11/08/2026:
    # a mensagem de commit que explicava o incidente foi recusada).
    "echo 'tmux kill-server e proibido'",
    'git commit -m "explica o incidente: tmux kill-server derrubou tudo"',
    "grep -rn kill-server backend/",
    "rtk proxy git add x && git commit -m 'bloqueia tmux kill-server'",
    "cat > /tmp/doc.md <<'EOF'\nNao rode tmux kill-server nunca.\nEOF",
    "echo 'avisa: tmux kill-server e proibido' | hangar-send hangar",
    "sed -i 's/tmux kill-server/tmux -L probe kill-server/' script.sh",
    'bash -c "npm run build"',   # interprete com comando inocente dentro
    'eval "$(fnm env)"',
    "xargs rm < lista.txt",      # xargs so recusa quando o alvo e tmux/pkill
    "pkill -f app.main",         # regex que NAO alcanca tmux
])
def test_libera(cmd):
    assert roda(cmd).returncode == 0, f"bloqueou demais: {cmd!r}"


def test_outra_tool_passa():
    assert roda("tmux kill-server", tool="Read").returncode == 0


def test_stdin_lixo_nao_trava():
    r = subprocess.run([sys.executable, HOOK], input="nao e json", capture_output=True, text=True)
    assert r.returncode == 0


def test_expansao_de_variavel_e_limitacao_conhecida():
    """`${CMD} kill-server` PASSA — resolver a variavel exigiria rodar um shell dentro do hook.
    Esta aqui pra ninguem "descobrir" isso de novo achando que e regressao: e escolha, e o
    docstring do hook diz. Se um dia virar bloqueio, este teste falha e alguem le o porque."""
    assert roda("CMD=tmux; ${CMD} kill-server").returncode == 0


def test_falha_deixa_rastro_em_disco(tmp_path):
    """Trava que libera por nao ter conseguido decidir tem que ANOTAR: falhar aberta e mudo
    esconde o furo pra sempre (ninguem descobre olhando pra tela que ela parou de valer)."""
    entrada = json.dumps({"tool_name": "Bash", "tool_input": {"command": {"nao": "e string"}}})
    r = subprocess.run([sys.executable, HOOK], input=entrada, capture_output=True, text=True,
                       env={**os.environ, "CLAUDE_CONFIG_DIR": str(tmp_path)})
    assert r.returncode == 0  # nunca trava a sessao
    log = (tmp_path / "guard_tmux-falhas.log").read_text()
    assert "nao e string" in log or "dict" in log
