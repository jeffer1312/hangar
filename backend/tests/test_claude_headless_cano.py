# backend/tests/test_claude_headless_cano.py
"""Cano da sessão sem terminal: o processo sobrevive ao cliente e o snapshot diz o que está em
aberto. O `claude` é um script falso que fala stream-json: responde initialize, e a cada prompt
pede uma permissão e só fecha o turno quando ela é respondida.

Contrato do `hangar-cano` (Rust), o cano que o servidor lança.
"""
import json
import os
import signal
import socket
import subprocess
import sys
import time
import uuid
from pathlib import Path

import pytest

import rust_bins

_TOKEN = "synthetic-cano-token"

_CLAUDE_FALSO = r'''
import json, sys
def out(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for linha in sys.stdin:
    ev = json.loads(linha)
    t = ev.get("type")
    if t == "control_request":
        sub = ev["request"]["subtype"]
        if sub == "initialize":
            out({"type": "system", "subtype": "init", "session_id": "sid-1", "model": "haiku", "permissionMode": "default"})
        elif sub == "emit_peer":
            out({"type": "command_lifecycle", "command_uuid": "peer-1", "state": "started"})
        elif sub == "finish_peer":
            out({"type": "result", "subtype": "success", "usage": {"input_tokens": 1}})
        out({"type": "control_response", "response": {"subtype": "success", "request_id": ev["request_id"], "response": {}}})
    elif t == "user":
        if ev["message"]["content"][0]["text"] == "sair":
            sys.stderr.write("tchau\n"); sys.stderr.flush(); sys.exit(3)
        out({"type": "control_request", "request_id": "perm-1",
             "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "ls"}}})
    elif t == "control_response":
        out({"type": "assistant", "message": {"content": [{"type": "text", "text": "feito"}]}})
        out({"type": "result", "subtype": "success", "usage": {"input_tokens": 1}})
sys.stderr.write("tchau\n")
'''


@pytest.fixture
def cano_cmd() -> list[str]:
    exe = rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN")
    if exe is None:
        pytest.skip("hangar-cano não compilado: rode `cargo build --release -p hangar-cano` em crates/ "
                    "ou aponte CP_RUST_CANO_BIN para o binário")
    return [str(exe)]


@pytest.fixture
def cano(tmp_path, cano_cmd):
    if os.name == "nt":
        pytest.skip("socket unix")
    falso = tmp_path / "claude_falso.py"
    falso.write_text(_CLAUDE_FALSO, encoding="utf-8")
    sock = tmp_path / "c.sock"
    log = tmp_path / "cano.log"
    p = subprocess.Popen([*cano_cmd, "--escuta", f"unix:{sock}", "--token", _TOKEN, "--log", str(log),
                          "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    for _ in range(100):
        if sock.exists():
            break
        time.sleep(0.05)
    assert sock.exists(), log.read_text() if log.exists() else "sem log"
    yield sock, p, log
    if p.poll() is None:
        p.kill()
    p.wait()


class _Cliente:
    def __init__(self, sock: Path):
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.connect(str(sock))
        self.s.settimeout(5)
        self.s.sendall((_TOKEN + "\n").encode())
        self.arq = self.s.makefile("rb")

    def manda(self, obj) -> None:
        self.s.sendall((json.dumps(obj) + "\n").encode())

    def le(self) -> dict:
        event = json.loads(self.arq.readline())
        return json.loads(event["frame"]) if event.get("type") == "cano_output" else event

    def le_ate(self, tipo: str) -> dict:
        while True:
            ev = self.le()
            if ev.get("type") == tipo:
                return ev

    def fecha(self) -> None:
        self.arq.close()   # o makefile segura o socket: só o close dele entrega o EOF ao cano
        self.s.close()


def test_snapshot_tem_os_campos_e_a_versao(cano):
    # O servidor só religa um cano de `versao` 2 (runtime/cano.rs).
    sock, proc, log = cano
    a = _Cliente(sock)
    snap = a.le()
    assert set(snap) == {"type", "versao", "pid", "init", "aberto", "pendentes", "ultimo_result",
                         "rate_limit", "stderr_tail", "saiu", "inflight"}
    assert snap["versao"] == 2 and isinstance(snap["pid"], int)
    assert snap["pendentes"] == [] and snap["stderr_tail"] == [] and snap["saiu"] is None
    a.fecha()


def test_snapshot_reconstroi_turno_aberto_e_permissao_pendente(cano):
    sock, proc, log = cano
    a = _Cliente(sock)
    snap = a.le()
    assert snap["type"] == "cano_snapshot" and snap["init"] is None and not snap["aberto"]
    a.manda({"type": "control_request", "request_id": "r1", "request": {"subtype": "initialize"}})
    assert a.le_ate("system")["subtype"] == "init"
    a.le_ate("control_response")
    a.manda({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "oi"}]}})
    pedido = a.le_ate("control_request")
    assert pedido["request_id"] == "perm-1"
    # O backend "cai" com a permissão pendente. O claude (falso) continua vivo no cano.
    a.fecha()
    time.sleep(0.2)
    assert proc.poll() is None
    b = _Cliente(sock)
    snap = b.le()
    assert json.loads(snap["init"])["subtype"] == "init"
    assert snap["aberto"] is True
    assert [json.loads(p)["request_id"] for p in snap["pendentes"]] == ["perm-1"]
    assert snap["saiu"] is None
    # O backend novo responde a permissão pendente e o turno fecha normalmente.
    b.manda({"type": "control_response", "response": {"subtype": "success", "request_id": "perm-1",
                                                       "response": {"behavior": "allow"}}})
    assert b.le_ate("result")["subtype"] == "success"
    b.fecha()
    c = _Cliente(sock)
    snap = c.le()
    assert snap["aberto"] is False and snap["pendentes"] == []
    assert json.loads(snap["ultimo_result"])["type"] == "result"
    c.fecha()


def test_snapshot_preserva_turno_iniciado_por_mensagem_de_outra_sessao(cano):
    sock, proc, log = cano
    a = _Cliente(sock)
    a.le()
    a.manda({"type": "control_request", "request_id": "peer", "request": {"subtype": "emit_peer"}})
    assert a.le_ate("command_lifecycle")["state"] == "started"
    a.le_ate("control_response")
    a.fecha()
    time.sleep(0.2)

    b = _Cliente(sock)
    assert b.le()["aberto"] is True
    b.manda({"type": "control_request", "request_id": "fim", "request": {"subtype": "finish_peer"}})
    assert b.le_ate("result")["subtype"] == "success"
    b.le_ate("control_response")
    b.fecha()
    time.sleep(0.2)

    c = _Cliente(sock)
    assert c.le()["aberto"] is False
    c.fecha()


def test_saida_do_claude_chega_ao_cliente_ligado_com_stderr(cano):
    sock, proc, log = cano
    a = _Cliente(sock)
    a.le()
    a.manda({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "sair"}]}})
    assert a.le_ate("cano_stderr")["linha"] == "tchau"
    saiu = a.le_ate("cano_saiu")
    assert saiu["rc"] == 3 and saiu["stderr_tail"] == ["tchau"]
    a.fecha()
    proc.wait(timeout=5)     # entregou o rc: sai sozinho, sem esperar o teto, e limpa o socket
    assert not sock.exists()


def test_saida_do_claude_sem_cliente_fica_no_snapshot(cano):
    sock, proc, log = cano
    a = _Cliente(sock)
    a.le()
    a.manda({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "sair"}]}})
    a.fecha()                # o backend caiu antes de ver a saída
    time.sleep(0.5)
    assert proc.poll() is None   # o cano espera alguém buscar o rc
    b = _Cliente(sock)
    snap = b.le()
    assert snap["saiu"] == 3 and snap["stderr_tail"] == ["tchau"]
    assert b.le()["type"] == "cano_saiu"   # e ainda manda o evento pra quem chegou depois
    b.fecha()
    proc.wait(timeout=5)


def test_sigterm_encerra_o_filho_e_limpa_o_socket(cano):
    if not Path("/proc").exists():
        pytest.skip("confere o filho pelo /proc")
    sock, proc, log = cano
    a = _Cliente(sock)
    filho = a.le()["pid"]
    a.fecha()
    proc.send_signal(signal.SIGTERM)
    assert proc.wait(timeout=5) == 0
    assert not sock.exists()
    inicio = _estado(filho)
    vistos = []
    for _ in range(100):
        vivo = _vivo(filho)
        vistos.append(f"{vivo}: {_estado(filho)}")
        if not vivo:
            break
        time.sleep(0.05)
    # O CI já viu o laço sair cedo e o mesmo pid "vivo" logo depois: o registro diz o que o /proc mostrou.
    assert not _vivo(filho), f"antes={inicio} laço={vistos} agora={_estado(filho)} log={log.read_text() if log.exists() else 'sem log'}"


def test_codigos_de_saida(cano_cmd, tmp_path):
    if os.name == "nt":
        pytest.skip("socket unix")
    sem_comando = subprocess.run([*cano_cmd, "--escuta", f"unix:{tmp_path / 'a.sock'}"],
                                 capture_output=True, timeout=10)
    assert sem_comando.returncode == 2
    log = tmp_path / "cano.log"
    longo = f"unix:{tmp_path / ('x' * 120 + '.sock')}"     # passa do limite do kernel para socket unix
    r = subprocess.run([*cano_cmd, "--escuta", longo, "--token", _TOKEN, "--log", str(log), "--", sys.executable, "-c", "pass"],
                       capture_output=True, timeout=10)
    assert r.returncode == 1 and "não consegui escutar" in log.read_text(encoding="utf-8")
    r = subprocess.run([*cano_cmd, "--escuta", f"unix:{tmp_path / 'b.sock'}", "--token", _TOKEN, "--log", str(log),
                        "--", str(tmp_path / "nao-existe")], capture_output=True, timeout=10)
    assert r.returncode == 1 and "claude não subiu" in log.read_text(encoding="utf-8")


_APP_SERVER_FALSO = r'''
import json, sys
def out(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for linha in sys.stdin:
    ev = json.loads(linha)
    if ev.get("method") == "turn/start":
        out({"jsonrpc": "2.0", "id": ev["id"], "result": {"turn": {"id": "t1"}}})
        out({"jsonrpc": "2.0", "id": 0, "method": "item/commandExecution/requestApproval",
             "params": {"threadId": "th", "itemId": "i1", "command": "touch x"}})
        out({"jsonrpc": "2.0", "id": 1, "method": "item/fileChange/requestApproval",
             "params": {"threadId": "th", "itemId": "i2"}})
    elif ev.get("method") == "turn/interrupt":
        out({"jsonrpc": "2.0", "id": ev["id"], "result": {}})
        out({"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th"}})
    elif "method" not in ev and ev.get("id") is not None:
        out({"jsonrpc": "2.0", "method": "serverRequest/resolved", "params": {"threadId": "th", "requestId": ev["id"]}})
        if ev["id"] == 1:
            out({"jsonrpc": "2.0", "method": "turn/completed", "params": {"threadId": "th"}})
'''


def test_pedido_jsonrpc_do_servidor_fica_pendente_no_snapshot(tmp_path, cano_cmd):
    if os.name == "nt":
        pytest.skip("socket unix")
    falso = tmp_path / "app_server_falso.py"
    falso.write_text(_APP_SERVER_FALSO, encoding="utf-8")
    sock = tmp_path / "c.sock"
    p = subprocess.Popen([*cano_cmd, "--escuta", f"unix:{sock}", "--token", _TOKEN, "--log", str(tmp_path / "cano.log"),
                          "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        for _ in range(100):
            if sock.exists():
                break
            time.sleep(0.05)
        a = _Cliente(sock)
        a.le()
        a.manda({"jsonrpc": "2.0", "id": 7, "method": "turn/start", "params": {}})
        assert a.le()["id"] == 7
        assert a.le()["method"] == "item/commandExecution/requestApproval"
        assert a.le()["method"] == "item/fileChange/requestApproval"
        a.manda({"jsonrpc": "2.0", "id": 0, "result": {"decision": "accept"}})    # respondeu um só
        assert a.le()["method"] == "serverRequest/resolved"
        a.fecha()
        time.sleep(0.2)
        b = _Cliente(sock)
        snap = b.le()
        assert [json.loads(x)["id"] for x in snap["pendentes"]] == [1]
        # Turno fechado sem resolver o pedido (interrupção) leva o pendente junto.
        b.manda({"jsonrpc": "2.0", "id": 8, "method": "turn/interrupt", "params": {"threadId": "th"}})
        assert b.le()["id"] == 8
        assert b.le()["method"] == "turn/completed"
        b.fecha()
        time.sleep(0.2)
        c = _Cliente(sock)
        assert c.le()["pendentes"] == []
        c.fecha()
    finally:
        if p.poll() is None:
            p.kill()
        p.wait()


def test_token_errado_e_recusado(tmp_path, cano_cmd):
    if os.name == "nt":
        pytest.skip("socket unix")
    token = uuid.uuid4().hex
    p, porta, s, _ = _cano_tcp(cano_cmd, tmp_path, token)
    try:
        s.close()
        s = _conectar_tcp(porta)
        s.sendall(b"errado\n")
        assert s.makefile("rb").readline() == b""     # fechado sem snapshot
        s = _conectar_tcp(porta)
        s.sendall((token + "\n").encode())
        assert json.loads(s.makefile("rb").readline())["type"] == "cano_snapshot"
        s.close()
    finally:
        p.kill()
        p.wait()


def test_porta_tomada_por_outro_servidor_sobe_o_cano_noutra(tmp_path, cano_cmd, monkeypatch):
    import http.server
    import threading
    alheio = http.server.HTTPServer(("127.0.0.1", 0), http.server.BaseHTTPRequestHandler)
    threading.Thread(target=alheio.serve_forever, daemon=True).start()
    real = _porta_livre
    portas = iter([alheio.server_address[1]])
    monkeypatch.setattr(sys.modules[__name__], "_porta_livre", lambda: next(portas, None) or real())
    try:
        p, porta, s, _ = _cano_tcp(cano_cmd, tmp_path, uuid.uuid4().hex)
        try:
            assert porta != alheio.server_address[1]
        finally:
            s.close()
            p.kill()
            p.wait()
    finally:
        alheio.shutdown()
        alheio.server_close()


def test_cliente_novo_substitui_o_ligado_em_tcp(tmp_path, cano_cmd):
    # Roda também no Windows (TCP + token). Com o accept em série, o segundo cliente não recebia
    # snapshot enquanto o primeiro seguia ligado — e quem conecta sem snapshot mata o cano.
    token = uuid.uuid4().hex
    p, porta, a, arq_a = _cano_tcp(cano_cmd, tmp_path, token)
    try:
        b = _conectar_tcp(porta)
        b.settimeout(3)
        b.sendall((token + "\n").encode())
        arq_b = b.makefile("rb")
        assert json.loads(arq_b.readline())["type"] == "cano_snapshot"
        try:
            assert arq_a.readline() == b""        # o antigo foi desligado
        except OSError:
            pass
        b.sendall((json.dumps({"type": "control_request", "request_id": "r1",
                               "request": {"subtype": "initialize"}}) + "\n").encode())
        event = json.loads(arq_b.readline())
        if event.get("type") == "cano_output":
            event = json.loads(event["frame"])
        assert event["type"] == "system"   # o novo fala com o claude
        assert p.poll() is None
    finally:
        p.kill()
        p.wait()


def _vivo(pid: int) -> bool:
    # Zumbi conta como morto: quem colhe o filho do cano é o init, no tempo dele.
    try:
        with open(f"/proc/{pid}/stat", encoding="ascii") as f:
            return f.read().rsplit(") ", 1)[1][0] != "Z"
    except (FileNotFoundError, ProcessLookupError):
        return False


def _estado(pid: int) -> str:
    try:
        with open(f"/proc/{pid}/stat", encoding="ascii") as f:
            campos = f.read().rsplit(") ", 1)[1].split()
        # estado, ppid e starttime: starttime diferente no mesmo pid é outro processo.
        return f"{campos[0]} ppid={campos[1]} start={campos[19]} t={time.monotonic():.3f}"
    except (OSError, IndexError) as e:
        return f"{type(e).__name__} t={time.monotonic():.3f}"


def test_process_disappears_during_stat_read(monkeypatch):
    from unittest.mock import mock_open
    opened = mock_open()
    opened.return_value.read.side_effect = ProcessLookupError("process exited")
    monkeypatch.setattr("builtins.open", opened)
    assert not _vivo(123)


def _porta_livre() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _cano_tcp(cano_cmd, tmp_path, token):
    """Sobe o cano em TCP e devolve (processo, porta, socket, arquivo) com o snapshot já lido.
    A porta é escolhida antes de o cano subir, e outro teste em paralelo pode tomá-la nesse
    intervalo: a conexão cairia no servidor dele. O snapshot prova que é o cano; sem ele, outra porta."""
    falso = tmp_path / "claude_falso.py"
    falso.write_text(_CLAUDE_FALSO, encoding="utf-8")
    respostas = []
    for _ in range(5):
        porta = _porta_livre()
        p = subprocess.Popen([*cano_cmd, "--escuta", f"tcp:127.0.0.1:{porta}", "--token", token,
                              "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            s = _conectar_tcp(porta)
            s.sendall((token + "\n").encode())
            arq = s.makefile("rb")
            linha = arq.readline()
        except BaseException:  # ninguém escutando ou prazo estourado: defeito, não porta tomada
            p.kill()
            p.wait()
            raise
        try:
            evento = json.loads(linha)
        except ValueError:
            evento = None
        if isinstance(evento, dict) and evento.get("type") == "cano_snapshot":
            return p, porta, s, arq
        s.close()
        p.kill()
        p.wait()
        # Resposta que não é JSON veio de outro servidor na porta; JSON sem snapshot é defeito do cano.
        if evento is not None:
            raise AssertionError(f"o cano respondeu sem snapshot: {linha[:200]!r}")
        respostas.append(linha[:120])
    raise AssertionError(f"a porta escolhida estava sempre com outro servidor: {respostas}")


def _conectar_tcp(porta: int) -> socket.socket:
    for _ in range(100):
        try:
            s = socket.create_connection(("127.0.0.1", porta), timeout=2)
            s.settimeout(5)
            return s
        except OSError:
            time.sleep(0.05)
    raise AssertionError("cano não escutou")
