import json
import os
import socket
import threading

import pytest

from app import uds_messaging


def test_separar_prefixo():
    assert uds_messaging.separar_prefixo("[de: hangar] oi") == ("hangar", "oi")
    assert uds_messaging.separar_prefixo("[grupo: x] aviso") == ("x", "aviso")
    assert uds_messaging.separar_prefixo("fala da pessoa") == (None, "fala da pessoa")


def test_socket_da_sessao_le_o_registro_do_cli(tmp_path, monkeypatch):
    # O CLI grava `<config>/sessions/<pid>.json` com sessionId e messagingSocketPath; e por ele
    # (nao pelo pane) que a sessao sem terminal e achada.
    monkeypatch.setattr(uds_messaging, "_cc_socks", lambda: tmp_path)
    sock = tmp_path / "1.sock"
    sock.touch()
    sessions = tmp_path / "cfg" / "sessions"
    sessions.mkdir(parents=True)
    (sessions / f"{os.getpid()}.json").write_text(json.dumps({
        "pid": str(os.getpid()), "sessionId": "abc", "messagingSocketPath": str(sock)}))
    assert uds_messaging.socket_da_sessao("abc", str(tmp_path / "cfg")) == str(sock)
    assert uds_messaging.socket_da_sessao("outra", str(tmp_path / "cfg")) is None


def test_enviar_escreve_o_quadro_do_send_message(tmp_path):
    # Formato medido em 21/09/2026 com um socket falso capturando o SendMessage do CLI 2.1.278.
    caminho = str(tmp_path / "alvo.sock")
    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(caminho)
    srv.listen(1)
    recebido: list[bytes] = []

    def _aceita():
        c, _ = srv.accept()
        buf = b""
        while True:
            ch = c.recv(65536)
            if not ch:
                break
            buf += ch
        recebido.append(buf)
        c.close()

    t = threading.Thread(target=_aceita)
    t.start()
    mid = uds_messaging.enviar(caminho, "[de: a] corpo\nlinha 2", "a", "bypass")
    t.join(5)
    srv.close()
    linhas = recebido[0].decode().splitlines()
    assert len(linhas) == 1
    q = json.loads(linhas[0])
    assert q["msgV"] == 1 and q["msg_id"] == mid and q["type"] == "user" and q["priority"] == "next"
    assert q["message"]["role"] == "user"
    assert q["message"]["content"].startswith('<cross-session-message from="uds:')
    assert 'from-name="a" from-mode="bypass">\n[de: a] corpo\nlinha 2\n</cross-session-message>' in q["message"]["content"]
    assert q["from"].startswith("uds:")


def test_enviar_levanta_sem_socket(tmp_path):
    with pytest.raises(OSError):
        uds_messaging.enviar(str(tmp_path / "nao-existe.sock"), "[de: a] x", "a", "bypass")



