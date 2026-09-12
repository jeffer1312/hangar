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
import runpy
import signal
import subprocess
import sys
import time
import tempfile
from types import SimpleNamespace
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
    if os.environ.get("FAKE_SERVER_OUT"):
        with open(os.environ["FAKE_SERVER_OUT"], "w") as fh:
            json.dump(args, fh)
    if os.environ.get("FAKE_SERVIDOR_MORRE"):
        if os.environ.get("FAKE_ERRO_PRIVADO"):
            print(os.environ["FAKE_ERRO_PRIVADO"], file=sys.stderr)
        print("error: unexpected argument '--listen' found", file=sys.stderr)
        sys.exit(3)
    from websockets.sync.server import serve
    porta = int(args[args.index("--listen") + 1].rsplit(":", 1)[1])
    time.sleep(float(os.environ.get("FAKE_SERVIDOR_LENTO", "0")))   # maquina carregada

    def handler(ws):
        for cru in ws:
            msg = json.loads(cru)
            if msg.get("method") == "initialize":
                ws.send(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {}}))
                if os.environ.get("FAKE_SEM_THREAD"):
                    continue
                ws.send(json.dumps({"jsonrpc": "2.0", "method": "thread/started", "params": {
                    "thread": {"id": "thread-falso", "path": os.environ["FAKE_ROLLOUT"],
                               "cwd": os.environ["FAKE_CWD"], "source": "vscode", "threadSource": "user"}}}))
                for thread in json.loads(os.environ.get("FAKE_THREAD_SEQUENCE", "[]")):
                    time.sleep(0.1)
                    ws.send(json.dumps({"jsonrpc": "2.0", "method": "thread/started", "params": {"thread": thread}}))
                if os.environ.get("FAKE_EVENTS_DONE"):
                    open(os.environ["FAKE_EVENTS_DONE"], "w").close()

    with serve(handler, "127.0.0.1", porta) as servidor:
        servidor.serve_forever()
else:
    with open(os.environ["FAKE_TUI_OUT"], "w") as fh:
        fh.write("\\n".join(args))
    if os.environ.get("FAKE_TUI_ENV"):
        with open(os.environ["FAKE_TUI_ENV"], "w") as fh:
            fh.write(os.environ.get("CODEX_HOME", ""))
    if os.environ.get("FAKE_TUI_TOKEN"):
        with open(os.environ["FAKE_TUI_TOKEN"], "w") as fh:
            fh.write(os.environ.get("OPENAI_API_KEY", ""))
    if os.environ.get("FAKE_TUI_CONN"):
        # A TUI de verdade conecta UMA vez e morre no refused; aqui so registramos o que ela veria.
        import socket
        host, porta = args[args.index("--remote") + 1].rsplit("/", 1)[1].rsplit(":", 1)
        try:
            socket.create_connection((host, int(porta)), timeout=1).close()
            resultado = "conectou"
        except OSError:
            resultado = "recusou"
        with open(os.environ["FAKE_TUI_CONN"], "w") as fh:
            fh.write(resultado)
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
        "USERPROFILE": str(tmp_path / "home"),
        "LOCALAPPDATA": str(tmp_path / "local"),
        "CODEX_HOME": str(tmp_path / "home" / ".codex"),
        "CLAUDE_CONFIG_DIR": str(tmp_path / "home" / ".claude"),
        "FAKE_ROLLOUT": str(tmp_path / "rollout.jsonl"),
        "FAKE_CWD": str(cwd),
        "FAKE_TUI_OUT": str(tmp_path / "tui-argv.txt"),
        # O lançador lê o .env do checkout; a prova não pode reconciliar o backend real.
        "CP_PORT": "0",
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
def test_lancador_ressobe_o_servidor_na_mesma_porta_com_a_tui_viva(tmp_path):
    # A TUI `--remote` desiste de reconectar em ~1 min e nao volta nem com mensagem nova; o unico
    # caminho sem relançar o pane e o servidor voltar na MESMA porta, e o sidecar apontar pro
    # dono novo (o backend confere o pid antes de conectar).
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "12"
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert _espera(_sidecar(env, "sess").exists), "o sidecar nunca apareceu"
        meta = json.loads(_sidecar(env, "sess").read_text())
        os.kill(meta["app_pid"], 9)
        assert _espera(lambda: not pid_vivo(meta["app_pid"]))

        def ressubiu():
            novo = json.loads(_sidecar(env, "sess").read_text())
            return novo["app_pid"] != meta["app_pid"] and pid_vivo(novo["app_pid"])
        assert _espera(ressubiu, 10), "o app-server nao foi ressubido"
        novo = json.loads(_sidecar(env, "sess").read_text())
        assert novo["endpoint"] == meta["endpoint"]
        assert proc.poll() is None, "a TUI nao pode ser relançada"
    finally:
        if proc.poll() is None:
            proc.wait(timeout=30)
    assert _espera(lambda: not pid_vivo(novo["app_pid"])), "o servidor novo sobreviveu ao lancador"
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
def test_tui_so_sobe_depois_de_o_app_server_aceitar_conexao(tmp_path):
    """Sem esperar a porta, num servidor lento a TUI levava `Connection refused`, saia com 1 e o
    pane morria — foi o `[exited]` de 1s ao digitar `codex` com a maquina carregada."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_SERVIDOR_LENTO"] = "1.5"
    env["FAKE_TUI_CONN"] = str(tmp_path / "tui-conn.txt")
    r = subprocess.run([sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
                       env=env, capture_output=True, text=True, timeout=60)
    assert r.returncode == 0, r.stderr
    assert (tmp_path / "tui-conn.txt").read_text() == "conectou"
    assert "Traceback" not in r.stderr, r.stderr


def test_app_server_que_morre_na_largada_diz_o_motivo(tmp_path):
    """O stderr do app-server e a UNICA pista quando ele nao sobe (versao sem `--listen`, por
    exemplo). Jogado fora, a falha vira uma espera de 60s sem explicacao."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_SERVIDOR_MORRE"] = "1"
    env["FAKE_TUI_SLEEP"] = "0.2"
    env["FAKE_ERRO_PRIVADO"] = "x" * 3000 + "token=SEGREDO-DO-CLI"
    r = subprocess.run([sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
                       env=env, capture_output=True, text=True, timeout=60)
    assert "codigo 3" in r.stderr
    assert "unexpected argument '--listen'" in r.stderr
    logs = (Path(env["LOCALAPPDATA"]) / "hangar" / "logs" if os.name == "nt"
            else Path(env["HOME"]) / ".hangar" / "logs")
    privados = list((logs / "privado").glob("codex-app-server-*.log"))
    assert len(privados) == 1
    assert privados[0].stat().st_size <= 2000
    assert "SEGREDO-DO-CLI" in privados[0].read_text()
    if os.name == "posix":
        assert privados[0].stat().st_mode & 0o777 == 0o600
    diario = "".join(p.read_text() for p in (logs / "diario").glob("uso-*.jsonl"))
    eventos = [json.loads(linha) for linha in diario.splitlines()]
    evento = next(e for e in eventos if e["evento"] == "codex.app_server.falhou")
    assert (evento["sessao"], evento["provider"], evento["codigo"], evento["retorno"]) == (
        "sess", "codex", "processo_encerrou", 3)
    assert "SEGREDO-DO-CLI" not in diario
    assert "unexpected argument" not in diario
    assert "ws://" not in diario


def test_timeout_guarda_so_ultima_cauda_privada(tmp_path, monkeypatch):
    from app import diag, log_paths
    monkeypatch.setattr(log_paths, "base", lambda: tmp_path / "logs")
    monkeypatch.setenv("CP_SESSION_NAME", "sess-timeout")
    lancador = runpy.run_path(str(_LANCADOR))
    servidor = SimpleNamespace(poll=lambda: None)
    with tempfile.TemporaryFile() as erros:
        erros.write(b"token=SEGREDO-ANTIGO")
        assert not lancador["_esperar_porta"]("ws://127.0.0.1:1", servidor, erros, 0)
        erros.write(b"x" * 3000 + b"token=SEGREDO-NOVO")
        assert not lancador["_esperar_porta"]("ws://127.0.0.1:1", servidor, erros, 0)
    privados = list((log_paths.base() / "privado").glob("codex-app-server-*.log"))
    assert len(privados) == 1
    cauda = privados[0].read_bytes()
    assert len(cauda) == lancador["_ERRO_MAX"]
    assert b"SEGREDO-NOVO" in cauda and b"SEGREDO-ANTIGO" not in cauda
    diario = diag.caminho_do_dia().read_text()
    eventos = [json.loads(linha) for linha in diario.splitlines()]
    assert all(e["codigo"] == "timeout" for e in eventos)
    assert "SEGREDO" not in diag.ler_tudo()


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_retoma_a_conversa_pedida(tmp_path):
    """`--resume` troca o comando da TUI por `codex resume <id>`. Sem `-C`: a conversa carrega o cwd
    dela, e o pane ja nasce la."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "0.3"
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
         "--resume", "01a052d1-3e59-7441-9ed3-6bbd9e2704fc"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    proc.wait(timeout=30)
    assert proc.returncode == 0
    argv = (tmp_path / "tui-argv.txt").read_text().split("\n")
    assert argv[0] == "resume"
    assert argv[-1] == "01a052d1-3e59-7441-9ed3-6bbd9e2704fc"
    assert "-C" not in argv
    # O resume remoto recusa overrides na TUI; as politicas pertencem ao app-server.
    assert "--remote" in argv
    assert "--sandbox" not in argv
    assert "--ask-for-approval" not in argv
    server_argv = json.loads((tmp_path / "server-argv.json").read_text())
    configs = [server_argv[i + 1] for i, arg in enumerate(server_argv[:-1]) if arg == "-c"]
    assert 'sandbox_mode="danger-full-access"' in configs
    assert 'approval_policy="never"' in configs


@pytest.mark.parametrize("resume", [False, True])
def test_lancador_acompanha_nova_principal_sem_tomar_subagente(tmp_path, resume):
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "4"
    env["FAKE_EVENTS_DONE"] = str(tmp_path / "events-done")
    main = {"id": "nova", "path": str(tmp_path / "nova.jsonl"), "cwd": str(cwd),
            "source": "vscode", "threadSource": "user", "parentThreadId": None}
    sub = {**main, "id": "sub", "source": {"subAgent": {}}, "threadSource": "subagent", "parentThreadId": "nova"}
    env["FAKE_THREAD_SEQUENCE"] = json.dumps([sub, main, sub, {**main, "id": "aux", "canAcceptDirectInput": False}])
    args = [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd), "--model", "gpt-test", "--effort", "high"]
    if resume:
        args += ["--resume", "anterior"]
        rollout = Path(env["HOME"]) / ".codex/sessions/2026/09/09/rollout-anterior.jsonl"
        rollout.parent.mkdir(parents=True)
        rollout.touch()
        previous = _sidecar(env, "sess")
        previous.parent.mkdir(parents=True)
        previous.write_text(json.dumps({"name": "sess", "thread_id": "anterior", "rollout_path": str(rollout),
                                       "cwd": str(cwd), "app_pid": 999999, "endpoint": "ws://127.0.0.1:1"}))
    proc = subprocess.Popen(args, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        assert _espera(lambda: Path(env["FAKE_EVENTS_DONE"]).exists())
        assert _espera(lambda: json.loads(_sidecar(env, "sess").read_text())["thread_id"] == "nova")
        meta = json.loads(_sidecar(env, "sess").read_text())
        assert meta["rollout_path"] == main["path"]
        assert (meta["model"], meta["effort"]) == ("gpt-test", "high")
        assert pid_vivo(meta["app_pid"]) and meta["endpoint"].startswith("ws://")
    finally:
        proc.wait(timeout=15)
    assert not _sidecar(env, "sess").exists()


def test_o_sandbox_nao_pode_voltar_a_prender_a_rede():
    """Sessao Codex roda SEM sandbox, como Claude, Pi e Kimi — e nao e so simetria.

    `workspace-write` corta a REDE, loopback incluido (medido em 30/08/2026): dentro dele o
    `hangar-send` de uma sessao Codex morria em "backend inacessivel em 127.0.0.1:8765" com o
    backend no ar, e o Codex era o unico agente incapaz de falar com as sessoes irmas. Quem trocar
    este valor de volta reintroduz isso, e o sintoma nao parece sandbox nenhum."""
    from app.adapters.codex.lancador import APPROVAL, SANDBOX
    assert SANDBOX == "danger-full-access"
    assert APPROVAL == "never"


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


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_aplica_codex_home_antes_da_tui(tmp_path):
    cwd = tmp_path / "proj"
    cwd.mkdir()
    codex_home = tmp_path / "codex-work"
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "0.3"
    env["FAKE_TUI_ENV"] = str(tmp_path / "tui-env.txt")
    proc = subprocess.run(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
         "--codex-home", str(codex_home)],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
    )
    assert proc.returncode == 0, proc.stderr
    assert Path(env["FAKE_TUI_ENV"]).read_text() == str(codex_home.absolute())
    meta = json.loads(_sidecar(env, "sess").read_text()) if _sidecar(env, "sess").exists() else None
    assert meta is None  # o lancador limpa o sidecar ao sair; o arquivo foi conferido durante a TUI


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_secundario_remove_token_openai_herdado(tmp_path):
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "0.3"
    env["FAKE_TUI_TOKEN"] = str(tmp_path / "tui-token.txt")
    env["OPENAI_API_KEY"] = "sentinel"
    r = subprocess.run(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
         "--codex-home", str(tmp_path / "codex-work"), "--codex-account", "work"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
    )
    assert r.returncode == 0, r.stderr
    assert Path(env["FAKE_TUI_TOKEN"]).read_text() == ""


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_nao_reclassifica_conta_pelo_codex_home_herdado(tmp_path):
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["CODEX_HOME"] = str(tmp_path / "stale-tmux-default")
    env["OPENAI_API_KEY"] = "sentinel"
    env["FAKE_TUI_TOKEN"] = str(tmp_path / "tui-token.txt")
    r = subprocess.run(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
         "--codex-home", str(tmp_path / "codex-work"), "--codex-account", "work"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
    )
    assert r.returncode == 0, r.stderr
    assert Path(env["FAKE_TUI_TOKEN"]).read_text() == ""
