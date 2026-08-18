"""Checagem de um peer (Task 8) — a primitiva "este endereço responde?" da aba Servidores.

A Task 3 criou `alcance.testar_endereco` (estado nomeado + motivo + tempo) e a Task 5 gravou os
peers. Esta Task cola as duas: o estado de cada lado de um peer registrado vem de uma checagem
contra o identificador da máquina, não de um erro genérico. Quatro estados nomeados, os do plano:
`ok` (responde e o identificador bate), `estranho` (responde mas é outro identificador), `falhou`
(não responde) e `recusou` (credencial errada). O teste troca só a primitiva de I/O (`_bater`), o
mesmo seam que `test_alcance.py` usa.
"""
import json

import pytest

from app import alcance, peers, peers_check

# A função real de I/O, capturada antes do fixture autouse trocá-la: o teste do socket local
# (Authorization no header) chama ELA, não o fake — mesmo padrão de test_alcance.py:132.
_bater_real = peers_check._bater


class _bater_fake:
    """Dupla da primitiva de I/O — devolve (status, corpo) como o real faria. O teste dirige o script."""

    def __init__(self):
        self.chamadas: list[tuple[str, str, str | None]] = []
        self._status = 0
        self._corpo = None
        self._lanca: Exception | None = None

    def __call__(self, url: str, path: str = "/api/peers/identificador", token: str | None = None) -> tuple[int, dict | None]:
        self.chamadas.append((url, path, token))
        if self._lanca is not None:
            raise self._lanca
        try:
            return self._status, json.loads(self._corpo) if self._corpo else None
        except (json.JSONDecodeError, ValueError):
            return self._status, None

    def dirige(self, status: int, corpo: str | None):
        self._status = status
        self._corpo = corpo
        self._lanca = None

    def dirige_rede(self, exc: Exception):
        self._lanca = exc


@pytest.fixture(autouse=True)
def _seam(monkeypatch):
    """Sempre isola a rede: nenhum teste desta casa toca processo real (régua do contrato)."""
    dupla = _bater_fake()
    monkeypatch.setattr(peers_check, "_bater", dupla)
    return dupla


# Estado ok: responde e o identificador bate.
def test_ok_quando_responde_com_identificador_igual(_seam):
    _seam.dirige(200, '{"identificador": "notebook"}')
    r = peers_check.checar_peer("http://notebook:8765", "notebook")
    assert r["estado"] == "ok"
    assert r["identificador"] == "notebook"
    assert "tempo_ms" in r and r["motivo"] == ""
    assert _seam.chamadas[0][0].rstrip("/") == "http://notebook:8765"


def test_ok_com_tempo_medido(_seam):
    _seam.dirige(200, '{"identificador": "notebook"}')
    r = peers_check.checar_peer("http://notebook:8765", "notebook")
    assert isinstance(r["tempo_ms"], (int, float)) and r["tempo_ms"] >= 0


# Estado estranho: responde, mas é OUTRA máquina no mesmo endereço (mock: "responde mas é outro
# identificador" — o plano nomeia os estados; trocar de máquina é o motivo do bloco de correção).
def test_estranho_quando_identificador_diverge(_seam):
    _seam.dirige(200, '{"identificador": "outra-maquina"}')
    r = peers_check.checar_peer("http://notebook:8765", "notebook")
    assert r["estado"] == "estranho"
    assert r["identificador"] == "outra-maquina"


def test_estranho_quando_identificador_ausente(_seam):
    # Backend antigo sem a rota /identificador devolve 404 — respondeu, mas não dá pra confirmar.
    _seam.dirige(404, '{"detail": "nao existe"}')
    r = peers_check.checar_peer("http://notebook:8765", "notebook")
    assert r["estado"] == "estranho"


def test_estranho_quando_corpo_invalido(_seam):
    # 200 com corpo que não é o contrato: respondeu, mas não dá pra confirmar a identidade.
    _seam.dirige(200, "nao-json")
    r = peers_check.checar_peer("http://notebook:8765", "notebook")
    assert r["estado"] == "estranho"


# Estado recusou: a credencial do peer foi recusada pelo backend remoto (401).
def test_recusou_quando_credencial_rejeitada(_seam):
    _seam.dirige(401, '{"detail": "token invalido"}')
    r = peers_check.checar_peer("http://notebook:8765", "notebook")
    assert r["estado"] == "recusou"
    assert r["motivo"] == "credencial"


# A correção da rodada 2: o batimento usa a credencial DO PEER (token do peers.json). Sem ela o
# remoto responde 401 -> recusou (medido na fumaça: os dois sentidos recusavam). O duplo continua
# na I/O (_bater); quem está sob prova é o caminho token-pra-header.
def test_pega_o_token_do_peer_no_peers_json(_seam, monkeypatch):
    # peer registrado com token: checar_peer resolve pelo id e o batimento sai com o token.
    monkeypatch.setattr(peers, "peer_cfg", lambda sid: ("http://x:8765", "tok-123") if sid == "notebook" else None)
    _seam.dirige(200, '{"identificador": "notebook"}')
    r = peers_check.checar_peer("http://notebook:8765", "notebook")
    assert r["estado"] == "ok"
    assert _seam.chamadas[0][2] == "tok-123"


def test_sem_peer_registrado_vai_sem_token(_seam, monkeypatch):
    # peer fora do peers.json (ou sem token): GET sai sem Authorization; o remoto devolve 401 e
    # o estado é recusou — "credencial não configurada" é estado nomeado, não erro.
    monkeypatch.setattr(peers, "peer_cfg", lambda sid: None)
    _seam.dirige(401, '{"detail": "x"}')
    r = peers_check.checar_peer("http://n:8765", "notebook")
    assert r["estado"] == "recusou"
    assert _seam.chamadas[0][2] is None


def test_bater_real_manda_authorization_do_token(monkeypatch):
    """O `_bater` REAL monta o header Authorization quando recebe o token.

    Socket local, porta efêmera, sem rede externa nem processo — mesmo padrão de test_alcance.py.
    Cai sem a correção (o GET ia sem header) e passa com ela.
    """
    import http.server
    import socketserver
    import threading

    recebido: dict[str, str] = {}

    class H(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self):
            recebido["authorization"] = self.headers.get("Authorization") or ""
            corpo = json.dumps({"identificador": "x"}).encode()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(corpo)))
            self.end_headers()
            self.wfile.write(corpo)

        def log_message(self, *a):
            pass

    with socketserver.TCPServer(("127.0.0.1", 0), H) as srv:
        porta = srv.server_address[1]
        t = threading.Thread(target=srv.handle_request, daemon=True)
        t.start()
        # Restaura a função real (o fixture autouse trocou pelo fake) só neste teste.
        monkeypatch.setattr(peers_check, "_bater", _bater_real)
        status, corpo = peers_check._bater(f"http://127.0.0.1:{porta}", token="tok-123")
        t.join(timeout=2)
    assert status == 200
    assert corpo == {"identificador": "x"}
    assert recebido["authorization"] == "Bearer tok-123"


# Estado falhou: o peer não responde (timeout/recusa de conexão/erro de rede).
def test_falhou_quando_transporte_falha(_seam):
    _seam.dirige_rede(TimeoutError("teto estourou"))
    r = peers_check.checar_peer("http://notebook:8765", "notebook")
    assert r["estado"] == "falhou"
    assert r["motivo"] in ("timeout", "recusou", "erro")


# URL vazia nunca é testada: "não configurado" não é defeito (mesmo contrato de testar_endereco).
def test_nao_configurado_quando_url_vazia(_seam):
    r = peers_check.checar_peer("", "notebook")
    assert r["estado"] == "nao_configurado"
    assert _seam.chamadas == []


# Nenhum texto de interface nasce no Python: o front traduz pelo code (régua da casa).
def test_estados_sao_nomeados(_seam):
    _seam.dirige(200, '{"identificador": "notebook"}')
    assert peers_check.checar_peer("http://n:8765", "notebook")["estado"] == "ok"
    _seam.dirige(401, '{"detail": "x"}')
    assert peers_check.checar_peer("http://n:8765", "notebook")["estado"] == "recusou"
    # usa a primitiva da Task 3: o teto de espera é o MESMO número do alcance
    assert peers_check.TETO_ESPERA_S == alcance.TETO_ESPERA_S
