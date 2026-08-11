"""Trava de `tmux kill-server` (hooks/guard_tmux.py) — o incidente de 11/08/2026.

Roda o hook como PROCESSO, do jeito que o Claude o dispara: JSON no stdin, decisao no exit code.
Testar so a funcao deixaria passar erro de contrato (exit 0 quando devia ser 2, stderr vazio).
"""
import json
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
    "echo 'avisa: tmux kill-server e proibido' | cp-send hangar",
    "sed -i 's/tmux kill-server/tmux -L probe kill-server/' script.sh",
])
def test_libera(cmd):
    assert roda(cmd).returncode == 0, f"bloqueou demais: {cmd!r}"


def test_outra_tool_passa():
    assert roda("tmux kill-server", tool="Read").returncode == 0


def test_stdin_lixo_nao_trava():
    r = subprocess.run([sys.executable, HOOK], input="nao e json", capture_output=True, text=True)
    assert r.returncode == 0
