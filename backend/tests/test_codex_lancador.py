"""O lancador unico da sessao Codex (scripts/hangar-codex-tui) contra um `codex` FALSO.

Mesmo padrao dos wrappers de shell (scripts/test-wrappers.sh): troca o binario de verdade por um
fake no PATH e confere o que ele recebeu. Aqui o fake precisa de duas caras, porque o lancador
chama o `codex` duas vezes com papeis diferentes -- `app-server --listen` (servidor WebSocket) e
`--remote` (a TUI). Nada de tmux: o lancador nao sabe o que e um pane.

O que estes testes protegem, e que nao da pra ver lendo o codigo:
- o app-server morre com o lancador (era orfao no desenho antigo, escutando em loopback);
- o sidecar sai com endpoint E pid, que e o que deixa o backend se ligar a um servidor que nao e
  filho dele.
"""
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

import pytest

from app.procinfo import pid_vivo

_LANCADOR = Path(__file__).resolve().parents[2] / "scripts" / "hangar-codex-tui"

# Fake `codex`: servidor no ramo `app-server`, TUI em qualquer outro. A TUI registra o proprio argv
# e fica viva o tempo de FAKE_TUI_SLEEP, pra o teste conseguir olhar o sidecar ENQUANTO a sessao
# existe -- ele e apagado na saida, que e justamente o outro comportamento sob teste.
_FAKE_CODEX = '''#!/usr/bin/env python3
import json, os, sys, time

args = sys.argv[1:]
if args[:1] == ["app-server"]:
    if os.environ.get("FAKE_SERVIDOR_MORRE"):
        print("error: unexpected argument '--listen' found", file=sys.stderr)
        sys.exit(3)
    from websockets.sync.server import serve
    porta = int(args[args.index("--listen") + 1].rsplit(":", 1)[1])

    def handler(ws):
        for cru in ws:
            msg = json.loads(cru)
            if msg.get("method") == "initialize":
                ws.send(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {}}))
                if os.environ.get("FAKE_SEM_THREAD"):
                    continue
                ws.send(json.dumps({"jsonrpc": "2.0", "method": "thread/started", "params": {
                    "thread": {"id": "thread-falso", "path": os.environ["FAKE_ROLLOUT"],
                               "cwd": os.environ["FAKE_CWD"]}}}))

    with serve(handler, "127.0.0.1", porta) as servidor:
        servidor.serve_forever()
else:
    with open(os.environ["FAKE_TUI_OUT"], "w") as fh:
        fh.write("\\n".join(args))
    time.sleep(float(os.environ.get("FAKE_TUI_SLEEP", "0.2")))
'''


def _ambiente(tmp_path, cwd):
    binario = tmp_path / "bin"
    binario.mkdir()
    fake = binario / "codex"
    fake.write_text(_FAKE_CODEX)
    fake.chmod(0o755)
    env = dict(os.environ)
    env.update({
        "PATH": f"{binario}{os.pathsep}{env['PATH']}",
        "HOME": str(tmp_path / "home"),        # o sidecar mora em ~/.hangar/codex-sessions
        "FAKE_ROLLOUT": str(tmp_path / "rollout.jsonl"),
        "FAKE_CWD": str(cwd),
        "FAKE_TUI_OUT": str(tmp_path / "tui-argv.txt"),
    })
    env.pop("CP_SESSION_NAME", None)
    (tmp_path / "home").mkdir()
    return env


def _sidecar(env, nome):
    return Path(env["HOME"]) / ".hangar" / "codex-sessions" / f"{nome}.json"


def _espera(cond, limite=15.0):
    fim = time.monotonic() + limite
    while time.monotonic() < fim:
        if cond():
            return True
        time.sleep(0.05)
    return False


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_grava_sidecar_completo_e_mata_o_servidor_na_saida(tmp_path):
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "6"
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert _espera(_sidecar(env, "sess").exists), "o sidecar nunca apareceu"
        meta = json.loads(_sidecar(env, "sess").read_text())
    finally:
        if proc.poll() is None:
            proc.wait(timeout=20)

    assert meta["provider"] == "codex"
    assert meta["thread_id"] == "thread-falso"
    assert meta["rollout_path"] == env["FAKE_ROLLOUT"]
    assert meta["cwd"] == str(cwd)
    # O par que existe por causa deste ticket: endereco E dono. Porta de loopback e reciclada, entao
    # o endpoint sozinho nao prova que o servidor do outro lado ainda e o desta sessao.
    assert meta["endpoint"].startswith("ws://127.0.0.1:")
    assert isinstance(meta["app_pid"], int)

    # A TUI e chamada com o MESMO endpoint do servidor, no cwd pedido, e sem alt-screen (o pane
    # precisa manter o scrollback).
    argv = (tmp_path / "tui-argv.txt").read_text().split("\n")
    assert argv[:2] == ["--remote", meta["endpoint"]]
    assert "--no-alt-screen" in argv
    assert argv[argv.index("-C") + 1] == str(cwd)

    # O que o ticket existe pra garantir: servidor morto junto com o lancador, e sidecar sem dono
    # nao fica para tras.
    assert _espera(lambda: not pid_vivo(meta["app_pid"])), "o app-server sobreviveu ao lancador"
    assert not _sidecar(env, "sess").exists()


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_preserva_sidecar_de_outro_dono(tmp_path):
    """Nome reusado: o sidecar que ficou no disco NAO e nosso, entao nao pode ser apagado.

    Apagar o de outro dono deixaria uma sessao VIVA invisivel pro app -- pior que o orfao que a
    limpeza existe pra evitar.
    """
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    # Sem thread/started este lancador nunca grava sidecar nenhum -- e a unica coisa em disco com
    # esse nome e a da outra sessao.
    env["FAKE_SEM_THREAD"] = "1"
    env["FAKE_TUI_SLEEP"] = "0.2"
    alheio = _sidecar(env, "sess")
    alheio.parent.mkdir(parents=True, exist_ok=True)
    alheio.write_text(json.dumps({"name": "sess", "provider": "codex", "thread_id": "outra",
                                  "rollout_path": "/x", "cwd": str(cwd),
                                  "endpoint": "ws://127.0.0.1:1", "app_pid": 999999}))

    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    proc.wait(timeout=30)
    assert json.loads(alheio.read_text())["thread_id"] == "outra"


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_sigterm_no_lancador_derruba_o_app_server(tmp_path):
    """O caminho que o pane usa de verdade: `tmux kill-session` derruba por SINAL, nao esperando o
    processo acabar. Sem o handler que vira SystemExit, o `finally` nunca roda e o servidor fica."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "60"
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert _espera(_sidecar(env, "sess").exists), "o sidecar nunca apareceu"
        pid = json.loads(_sidecar(env, "sess").read_text())["app_pid"]
        assert pid_vivo(pid)
        proc.send_signal(signal.SIGTERM)
        proc.wait(timeout=30)
    finally:
        if proc.poll() is None:
            proc.kill()
    assert _espera(lambda: not pid_vivo(pid)), "o app-server sobreviveu ao SIGTERM no lancador"
    assert not _sidecar(env, "sess").exists()


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_app_server_que_morre_na_largada_diz_o_motivo(tmp_path):
    """O stderr do app-server e a UNICA pista quando ele nao sobe (versao sem `--listen`, por
    exemplo). Jogado fora, a falha vira uma espera de 60s sem explicacao."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_SERVIDOR_MORRE"] = "1"
    env["FAKE_TUI_SLEEP"] = "0.2"
    r = subprocess.run([sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
                       env=env, capture_output=True, text=True, timeout=60)
    assert "codigo 3" in r.stderr
    assert "unexpected argument '--listen'" in r.stderr


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_retoma_a_conversa_pedida(tmp_path):
    """`--resume` troca o comando da TUI por `codex resume <id>`. Sem `-C`: a conversa carrega o cwd
    dela, e o pane ja nasce la."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "0.3"
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
         "--resume", "01a052d1-3e59-7441-9ed3-6bbd9e2704fc"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    proc.wait(timeout=30)
    argv = (tmp_path / "tui-argv.txt").read_text().split("\n")
    assert argv[0] == "resume"
    assert argv[-1] == "01a052d1-3e59-7441-9ed3-6bbd9e2704fc"
    assert "-C" not in argv
    # A politica de sandbox/aprovacao vale na conversa retomada tambem: sem ela a TUI pode parar
    # num pedido de aprovacao que ninguem responde, e o app fica olhando uma sessao muda.
    assert argv[argv.index("--sandbox") + 1] == "workspace-write"
    assert argv[argv.index("--ask-for-approval") + 1] == "never"


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_traduz_a_escolha_de_modelo(tmp_path):
    """Duas gramaticas: o modelo TEM flag (`-m`), o esforco NAO — ele e uma chave de configuracao,
    e o `-c` parseia o valor como TOML (dai as aspas). Mandar `--effort` pro `codex` mataria o
    processo no arranque com o pane ja criado."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "6"
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
         "--model", "gpt-5.6-luna", "--effort", "xhigh"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert _espera(_sidecar(env, "sess").exists), "o sidecar nunca apareceu"
        meta = json.loads(_sidecar(env, "sess").read_text())
    finally:
        if proc.poll() is None:
            proc.wait(timeout=20)
    argv = (tmp_path / "tui-argv.txt").read_text().split("\n")
    assert argv[argv.index("-m") + 1] == "gpt-5.6-luna"
    assert argv[argv.index("-c") + 1] == 'model_reasoning_effort="xhigh"'
    assert "--effort" not in argv
    # A escolha tambem vai pro SIDECAR: e de la que a pill do app le o modelo da sessao. Sem isto a
    # sessao nascia no modelo certo e a pill mostrava vazio (medido ao vivo em 30/08/2026) — o ramo
    # `_conectar` do sidecar com endpoint so conecta, entao o default da thread nunca e lido.
    assert (meta["model"], meta["effort"]) == ("gpt-5.6-luna", "xhigh")


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_sem_escolha_e_o_comando_de_hoje(tmp_path):
    """Ninguem pediu modelo: nem `-m` nem `-c` no comando, byte por byte como antes do ticket."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "0.3"
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    proc.wait(timeout=30)
    argv = (tmp_path / "tui-argv.txt").read_text().split("\n")
    assert "-m" not in argv and "-c" not in argv


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_recusa_sem_nome(tmp_path):
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    r = subprocess.run([sys.executable, str(_LANCADOR), "--cwd", str(cwd)],
                       env=env, capture_output=True, text=True, timeout=30)
    assert r.returncode == 2
    assert "CP_SESSION_NAME" in r.stderr
