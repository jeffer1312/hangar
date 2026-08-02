import pytest

from app.config import settings
from app.pi_inbox import INBOX


def _client(host="127.0.0.1"):
    """`client=` explícito: o TestClient conecta como host 'testclient' por padrão
    (starlette/testclient.py:392) e cairia na recusa de não-loopback da rota. Mesmo contorno de
    tests/test_auth_backoff.py:21."""
    settings.auth_token = "secret"
    from app.api import app
    from fastapi.testclient import TestClient
    return TestClient(app, client=(host, 12345))


def test_conexao_sem_token_e_recusada():
    client = _client()
    with pytest.raises(Exception):
        with client.websocket_connect("/api/pi/inbox"):
            pass


def test_registra_e_desregistra_pelo_pane():
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"pane": "%33"})
        ws.send_json({"pong": True})   # round-trip: garante que o registro já aconteceu
        assert INBOX.tem_linha("%33") is True
    assert INBOX.tem_linha("%33") is False, "fechou o socket, a linha tem que sair"


def test_primeira_mensagem_sem_pane_fecha():
    """Sem identidade não dá pra registrar — e registrar com chave errada é pior que recusar."""
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"nada": 1})
        with pytest.raises(Exception):
            ws.receive_json()


def test_confirmacao_de_id_desconhecido_nao_derruba():
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"pane": "%9"})
        ws.send_json({"id": "inexistente", "ok": True})
        ws.send_json({"pong": True})
        assert INBOX.tem_linha("%9") is True


def test_mensagem_gigante_fecha_a_linha():
    """O middleware de body-size ignora WebSocket de propósito (api.py:83), então o teto é aqui."""
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"pane": "%7"})
        ws.send_text("x" * (256 * 1024 + 10))
        with pytest.raises(Exception):
            ws.receive_json()


def test_primeira_mensagem_gigante_fecha_antes_de_registrar():
    """Achado da revisão: receive_json() direto na 1ª mensagem pulava o teto de tamanho — só o
    loop depois do registro passava por len(bruto). O teto tem que valer ANTES do json.loads."""
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_text("x" * (256 * 1024 + 10))
        with pytest.raises(Exception):
            ws.receive_json()


def test_conexao_do_bind_configurado_e_aceita(monkeypatch):
    """Achado da revisão final: com CP_LAN_BIND_IP=auto/IP fixo de LAN, o uvicorn NÃO escuta em
    loopback (main.py: resolve_bind_ip) — a extensão (mesma máquina) chega com esse endereço como
    origem, e recusar isso é a linha do Pi nunca conectar em produção."""
    from app import api
    monkeypatch.setattr(api, "resolve_bind_ip", lambda s: "192.168.1.50")
    client = _client(host="192.168.1.50")
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"pane": "%50"})
        ws.send_json({"pong": True})
        assert INBOX.tem_linha("%50") is True


def test_conexao_de_outro_ip_da_lan_continua_recusada(monkeypatch):
    """Aceitar o bind configurado NÃO pode virar aceitar qualquer host da LAN — só o endereço que o
    próprio backend subiu (a defesa real contra quem não está na máquina)."""
    from app import api
    monkeypatch.setattr(api, "resolve_bind_ip", lambda s: "192.168.1.50")
    client = _client(host="192.168.1.99")
    with pytest.raises(Exception):
        with client.websocket_connect("/api/pi/inbox?token=secret"):
            pass


def test_recusa_por_token_agora_loga(caplog):
    """Achado ALTA da revisao 02/08/2026: ate aqui a recusa por token era MUDA — nem no terminal do
    Pi nem no log do backend sobrava rastro de por que a linha rapida nunca ligava."""
    client = _client()
    with caplog.at_level("WARNING", logger="claude_pocket"):
        with pytest.raises(Exception):
            with client.websocket_connect("/api/pi/inbox"):   # sem ?token=
                pass
    assert "token recusado" in caplog.text


def test_recusa_por_token_loga_so_uma_vez_ate_acertar(caplog):
    """Cuidado com enxurrada: o retry da extensao roda em laco com recuo — logar TODA tentativa
    inundaria o journal. Aviso unico por host, ate uma conexao daquele host dar certo."""
    client = _client()
    with caplog.at_level("WARNING", logger="claude_pocket"):
        for _ in range(3):
            with pytest.raises(Exception):
                with client.websocket_connect("/api/pi/inbox"):
                    pass
    avisos = [r for r in caplog.records if "token recusado" in r.getMessage()]
    assert len(avisos) == 1


def test_recusa_por_origem_agora_loga(monkeypatch, caplog):
    """Mesmo achado, pela outra porta: host fora do bind aceito tambem era recusado calado."""
    from app import api
    monkeypatch.setattr(api, "resolve_bind_ip", lambda s: "192.168.1.50")
    client = _client(host="192.168.1.99")
    with caplog.at_level("WARNING", logger="claude_pocket"):
        with pytest.raises(Exception):
            with client.websocket_connect("/api/pi/inbox?token=secret"):
                pass
    assert "origem recusada" in caplog.text


def test_recusa_para_e_conexao_boa_reabre_o_aviso(caplog):
    """O aviso nao e 'uma vez na vida do processo': depois de uma conexao BOA daquele host, uma
    recusa nova (token girou de novo) tem que voltar a logar."""
    client = _client()
    with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
        ws.send_json({"pane": "%1"})
        ws.send_json({"pong": True})
    with caplog.at_level("WARNING", logger="claude_pocket"):
        with pytest.raises(Exception):
            with client.websocket_connect("/api/pi/inbox"):   # token errado de novo
                pass
    assert "token recusado" in caplog.text


def test_heartbeat_sem_resposta_derruba_linha_zumbi(monkeypatch):
    """Achado da revisão: send_json sozinho não pega a linha zumbi (buffer do SO absorvendo o
    ping com o outro lado travado) — só o CONTADOR fecha. _WS_PING/_WS_PINGS_SEM_RESPOSTA_MAX
    encolhidos pra não pagar o prazo real em segundos."""
    from app import api
    monkeypatch.setattr(api, "_WS_PING", 0.05)
    monkeypatch.setattr(api, "_WS_PINGS_SEM_RESPOSTA_MAX", 1)
    client = _client()
    with pytest.raises(Exception):
        with client.websocket_connect("/api/pi/inbox?token=secret") as ws:
            ws.send_json({"pane": "%42"})
            ws.send_json({"pong": True})  # confirma que o registro aconteceu antes do silêncio
            assert INBOX.tem_linha("%42") is True
            for _ in range(20):
                ws.receive_json()  # dreno os pings até a linha fechar por conta própria
    assert INBOX.tem_linha("%42") is False
