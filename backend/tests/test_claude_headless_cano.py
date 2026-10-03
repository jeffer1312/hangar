# backend/tests/test_claude_headless_cano.py
"""Cano da sessão sem terminal: o processo sobrevive ao cliente e o snapshot diz o que está em
aberto. O `claude` é um script falso que fala stream-json: responde initialize, e a cada prompt
pede uma permissão e só fecha o turno quando ela é respondida.

Cada teste que sobe um cano roda duas vezes: contra o `cano.py` e contra o `hangar-cano` (Rust).
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

from app import rust_bins
from app.adapters.claude_headless import cano as cano_mod

CANO = Path(__file__).resolve().parents[1] / "app" / "adapters" / "claude_headless" / "cano.py"

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


@pytest.fixture(params=["cano.py", "hangar-cano"])
def cano_cmd(request) -> list[str]:
    """Lançador do cano. Os mesmos testes provam as duas implementações do mesmo contrato."""
    if request.param == "cano.py":
        return [sys.executable, str(CANO)]
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
    p = subprocess.Popen([*cano_cmd, "--escuta", f"unix:{sock}", "--log", str(log),
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
        self.arq = self.s.makefile("rb")

    def manda(self, obj) -> None:
        self.s.sendall((json.dumps(obj) + "\n").encode())

    def le(self) -> dict:
        return json.loads(self.arq.readline())

    def le_ate(self, tipo: str) -> dict:
        while True:
            ev = self.le()
            if ev.get("type") == tipo:
                return ev

    def fecha(self) -> None:
        self.arq.close()   # o makefile segura o socket: só o close dele entrega o EOF ao cano
        self.s.close()


def test_snapshot_tem_os_campos_e_a_versao_do_cano_py(cano):
    # O adapter compara `versao` com `cano_mod.VERSAO` para decidir se reabre: as duas
    # implementações precisam falar a mesma.
    sock, proc, log = cano
    a = _Cliente(sock)
    snap = a.le()
    assert set(snap) == {"type", "versao", "pid", "init", "aberto", "pendentes", "ultimo_result",
                         "rate_limit", "stderr_tail", "saiu"}
    assert snap["versao"] == cano_mod.VERSAO and isinstance(snap["pid"], int)
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
    for _ in range(100):
        if not _vivo(filho):
            break
        time.sleep(0.05)
    assert not _vivo(filho)


def test_codigos_de_saida(cano_cmd, tmp_path):
    if os.name == "nt":
        pytest.skip("socket unix")
    sem_comando = subprocess.run([*cano_cmd, "--escuta", f"unix:{tmp_path / 'a.sock'}"],
                                 capture_output=True, timeout=10)
    assert sem_comando.returncode == 2
    log = tmp_path / "cano.log"
    longo = f"unix:{tmp_path / ('x' * 120 + '.sock')}"     # passa do limite do kernel para socket unix
    r = subprocess.run([*cano_cmd, "--escuta", longo, "--log", str(log), "--", sys.executable, "-c", "pass"],
                       capture_output=True, timeout=10)
    assert r.returncode == 1 and "não consegui escutar" in log.read_text(encoding="utf-8")
    r = subprocess.run([*cano_cmd, "--escuta", f"unix:{tmp_path / 'b.sock'}", "--log", str(log),
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
    p = subprocess.Popen([*cano_cmd, "--escuta", f"unix:{sock}", "--log", str(tmp_path / "cano.log"),
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
    falso = tmp_path / "claude_falso.py"
    falso.write_text(_CLAUDE_FALSO, encoding="utf-8")
    porta = _porta_livre()
    token = uuid.uuid4().hex
    p = subprocess.Popen([*cano_cmd, "--escuta", f"tcp:127.0.0.1:{porta}", "--token", token,
                          "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
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


def test_cliente_novo_substitui_o_ligado_em_tcp(tmp_path, cano_cmd):
    # Roda também no Windows (TCP + token). Com o accept em série, o segundo cliente não recebia
    # snapshot enquanto o primeiro seguia ligado — e quem conecta sem snapshot mata o cano.
    falso = tmp_path / "claude_falso.py"
    falso.write_text(_CLAUDE_FALSO, encoding="utf-8")
    porta, token = _porta_livre(), uuid.uuid4().hex
    p = subprocess.Popen([*cano_cmd, "--escuta", f"tcp:127.0.0.1:{porta}", "--token", token,
                          "--cwd", str(tmp_path), "--", sys.executable, str(falso)],
                         stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        a = _conectar_tcp(porta)
        a.sendall((token + "\n").encode())
        arq_a = a.makefile("rb")
        assert json.loads(arq_a.readline())["type"] == "cano_snapshot"
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
        assert json.loads(arq_b.readline())["type"] == "system"   # o novo fala com o claude
        assert p.poll() is None
    finally:
        p.kill()
        p.wait()


def test_stderr_na_codepage_do_windows_nao_vira_caractere_quebrado():
    import importlib.util
    spec = importlib.util.spec_from_file_location("cano_mod", CANO)
    cano_mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(cano_mod)
    assert cano_mod._texto_do_stderr("não existe".encode("utf-8")) == "não existe"
    cp = cano_mod._texto_do_stderr("não existe".encode("cp1252"))
    assert "�" not in cp and cp.startswith("n") and cp.endswith("o existe")


def test_suite_nunca_le_os_sidecars_reais():
    from app.adapters.claude_headless import sessions
    real = Path.home() / ".hangar" / "claude-headless"
    assert sessions._dir() != real and real not in sessions._dir().parents


def _vivo(pid: int) -> bool:
    # Zumbi conta como morto: quem colhe o filho do cano é o init, no tempo dele.
    try:
        with open(f"/proc/{pid}/stat", encoding="ascii") as f:
            return f.read().rsplit(") ", 1)[1][0] != "Z"
    except FileNotFoundError:
        return False


def _porta_livre() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def _conectar_tcp(porta: int) -> socket.socket:
    for _ in range(100):
        try:
            s = socket.create_connection(("127.0.0.1", porta), timeout=2)
            s.settimeout(5)
            return s
        except OSError:
            time.sleep(0.05)
    raise AssertionError("cano não escutou")
