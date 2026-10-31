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
import contextlib
import http.server
import json
import os
import threading
import runpy
import signal
import subprocess
import sys
import time
import tempfile
import tomllib
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
    if os.environ.get("FAKE_SERVER_ENV"):
        with open(os.environ["FAKE_SERVER_ENV"], "w") as fh:
            json.dump({"cwd": os.getcwd(), "identity": {key: os.environ.get(key) for key in (
                "CODEX_HOME", "CP_SESSION_NAME", "CP_SESSION_KEY", "TMUX", "TMUX_PANE", "HANGAR_CANO_KEY")},
                "has_openai_key": "OPENAI_API_KEY" in os.environ}, fh)
    if os.environ.get("FAKE_SERVER_OUT"):
        with open(os.environ["FAKE_SERVER_OUT"], "w") as fh:
            json.dump(args, fh)
    if os.environ.get("FAKE_SERVER_INSTRUCTION_LIMIT"):
        import tomllib
        from pathlib import Path
        config = Path(os.environ["CODEX_HOME"]) / "config.toml"
        limit = tomllib.loads(config.read_text()).get("project_doc_max_bytes", 32768)
        for index, arg in enumerate(args[:-1]):
            if arg in ("-c", "--config") and args[index + 1].startswith("project_doc_max_bytes="):
                limit = tomllib.loads(args[index + 1])["project_doc_max_bytes"]
        Path(os.environ["FAKE_SERVER_INSTRUCTION_LIMIT"]).write_text(str(limit))
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
            elif "id" in msg:
                ws.send(json.dumps({"jsonrpc": "2.0", "id": msg["id"], "result": {"data": []}}))

    with serve(handler, "127.0.0.1", porta) as servidor:
        servidor.serve_forever()
else:
    if os.environ.get("FAKE_TUI_PID"):
        with open(os.environ["FAKE_TUI_PID"], "w") as fh:
            fh.write(str(os.getpid()))
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
    env.pop("CP_SESSION_KEY", None)
    (tmp_path / "home").mkdir()
    return env


@contextlib.contextmanager
def _backend_conta_pronta(env, status="ready", issues=None):
    """Conta secundária só abre depois do preparo no backend; aqui ele responde pronto na hora."""
    class _Pronto(http.server.BaseHTTPRequestHandler):
        def _responder(self):
            corpo = json.dumps({"status": status, "issues": issues or []}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(corpo)))
            self.end_headers()
            self.wfile.write(corpo)

        do_GET = do_POST = _responder

        def log_message(self, *_args):
            pass

    servidor = http.server.ThreadingHTTPServer(("127.0.0.1", 0), _Pronto)
    thread = threading.Thread(target=servidor.serve_forever, daemon=True)
    thread.start()
    env["CP_PORT"] = str(servidor.server_address[1])
    env["CP_AUTH_TOKEN"] = "teste"
    try:
        yield
    finally:
        servidor.shutdown()
        servidor.server_close()
        thread.join(timeout=5)
        assert not thread.is_alive()


def _sidecar(env, nome):
    return Path(env["HOME"]) / ".hangar" / "codex-sessions" / f"{nome}.json"


def _espera(cond, limite=15.0):
    fim = time.monotonic() + limite
    while time.monotonic() < fim:
        if cond():
            return True
        time.sleep(0.05)
    return False


@pytest.mark.parametrize("tier", [None, "priority", "default"])
def test_launcher_command_transports_only_explicit_service_tier(tier):
    from app.adapters.codex.lancador import comando_do_lancador
    argv = comando_do_lancador("/tmp/proj", initial_prompt="primeiro prompt", service_tier=tier)
    if tier is None:
        assert "--service-tier" not in argv
    else:
        assert argv[argv.index("--service-tier") + 1] == tier
        assert argv.index("--service-tier") < argv.index("--prompt")
    with pytest.raises(ValueError, match="service_tier"):
        comando_do_lancador("/tmp/proj", service_tier='priority"; rm -rf /')


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
@pytest.mark.parametrize("tier", [None, "priority", "default"])
def test_launcher_applies_creation_tier_to_server_and_tui_before_prompt(tmp_path, tier):
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "3"
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    args = [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd), "--prompt", "primeiro prompt"]
    if tier is not None:
        args += ["--service-tier", tier]
    proc = subprocess.Popen(args, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        assert _espera(lambda: _sidecar(env, "sess").exists() and Path(env["FAKE_TUI_OUT"]).exists())
        meta = json.loads(_sidecar(env, "sess").read_text())
        server_args = json.loads(Path(env["FAKE_SERVER_OUT"]).read_text())
        tui_args = Path(env["FAKE_TUI_OUT"]).read_text().splitlines()
        assert meta.get("service_tier") == tier
        assert tui_args[-1] == "primeiro prompt"
        for argv in (server_args, tui_args):
            if tier is None:
                assert not any("service_tier=" in arg for arg in argv)
            else:
                assert f'service_tier="{tier}"' in argv
    finally:
        proc.wait(timeout=20)


def test_conta_secundaria_aguarda_primeiro_preparo(monkeypatch):
    lancador = runpy.run_path(str(_LANCADOR))
    chamadas = []
    states = iter(["idle","running","running","ready"])

    def api(method, path):
        chamadas.append((method, path))
        return {"status": next(states), "etapa": "plugins", "issues": []}

    monkeypatch.setitem(lancador["_preparar_conta_codex"].__globals__, "_api_backend", api)
    monkeypatch.setattr(time,"sleep",lambda _:None)

    result = lancador["_preparar_conta_codex"]("work", "/repo com espaço")

    path = "/api/codex-contas/work/prepare?cwd=%2Frepo%20com%20espa%C3%A7o"
    assert chamadas == [("GET",path),("POST",path),("GET",path),("GET",path)]
    assert result["status"] == "ready"


def test_conta_ja_preparada_ainda_confirma_trust_da_pasta(monkeypatch):
    lancador = runpy.run_path(str(_LANCADOR))
    chamadas = []

    def api(method, path):
        chamadas.append(method)
        return {"status": "ready", "trust_pending": False, "issues": []}

    monkeypatch.setitem(lancador["_preparar_conta_codex"].__globals__, "_api_backend", api)

    assert lancador["_preparar_conta_codex"]("work", "/repo")["status"] == "ready"
    assert chamadas == ["GET"]


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_conta_preparada_abre_com_backend_desligado(tmp_path):
    cwd = tmp_path/"project"
    cwd.mkdir()
    env = _ambiente(tmp_path,cwd)
    env["FAKE_SERVER_OUT"] = str(tmp_path/"server-started.json")
    home = tmp_path/"profile"
    home.mkdir()
    (home/".hangar-prepared-v1").write_text('{"version":1}')
    control = Path(env["HOME"])/".hangar/codex-atualizacao.json"
    control.parent.mkdir()
    control.write_text(json.dumps({"proxima_em":time.time()+3600}))
    process = subprocess.Popen([sys.executable,str(_LANCADOR),"--name","prep-offline","--cwd",str(cwd),
        "--codex-home",str(home),"--codex-account","work"],env=env,
        stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
    try:
        stdout,stderr = process.communicate(timeout=25)
    finally:
        if process.poll() is None:
            process.terminate()
            try:process.wait(timeout=5)
            except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
    assert process.returncode == 0,stdout+stderr
    assert Path(env["FAKE_SERVER_OUT"]).exists()
    assert Path(env["FAKE_TUI_OUT"]).exists()
    import tomllib
    assert tomllib.loads((home/"config.toml").read_text())["projects"][str(cwd)]["trust_level"] == "trusted"


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_preparo_incompleto_nao_sobe_appserver_nem_tui(tmp_path):
    cwd = tmp_path/"project"
    cwd.mkdir()
    env = _ambiente(tmp_path,cwd)
    env["FAKE_SERVER_OUT"] = str(tmp_path/"server-started.json")
    control = Path(env["HOME"])/".hangar/codex-atualizacao.json"
    control.parent.mkdir()
    control.write_text(json.dumps({"proxima_em":time.time()+3600}))
    with _backend_conta_pronta(env,status="partial",issues=[
        {"code":"codex_account_plugin_hooks_failed","params":{"error":"private detail"}}]):
        process = subprocess.Popen([sys.executable,str(_LANCADOR),"--name","prep-test","--cwd",str(cwd),
            "--codex-home",str(tmp_path/"profile"),"--codex-account","work"],env=env,
            stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        try:
            stdout,stderr = process.communicate(timeout=25)
        finally:
            if process.poll() is None:
                process.terminate()
                try:process.wait(timeout=5)
                except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)
    assert process.returncode != 0,stdout+stderr
    assert "codex_account_plugin_hooks_failed" in stderr
    assert "private detail" not in stderr
    assert not Path(env["FAKE_SERVER_OUT"]).exists()
    assert not Path(env["FAKE_TUI_OUT"]).exists()


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
    # Quem esta sessao criar herda o modo dela; sem o campo, a nova caia no padrao da conta.
    assert meta["permission_mode"] == "Full Access"
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
def test_sessao_nova_grava_a_chave_do_spec_que_o_pane_recebeu(tmp_path):
    # O Rust cria o pane com a chave do spec: sem ela no sidecar a adoção sorteia outra e o registro
    # da sessão (indexado pela chave) fica sem dono.
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "3"
    env["CP_SESSION_KEY"] = "a" * 32
    proc = subprocess.Popen([sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        assert _espera(_sidecar(env, "sess").exists), "o sidecar nunca apareceu"
        meta = json.loads(_sidecar(env, "sess").read_text())
    finally:
        proc.wait(timeout=20)
    assert meta.get("key") == "a" * 32


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_encerrar_a_sessao_nao_conta_como_queda_do_app_server(tmp_path):
    # S-24: fechar o terminal derruba app-server e TUI quase juntos; o servidor saindo primeiro não é
    # queda (nem aviso, nem ressubida).
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "20"
    tui_pid_file = tmp_path / "tui.pid"
    env["FAKE_TUI_PID"] = str(tui_pid_file)
    proc = subprocess.Popen([sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd)],
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        assert _espera(lambda: tui_pid_file.exists() and tui_pid_file.read_text().strip()), "a TUI não subiu"
        assert _espera(_sidecar(env, "sess").exists), "o sidecar nunca apareceu"
        meta = json.loads(_sidecar(env, "sess").read_text())
        os.kill(meta["app_pid"], signal.SIGTERM)
        time.sleep(0.7)
        os.kill(int(tui_pid_file.read_text()), signal.SIGTERM)
        _, stderr = proc.communicate(timeout=20)
    finally:
        if proc.poll() is None:
            proc.kill()
    assert "o app-server saiu com codigo" not in stderr
    assert "ressubido" not in stderr


@pytest.mark.skipif(os.name != "posix", reason="o lancador so e usado em pane POSIX por ora")
def test_lancador_ressobe_o_servidor_na_mesma_porta_com_a_tui_viva(tmp_path, monkeypatch):
    # A TUI `--remote` desiste de reconectar em ~1 min e nao volta nem com mensagem nova; o unico
    # caminho sem relançar o pane e o servidor voltar na MESMA porta, e o sidecar apontar pro
    # dono novo (o backend confere o pid antes de conectar).
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "12"
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    tui_pid_file = tmp_path / "tui.pid"
    env["FAKE_TUI_PID"] = str(tui_pid_file)
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
         "--tool-output-token-limit", "144000", "--service-tier", "priority"],
        env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    try:
        assert _espera(_sidecar(env, "sess").exists), "o sidecar nunca apareceu"
        def tui_alive():
            if not tui_pid_file.exists():
                return False
            pid = tui_pid_file.read_text().strip()
            return bool(pid) and pid_vivo(int(pid))

        assert _espera(tui_alive), "a TUI não ficou viva antes de trocar Fast"
        meta = json.loads(_sidecar(env, "sess").read_text())
        assert meta["tool_output_token_limit"] == 144000
        assert "tool_output_token_limit=144000" in json.loads((tmp_path / "server-argv.json").read_text())
        assert meta["service_tier"] == "priority"
        assert 'service_tier="priority"' in json.loads((tmp_path / "server-argv.json").read_text())
        # O usuário desligou Fast depois da abertura; reiniciar não pode repetir a escolha inicial.
        from app.adapters.codex import sessions
        monkeypatch.setattr(sessions, "_dir", lambda: _sidecar(env, "sess").parent)
        # A troca usa a trava do produto, compartilhada com as escritas do lançador.
        assert sessions.update_service_tier("sess", meta["thread_id"], "default")
        os.kill(meta["app_pid"], 9)
        assert _espera(lambda: not pid_vivo(meta["app_pid"]))

        def ressubiu():
            novo = json.loads(_sidecar(env, "sess").read_text())
            return novo["app_pid"] != meta["app_pid"] and pid_vivo(novo["app_pid"])
        assert _espera(ressubiu, 10), "o app-server nao foi ressubido"
        novo = json.loads(_sidecar(env, "sess").read_text())
        assert novo["endpoint"] == meta["endpoint"]
        assert novo["tool_output_token_limit"] == 144000
        assert novo["service_tier"] == "default"
        restarted_args = json.loads((tmp_path / "server-argv.json").read_text())
        assert "tool_output_token_limit=144000" in restarted_args
        assert 'service_tier="default"' in restarted_args
        assert 'service_tier="priority"' not in restarted_args
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
@pytest.mark.parametrize("restricted", [False, True])
def test_lancador_retoma_a_conversa_pedida(tmp_path, restricted):
    """`--resume` troca o comando da TUI por `codex resume <id>`. Sem `-C`: a conversa carrega o cwd
    dela, e o pane ja nasce la."""
    cwd = tmp_path / "proj"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "0.3"
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    proc = subprocess.Popen(
        [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
         "--resume", "01a052d1-3e59-7441-9ed3-6bbd9e2704fc",
         *(["--approval-policy", "on-request", "--sandbox", "read-only"] if restricted else [])],
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
    sandbox, approval = ("read-only", "on-request") if restricted else ("danger-full-access", "never")
    assert f'sandbox_mode="{sandbox}"' in configs
    assert f'approval_policy="{approval}"' in configs


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
    assert 'model_reasoning_effort="xhigh"' in argv
    # A checagem de atualização sai sempre: o aviso dela trava a TUI antes da thread.
    assert "check_for_update_on_startup=false" in argv
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
    assert "-m" not in argv
    assert [argv[i + 1] for i, x in enumerate(argv) if x == "-c"] == ["check_for_update_on_startup=false"]


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
    with _backend_conta_pronta(env):
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
    with _backend_conta_pronta(env):
        r = subprocess.run(
            [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd),
             "--codex-home", str(tmp_path / "codex-work"), "--codex-account", "work"],
            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, timeout=30,
        )
    assert r.returncode == 0, r.stderr
    assert Path(env["FAKE_TUI_TOKEN"]).read_text() == ""


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
@pytest.mark.parametrize("explicit_overrides", [False, True])
@pytest.mark.parametrize("service_tier", ["priority", "default"])
def test_launcher_resume_uses_imported_account_policy_identity_and_explicit_overrides(tmp_path, explicit_overrides, service_tier):
    cwd = tmp_path / "project"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "3"
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    env["FAKE_SERVER_ENV"] = str(tmp_path / "server-env.json")
    env["FAKE_SEM_THREAD"] = "1"
    env.update(CP_SESSION_NAME="operator", CP_SESSION_KEY="operator-key", TMUX="operator-tmux",
               TMUX_PANE="%operator", HANGAR_CANO_KEY="operator-cano", OPENAI_API_KEY="operator-token")
    thread_id = "01a052d1-3e59-7441-9ed3-6bbd9e2704fc"
    account_home = tmp_path / "codex-work"
    explicit_home = tmp_path / "codex-alternate"
    for home in (account_home, explicit_home):
        rollout = home / "sessions" / "2026" / "10" / "03" / f"rollout-example-{thread_id}.jsonl"
        rollout.parent.mkdir(parents=True)
        rollout.write_text("{}\n")
    sidecar = _sidecar(env, "sess")
    sidecar.parent.mkdir(parents=True, exist_ok=True)
    meta = {"name": "sess", "thread_id": thread_id, "rollout_path": env["FAKE_ROLLOUT"],
            "cwd": str(cwd), "codex_home": str(account_home), "key": "same-key",
            "codex_account": "work", "transfer_id": "import", "tool_output_token_limit": 144000,
            "permission_mode": "Ask for approval", "jev": False, "model": "native-model", "effort": "high",
            "service_tier": service_tier}
    sidecar.write_text(json.dumps(meta))
    overrides = (["--codex-home", str(explicit_home), "--codex-account", "alternate",
                  "--approval-policy", "never", "--sandbox", "danger-full-access",
                  "--model", "override-model", "--effort", "low", "--tool-output-token-limit", "288000"]
                 if explicit_overrides else [])
    proc = subprocess.Popen([sys.executable, str(_LANCADOR), "--name", "sess",
                             "--resume", thread_id, *overrides], env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True)
    try:
        assert _espera(lambda: sidecar.exists() and json.loads(sidecar.read_text()).get("tui_pid")
                       and Path(env["FAKE_SERVER_ENV"]).exists() and Path(env["FAKE_TUI_OUT"]).exists())
        saved = json.loads(sidecar.read_text())
        budget = 288000 if explicit_overrides else 144000
        expected_home = str(explicit_home if explicit_overrides else account_home)
        assert saved["tool_output_token_limit"] == budget and saved["transfer_id"] == "import"
        assert saved["model"] == ("override-model" if explicit_overrides else "native-model")
        assert saved["effort"] == ("low" if explicit_overrides else "high")
        assert saved["codex_home"] == expected_home
        assert saved["codex_account"] == ("alternate" if explicit_overrides else "work")
        assert saved["permission_mode"] == ("Full Access" if explicit_overrides else "Ask for approval")
        assert saved["key"] == "same-key" and saved["thread_id"] == thread_id
        server_args = json.loads((tmp_path / "server-argv.json").read_text())
        assert f"tool_output_token_limit={budget}" in server_args
        assert saved["service_tier"] == service_tier
        assert f'service_tier="{service_tier}"' in server_args
        assert f'service_tier="{service_tier}"' in Path(env["FAKE_TUI_OUT"]).read_text().splitlines()
        assert ('approval_policy="never"' if explicit_overrides else 'approval_policy="on-request"') in server_args
        assert ('sandbox_mode="danger-full-access"' if explicit_overrides else 'sandbox_mode="read-only"') in server_args
        assert not any("project_doc_max_bytes" in arg for arg in server_args)
        process_environment = json.loads(Path(env["FAKE_SERVER_ENV"]).read_text())
        assert process_environment == {"cwd": str(cwd), "has_openai_key": False, "identity": {
            "CODEX_HOME": expected_home, "CP_SESSION_NAME": "sess", "CP_SESSION_KEY": "same-key",
            "TMUX": None, "TMUX_PANE": None, "HANGAR_CANO_KEY": None}}
    finally:
        proc.wait(timeout=20)


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
@pytest.mark.parametrize("permission_mode", ["Ask for approval", None])
def test_relaunch_of_own_session_completes_from_the_sidecar_and_refuses_unknown_mode(tmp_path, permission_mode):
    # A conta pronta já traz o limite publicado pelo preparo nativo, inclusive sem servidor.
    cwd = tmp_path / "project"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "3"
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    env["FAKE_SERVER_INSTRUCTION_LIMIT"] = str(tmp_path / "instruction-limit.txt")
    env["FAKE_SEM_THREAD"] = "1"
    env["CP_SESSION_KEY"] = "same-key"
    thread_id = "01a052d1-3e59-7441-9ed3-6bbd9e2704fc"
    home = tmp_path / "codex-work"
    rollout = home / "sessions" / "2026" / "10" / "03" / f"rollout-example-{thread_id}.jsonl"
    rollout.parent.mkdir(parents=True)
    rollout.write_text("{}\n")
    (home/".hangar-prepared-v1").write_text('{"version":1}')
    (home/"config.toml").write_text("project_doc_max_bytes = 1048576\n", encoding="utf-8")
    sidecar = _sidecar(env, "sess")
    sidecar.parent.mkdir(parents=True, exist_ok=True)
    meta = {"name": "sess", "thread_id": thread_id, "rollout_path": env["FAKE_ROLLOUT"], "cwd": str(cwd),
            "codex_home": str(home), "codex_account": "work", "key": "same-key", "headless": False,
            "tool_output_token_limit": 144000, "model": "native-model", "effort": "high",
            "service_tier": "priority", "permission_mode": permission_mode}
    sidecar.write_text(json.dumps(meta))
    argv = [sys.executable, str(_LANCADOR), "--name", "sess", "--cwd", str(cwd), "--resume", thread_id]
    if permission_mode is None:
        result = subprocess.run(argv, env=env, capture_output=True, text=True, timeout=15)
        assert result.returncode != 0 and "modo de permissão da sessão desconhecido" in result.stderr
        assert not Path(env["FAKE_SERVER_OUT"]).exists(), "nunca cai no Full Access do padrão"
        return
    proc = subprocess.Popen(argv, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        assert _espera(lambda: sidecar.exists() and json.loads(sidecar.read_text()).get("tui_pid"))
        saved = json.loads(sidecar.read_text())
        assert (saved["model"], saved["effort"], saved["service_tier"]) == ("native-model", "high", "priority")
        assert (saved["codex_home"], saved["codex_account"]) == (str(home), "work")
        server_args = json.loads((tmp_path / "server-argv.json").read_text())
        assert "tool_output_token_limit=144000" in server_args
        assert 'approval_policy="on-request"' in server_args and 'sandbox_mode="read-only"' in server_args
        assert Path(env["FAKE_SERVER_INSTRUCTION_LIMIT"]).read_text() == "1048576"
        assert tomllib.loads((home/"config.toml").read_text(encoding="utf-8")) == {
            "project_doc_max_bytes": 1048576, "projects": {str(cwd): {"trust_level": "trusted"}}
        }
    finally:
        proc.wait(timeout=20)


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
@pytest.mark.parametrize("field,value", [("key", None), ("permission_mode", "unknown"), ("codex_home", "relative")])
def test_imported_resume_rejects_invalid_metadata_before_opening_server(tmp_path, field, value):
    cwd = tmp_path / "project"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    sidecar = _sidecar(env, "sess")
    sidecar.parent.mkdir(parents=True)
    meta = {"name": "sess", "thread_id": "thread", "transfer_id": "import", "key": "key",
            "cwd": str(cwd), "codex_home": str(tmp_path / "secondary"), "codex_account": "work",
            "permission_mode": "Ask for approval", "tool_output_token_limit": 144000}
    meta[field] = value
    sidecar.write_text(json.dumps(meta))
    result = subprocess.run([sys.executable, str(_LANCADOR), "--name", "sess", "--resume", "thread"],
                            env=env, capture_output=True, text=True, timeout=15)
    assert result.returncode != 0
    assert "metadados da conversa importada inválidos" in result.stderr
    assert not Path(env["FAKE_SERVER_OUT"]).exists()
    assert json.loads(sidecar.read_text()) == meta


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
@pytest.mark.parametrize("valid_override", [False, True])
def test_imported_partial_policy_override_is_validated_before_spawn_and_persisted(tmp_path, valid_override):
    cwd = tmp_path / "project"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "3"
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    env["FAKE_SEM_THREAD"] = "1"
    home = tmp_path / "secondary"
    thread_id = "same-thread"
    rollout = home / "sessions" / "2026" / "10" / "03" / f"rollout-example-{thread_id}.jsonl"
    rollout.parent.mkdir(parents=True)
    rollout.write_text("{}\n")
    sidecar = _sidecar(env, "sess")
    sidecar.parent.mkdir(parents=True)
    meta = {"name": "sess", "thread_id": thread_id, "transfer_id": "import", "key": "durable-key",
            "cwd": str(cwd), "codex_home": str(home), "codex_account": "work",
            "rollout_path": str(rollout), "permission_mode": "Ask for approval",
            "tool_output_token_limit": 144000, "model": "native-model", "effort": "high"}
    sidecar.write_text(json.dumps(meta))
    original = sidecar.read_bytes()
    argv = [sys.executable, str(_LANCADOR), "--name", "sess", "--resume", thread_id]
    if not valid_override:
        rejected = subprocess.run([*argv, "--approval-policy", "never"], env=env,
                                  capture_output=True, text=True, timeout=15)
        assert rejected.returncode != 0
        assert "combinação de aprovação e sandbox não suportada" in rejected.stderr
        assert not Path(env["FAKE_SERVER_OUT"]).exists()
        assert not Path(env["FAKE_TUI_OUT"]).exists()
        assert sidecar.read_bytes() == original
        # A tentativa seguinte usa o registro original, sem qualquer reparo pelo teste.
    overrides = ["--sandbox", "workspace-write"] if valid_override else []
    proc = subprocess.Popen([*argv, *overrides], env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True)
    try:
        assert _espera(lambda: sidecar.exists() and json.loads(sidecar.read_text()).get("tui_pid"))
        saved = json.loads(sidecar.read_text())
        assert saved["permission_mode"] == ("Approve for me" if valid_override else "Ask for approval")
        assert saved["key"] == meta["key"] and saved["codex_account"] == "work"
        assert saved["thread_id"] == thread_id and saved["tool_output_token_limit"] == 144000
        server_args = json.loads(Path(env["FAKE_SERVER_OUT"]).read_text())
        assert 'approval_policy="on-request"' in server_args
        assert ('sandbox_mode="workspace-write"' if valid_override else 'sandbox_mode="read-only"') in server_args
        assert 'sandbox_mode="danger-full-access"' not in server_args
    finally:
        proc.wait(timeout=20)


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_non_imported_resume_keeps_legacy_independent_permission_flags(tmp_path):
    cwd = tmp_path / "project"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_SERVER_OUT"] = str(tmp_path / "server-argv.json")
    result = subprocess.run([sys.executable, str(_LANCADOR), "--name", "legacy", "--cwd", str(cwd),
                             "--resume", "legacy-thread", "--approval-policy", "never", "--sandbox", "read-only"],
                            env=env, capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stderr
    server_args = json.loads(Path(env["FAKE_SERVER_OUT"]).read_text())
    assert 'approval_policy="never"' in server_args and 'sandbox_mode="read-only"' in server_args


def _atualizacao(monkeypatch, tmp_path, publicada="0.161.0", npm_install_rc=0, view_rc=0,
                install_stderr="", install=None, instalado="/lib/node_modules/@openai/codex/bin/codex.js"):
    lancador = runpy.run_path(str(_LANCADOR))
    globais = lancador["_atualizar_codex"].__globals__
    chamadas = []

    def run(argv, **_):
        chamadas.append(argv[1:])
        if argv[1] == "install" and install is not None:
            return install(argv)
        saida = {"--version": "codex-cli 0.159.3\n", "view": f"{publicada}\n"}.get(argv[1], "")
        rc = {"install": npm_install_rc, "view": view_rc}.get(argv[1], 0)
        erro = install_stderr if argv[1] == "install" else ""
        return subprocess.CompletedProcess(argv, rc, stdout=saida, stderr=erro)

    monkeypatch.setattr(globais["shutil"], "which", lambda nome: f"/bin/{nome}")
    monkeypatch.setattr(globais["os"].path, "realpath", lambda _: instalado)
    monkeypatch.setattr(globais["subprocess"], "run", run)
    monkeypatch.setattr(globais["Path"], "home", lambda: tmp_path)
    return lancador["_atualizar_codex"], chamadas


def test_atualiza_o_codex_desatualizado_e_avisa_na_tela(monkeypatch, tmp_path, capsys):
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path)
    atualizar()
    assert ["install", "-g", "@openai/codex@0.161.0"] in chamadas
    err = capsys.readouterr().err
    assert "conferindo a versão do Codex…" in err
    assert "atualizando o Codex 0.159.3 → 0.161.0" in err


def test_consulta_a_versao_no_maximo_uma_vez_por_hora(monkeypatch, tmp_path):
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path, publicada="0.159.3")
    atualizar()
    feitas = len(chamadas)
    atualizar()
    assert len(chamadas) == feitas


def test_falha_do_npm_nao_impede_a_abertura(monkeypatch, tmp_path, capsys):
    atualizar, _ = _atualizacao(monkeypatch, tmp_path, npm_install_rc=1)
    atualizar()
    assert "a sessão abre na 0.159.3" in capsys.readouterr().err


def test_cache_que_nao_e_objeto_nao_derruba_o_lancador(monkeypatch, tmp_path, capsys):
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path, publicada="0.159.3")
    (tmp_path / ".hangar").mkdir()
    for conteudo in ("[]", '"x"', '{"proxima_em": "amanhã"}'):
        (tmp_path / ".hangar" / "codex-atualizacao.json").write_text(conteudo)
        atualizar()
    assert chamadas.count(["view", "@openai/codex", "version"]) == 3
    assert "não deu" not in capsys.readouterr().err


@pytest.mark.skipif(os.name != "posix", reason="a trava usa fcntl")
def test_trava_ocupada_avisa_e_nao_instala(monkeypatch, tmp_path, capsys):
    import fcntl
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path)
    (tmp_path / ".hangar").mkdir()
    with open(tmp_path / ".hangar" / "codex-atualizacao.lock", "w") as outra:
        fcntl.flock(outra, fcntl.LOCK_EX)
        atualizar()
    assert chamadas == []
    assert "outra sessão está atualizando o Codex" in capsys.readouterr().err


def test_falha_do_npm_view_avisa_e_tenta_de_novo_em_5_min(monkeypatch, tmp_path, capsys):
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path, view_rc=1)
    atualizar()
    assert "tenta de novo em 5 min" in capsys.readouterr().err
    feitas = len(chamadas)
    atualizar()
    assert len(chamadas) == feitas
    agora = time.time()
    monkeypatch.setattr(time, "time", lambda: agora + 301)
    atualizar()
    assert len(chamadas) > feitas


def test_install_que_passa_do_prazo_tem_mensagem_propria(monkeypatch, tmp_path, capsys):
    def estoura(argv):
        raise subprocess.TimeoutExpired(argv, 600)
    atualizar, _ = _atualizacao(monkeypatch, tmp_path, install=estoura)
    atualizar()
    err = capsys.readouterr().err
    assert "passou de 10 min e foi interrompida" in err
    assert "não deu para conferir" not in err


@pytest.mark.parametrize("stderr,classe", [
    ("npm ERR! code EACCES /usr/lib/node_modules", "sem permissão para instalar"),
    ("npm ERR! code ENOTFOUND registry.npmjs.org", "falha de rede"),
    ("npm ERR! algo estranho", "outro erro do npm"),
])
def test_falha_do_install_mostra_so_a_classe(monkeypatch, tmp_path, capsys, stderr, classe):
    atualizar, _ = _atualizacao(monkeypatch, tmp_path, npm_install_rc=1, install_stderr=stderr)
    atualizar()
    err = capsys.readouterr().err
    assert f"({classe})" in err and "tenta de novo em 1 h" in err
    assert "npm ERR!" not in err


def test_instalacao_fora_do_npm_avisa_uma_vez_por_hora(monkeypatch, tmp_path, capsys):
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path, instalado="/usr/local/bin/codex")
    monkeypatch.setitem(atualizar.__globals__, "_latest_standalone_version",
                        lambda: pytest.fail("instalação de fora não é standalone"))
    atualizar()
    atualizar()
    assert capsys.readouterr().err.count("atualização automática do Codex indisponível") == 1
    assert ["view", "@openai/codex", "version"] not in chamadas


def _standalone(monkeypatch, tmp_path, publicada="0.161.0", update_rc=0, troca=True, corpo="",
                pelo_current=False):
    """Árvore standalone falsa (layout do instalador oficial) com um `codex` falso de verdade.

    O `update` falso grava o ambiente que recebeu; com `troca`, o `--version` passa a responder a
    versão nova depois dele.
    """
    home = tmp_path / ".codex"
    release = home / "packages" / "standalone" / "releases" / "0.159.3-x86_64-unknown-linux-musl"
    (release / "bin").mkdir(parents=True)
    falso = release / "bin" / "codex"
    nova = '[ -e "$FAKE_UPDATE_OUT" ] && { echo "codex-cli 0.161.0"; exit 0; }; ' if troca else ""
    falso.write_text(f'#!/bin/sh\nif [ "$1" = --version ]; then {nova}echo "codex-cli 0.159.3"; exit 0; fi\n'
                     '[ "$1" = update ] || exit 9\n'
                     'printf "%s\\n" "$CODEX_HOME" "$CODEX_RELEASE" "$CODEX_INSTALL_DIR" > "$FAKE_UPDATE_OUT"\n'
                     f'{corpo}\nexit {update_rc}\n')
    falso.chmod(0o755)
    current = home / "packages" / "standalone" / "current"
    current.symlink_to(release)
    visivel = tmp_path / ".local" / "bin" / "codex"
    visivel.parent.mkdir(parents=True)
    visivel.symlink_to(current / "bin" / "codex")
    achado = current / "bin" / "codex" if pelo_current else visivel
    saida = tmp_path / "update.out"
    monkeypatch.setenv("FAKE_UPDATE_OUT", str(saida))
    # Valores da conta do Hangar, que não podem chegar ao instalador.
    monkeypatch.setenv("CODEX_HOME", str(tmp_path / "conta"))
    monkeypatch.setenv("CODEX_INSTALL_DIR", str(tmp_path / "conta" / "bin"))
    monkeypatch.setenv("CODEX_RELEASE", "0.1.0")

    atualizar = runpy.run_path(str(_LANCADOR))["_atualizar_codex"]
    globais = atualizar.__globals__
    consultas = []
    monkeypatch.setattr(globais["shutil"], "which", lambda nome: str(achado) if nome == "codex" else None)
    monkeypatch.setattr(globais["Path"], "home", lambda: tmp_path)
    monkeypatch.setitem(globais, "_latest_standalone_version",
                        lambda: consultas.append(1) or publicada)
    return atualizar, saida, consultas, home


def _proxima_em(tmp_path) -> float:
    return json.loads((tmp_path / ".hangar" / "codex-atualizacao.json").read_text())["proxima_em"] - time.time()


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_standalone_desatualizado_chama_o_atualizador_oficial(monkeypatch, tmp_path, capsys):
    atualizar, saida, _, home = _standalone(monkeypatch, tmp_path)
    atualizar()
    assert saida.read_text().splitlines() == [str(home), "0.161.0", str(tmp_path / ".local" / "bin")]
    err = capsys.readouterr().err
    assert "atualizando o Codex 0.159.3 → 0.161.0" in err and "Codex atualizado para 0.161.0" in err
    assert 3500 < _proxima_em(tmp_path) <= 3600


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_standalone_na_versao_publicada_nao_chama_o_atualizador(monkeypatch, tmp_path, capsys):
    atualizar, saida, consultas, _ = _standalone(monkeypatch, tmp_path, publicada="0.159.3")
    atualizar()
    atualizar()
    assert not saida.exists() and len(consultas) == 1
    assert "indisponível" not in capsys.readouterr().err
    assert 3500 < _proxima_em(tmp_path) <= 3600


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_falha_do_atualizador_standalone_abre_e_tenta_em_1_h(monkeypatch, tmp_path, capsys):
    atualizar, saida, _, _ = _standalone(monkeypatch, tmp_path, update_rc=3)
    atualizar()
    assert saida.exists()
    err = capsys.readouterr().err
    assert "atualizador oficial saiu com código 3" in err
    assert "a sessão abre na 0.159.3 e tenta de novo em 1 h" in err
    assert 3500 < _proxima_em(tmp_path) <= 3600


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_atualizador_que_sai_com_0_sem_trocar_a_versao_nao_e_sucesso(monkeypatch, tmp_path, capsys):
    atualizar, saida, _, _ = _standalone(monkeypatch, tmp_path, troca=False)
    atualizar()
    assert saida.exists()
    err = capsys.readouterr().err
    assert "Codex responde 0.159.3 em vez de 0.161.0" in err and "tenta de novo em 1 h" in err
    assert "Codex atualizado" not in err
    assert 3500 < _proxima_em(tmp_path) <= 3600


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_codex_achado_dentro_do_standalone_nao_leva_pasta_de_link(monkeypatch, tmp_path):
    atualizar, saida, _, home = _standalone(monkeypatch, tmp_path, pelo_current=True)
    atualizar()
    # Sem pasta de link a conta não vaza: o instalador fica no padrão dele.
    assert saida.read_text().splitlines() == [str(home), "0.161.0", ""]


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_prazo_estourado_mata_o_instalador_neto(monkeypatch, tmp_path, capsys):
    neto = tmp_path / "neto.pid"
    atualizar, _, _, _ = _standalone(monkeypatch, tmp_path,
                                     corpo=f'sleep 60 & echo $! > "{neto}"; wait')
    monkeypatch.setitem(atualizar.__globals__, "_UPDATE_TIMEOUT_S", 1)
    atualizar()
    assert "passou de 10 min e foi interrompida" in capsys.readouterr().err
    pid = int(neto.read_text())

    def morto():
        try:
            return Path(f"/proc/{pid}/stat").read_text().split()[2] == "Z"
        except OSError:
            return True
    assert _espera(morto, 5)


def test_npm_em_pasta_packages_standalone_de_monorepo_nao_e_standalone():
    standalone_home = runpy.run_path(str(_LANCADOR))["_standalone_home"]
    assert standalone_home("/w/mono/packages/standalone/node_modules/@openai/codex/bin/codex.js") is None
    assert standalone_home("/h/.codex/packages/standalone/releases/0.1.0-x/bin/codex") == Path("/h/.codex")


def test_canal_standalone_le_a_tag_com_user_agent_proprio(monkeypatch):
    import io
    versao = runpy.run_path(str(_LANCADOR))["_latest_standalone_version"]
    pedidos = []

    def urlopen(pedido, timeout):
        pedidos.append(pedido)
        return io.BytesIO(b'{"tag_name": "rust-v0.161.0", "assets": []}')
    monkeypatch.setattr(versao.__globals__["urllib"].request, "urlopen", urlopen)
    assert versao() == "0.161.0"
    # Com o User-Agent padrão do urllib o canal responde 403.
    assert pedidos[0].get_header("User-agent") == "hangar-codex-tui"


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_falha_ao_consultar_o_canal_standalone_tenta_em_5_min(monkeypatch, tmp_path, capsys):
    import urllib.error
    atualizar, saida, _, _ = _standalone(monkeypatch, tmp_path)

    def sem_rede():
        raise urllib.error.URLError("offline")
    monkeypatch.setitem(atualizar.__globals__, "_latest_standalone_version", sem_rede)
    atualizar()
    assert not saida.exists()
    assert "tenta de novo em 5 min" in capsys.readouterr().err
    assert 0 < _proxima_em(tmp_path) <= 300


def _hooks_server(respostas_batch):
    """App-server falso: responde initialize, hooks/list e config/batchWrite."""
    from websockets.sync.server import serve

    gravado = []
    hooks = [
        {"key": "user:Stop:0", "source": "user", "enabled": True, "trustStatus": "untrusted",
         "currentHash": "h1"},
        {"key": "plugin:x:Stop:0", "source": "plugin", "enabled": True, "trustStatus": "modified",
         "currentHash": "h2"},
        {"key": "project:Stop:0", "source": "project", "enabled": True, "trustStatus": "untrusted",
         "currentHash": "h3"},
        {"key": f"{Path.home() / '.codex' / 'hooks.json'}:stop:0:0", "source": "project", "enabled": True,
         "trustStatus": "untrusted", "currentHash": "h4"},
    ]

    def atender(ws):
        for bruto in ws:
            msg = json.loads(bruto)
            ws.send(json.dumps({"jsonrpc": "2.0", "method": "aviso", "params": {}}))
            if msg["method"] == "hooks/list":
                ws.send(json.dumps({"id": msg["id"], "result": {"data": [{"hooks": hooks}]}}))
            elif msg["method"] == "config/batchWrite":
                gravado.extend(msg["params"]["edits"])
                ws.send(json.dumps({"id": msg["id"], **respostas_batch}))
            else:
                ws.send(json.dumps({"id": msg["id"], "result": {}}))

    servidor = serve(atender, "127.0.0.1", 0)
    threading.Thread(target=servidor.serve_forever, daemon=True).start()
    return servidor, f"ws://127.0.0.1:{servidor.socket.getsockname()[1]}", gravado


@pytest.mark.parametrize("resposta", [{"result": {}}, {"error": {"code": -32603, "message": "segredo"}}])
def test_confia_so_nos_hooks_sincronizados_pelo_hangar(capsys, resposta):
    confiar = runpy.run_path(str(_LANCADOR))["_confiar_hooks"]
    servidor, endpoint, gravado = _hooks_server(resposta)
    try:
        confirmed = confiar(endpoint, "/tmp")
    finally:
        servidor.shutdown()
    chaves = [json.loads("[" + e["keyPath"].replace('"."', '","') + "]")[2] for e in gravado]
    sincronizado = f"{Path.home() / '.codex' / 'hooks.json'}:stop:0:0"
    assert chaves == ["user:Stop:0", "plugin:x:Stop:0", sincronizado]
    err = capsys.readouterr().err
    assert "segredo" not in err
    if "error" in resposta:
        assert confirmed is False
        assert "aceitos" not in err
        assert "config/batchWrite, código -32603" in err
    else:
        assert confirmed is True
        assert "1 hooks de outra origem" in err


@pytest.mark.skipif(os.name != "posix", reason="a trava usa fcntl")
def test_falha_fora_da_consulta_grava_nova_tentativa_em_5_min(monkeypatch, tmp_path, capsys):
    import errno
    import fcntl
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path)

    def falha(*_):
        raise OSError(errno.EIO, "io")
    monkeypatch.setattr(fcntl, "flock", falha)
    atualizar()
    assert "não deu para preparar a atualização do Codex" in capsys.readouterr().err
    proxima = json.loads((tmp_path / ".hangar" / "codex-atualizacao.json").read_text())["proxima_em"]
    assert 0 < proxima - time.time() <= 300
    assert chamadas == []


@pytest.mark.skipif(os.name != "posix" or os.geteuid() == 0, reason="permissão de pasta POSIX")
def test_pasta_sem_escrita_ainda_atualiza_e_avisa(monkeypatch, tmp_path, capsys):
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path)
    pasta = tmp_path / ".hangar"
    pasta.mkdir()
    pasta.chmod(0o500)
    try:
        atualizar()
    finally:
        pasta.chmod(0o700)
    assert ["install", "-g", "@openai/codex@0.161.0"] in chamadas
    assert "não deu para criar a trava da atualização do Codex" in capsys.readouterr().err


@pytest.mark.parametrize("conteudo", ['{"proxima_em": NaN}', '{"proxima_em": 1e999}',
                                      '{"proxima_em": 99999999999}'])
def test_prazo_absurdo_no_cache_nao_desliga_a_atualizacao(monkeypatch, tmp_path, conteudo):
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path, publicada="0.159.3")
    (tmp_path / ".hangar").mkdir()
    (tmp_path / ".hangar" / "codex-atualizacao.json").write_text(conteudo)
    atualizar()
    assert ["view", "@openai/codex", "version"] in chamadas


@pytest.mark.parametrize("publicada", ["0.161.0 --foo", "9.9.9.9", "v0.161.0", "0.161.0\x1b[2J"])
def test_versao_publicada_malformada_nao_e_instalada(monkeypatch, tmp_path, capsys, publicada):
    atualizar, chamadas = _atualizacao(monkeypatch, tmp_path, publicada=publicada)
    atualizar()
    assert not any(c[0] == "install" for c in chamadas)
    err = capsys.readouterr().err
    assert "tenta de novo em 5 min" in err and "\x1b" not in err


@pytest.mark.skipif(os.name != "posix", reason="binário falso POSIX")
def test_retomada_com_par_fora_dos_modos_conserva_o_modo_gravado(tmp_path):
    # Aprovação e sandbox que não formam um modo do app: não há o que gravar, e o modo que já estava
    # (de quem a sessão criadora herda) não pode virar nulo.
    cwd = tmp_path / "project"
    cwd.mkdir()
    env = _ambiente(tmp_path, cwd)
    env["FAKE_TUI_SLEEP"] = "3"
    thread_id = "same-thread"
    rollout = Path(env["CODEX_HOME"]) / "sessions" / "2026" / "10" / "03" / f"rollout-example-{thread_id}.jsonl"
    rollout.parent.mkdir(parents=True)
    rollout.write_text("{}\n")
    sidecar = _sidecar(env, "sess")
    sidecar.parent.mkdir(parents=True)
    sidecar.write_text(json.dumps({"name": "sess", "thread_id": thread_id, "key": "durable-key", "cwd": str(cwd),
                                   "rollout_path": str(rollout), "permission_mode": "Ask for approval"}))
    proc = subprocess.Popen([sys.executable, str(_LANCADOR), "--name", "sess", "--resume", thread_id,
                             "--approval-policy", "never", "--sandbox", "read-only"],
                            env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        # `app_pid` só chega pela gravação da retomada (a TUI grava `tui_pid` antes).
        assert _espera(lambda: sidecar.exists() and json.loads(sidecar.read_text()).get("app_pid"))
        assert json.loads(sidecar.read_text())["permission_mode"] == "Ask for approval"
    finally:
        proc.wait(timeout=20)
