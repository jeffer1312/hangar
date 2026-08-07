import json
import subprocess
import time
import pytest
from fastapi.testclient import TestClient

from app import termsock
from app.config import settings

SESS = "cp-test-termsock"


def _client(host="127.0.0.1"):
    """Sem `with`: o lifespan do app sobe watchers, um registry.list() real e DUAS chamadas de rede
    (pricing, usd_brl). Nenhuma rota daqui precisa disso. `client=` explicito porque o TestClient
    conecta como host 'testclient' por padrao — mesmo contorno de tests/test_pi_inbox_route.py:5-13.
    """
    settings.auth_token = "secret"
    from app.api import app
    return TestClient(app, client=(host, 12345))


@pytest.fixture
def sessao():
    subprocess.run(["tmux", "kill-session", "-t", f"={SESS}"], capture_output=True)
    subprocess.run(["tmux", "new-session", "-d", "-s", SESS, "-x", "200", "-y", "50"], check=True)
    yield SESS
    subprocess.run(["tmux", "kill-session", "-t", f"={SESS}"], capture_output=True)


def _tam(nome):
    # `={nome}` sozinho (sem `:`) deixa window_width/window_height vazios no `display -p` — medido
    # no Step 1 (brief so cobria o #{window-size}, nao essa parte). Com `:` (janela ativa) volta
    # certo; e a mesma correcao aplicada em termsock._tamanho_da_janela.
    cp = subprocess.run(["tmux", "display", "-p", "-t", f"={nome}:",
                         "#{window_width}x#{window_height} #{window-size}"],
                        capture_output=True, text=True)
    return cp.stdout.strip()


def _clientes(nome):
    return subprocess.run(["tmux", "list-clients", "-t", f"={nome}"],
                          capture_output=True, text=True).stdout.strip()


def _esperar(condicao, timeout=5.0, passo=0.1):
    """Poll com limite: a suite inteira (1500+ testes, subprocessos por todo lado) as vezes deixa
    o _desmontar (que gira ate 3s por dentro) mais lento que um sleep fixo de 2s cobre — visto na
    corrida completa, nao na execucao isolada deste arquivo. Sem poll, um `time.sleep` fixo vira
    teste instavel; sem limite, um teste realmente quebrado trava a suite (achado desta corrida).
    """
    limite = time.monotonic() + timeout
    while time.monotonic() < limite:
        if condicao():
            return
        time.sleep(passo)
    assert condicao()


def test_token_errado_fecha_sem_abrir_pty(sessao):
    c = _client()
    with pytest.raises(Exception):
        with c.websocket_connect(f"/api/sessions/{sessao}/term?token=errado&cols=80&rows=24"):
            pass
    assert termsock.clientes_ativos() == set()


def test_sessao_desconhecida_recusa(sessao):
    c = _client()
    with pytest.raises(Exception):
        with c.websocket_connect("/api/sessions/nao-existe/term?token=secret&cols=80&rows=24"):
            pass


def test_anexa_recebe_bytes_e_repoe_tamanho_ao_sair(sessao):
    c = _client()
    assert _tam(sessao).startswith("200x50")
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        assert len(ws.receive_bytes()) > 0          # tmux pintou a tela
        time.sleep(1.0)
        assert _tam(sessao).startswith("80x24")     # encolheu, como desenhado
        assert sessao in termsock.clientes_ativos()
        ws.close()
        # Fechar e conferir AINDA DENTRO do `with`: o `__exit__` do WebSocketTestSession do
        # Starlette so ENFILEIRA o disconnect (nao espera o handler processar) e cancela a
        # cancel-scope do servidor logo em seguida — se o nosso `_desmontar` (que gira ate 3s)
        # ainda estiver rodando quando isso acontece, ele e cortado NO MEIO, e o cliente tmux
        # fica pra sempre anexado (medido: pid do fork continuava vivo e anexado minutos depois,
        # sem nenhuma chamada a `detach-client` no log — achado desta corrida, fora do brief).
        # Esperar aqui, antes do `with` fechar de verdade, tira a corrida.
        # As DUAS metades: tamanho reposto E window-size de volta em latest. Sem a segunda, um
        # attach nativo posterior abriria recortado e ninguem saberia por que (medicao no spec,
        # linha 5).
        _esperar(lambda: _tam(sessao) == "200x50 latest")
        assert termsock.clientes_ativos() == set()
        assert _clientes(sessao) == ""


def test_segunda_conexao_derruba_a_primeira(sessao):
    c = _client()
    url = f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24"
    with c.websocket_connect(url) as a:
        a.receive_bytes()
        with c.websocket_connect(url) as b:
            b.receive_bytes()
            time.sleep(0.8)
            # `clientes_ativos()` e chaveado por NOME: contar 1 seria verdade mesmo sem derrubar
            # ninguem (achado do pass). Quem prova e o tmux: um cliente anexado, uma linha.
            assert len(_clientes(sessao).splitlines()) == 1


def test_resize_chega_no_pty(sessao):
    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        ws.send_text(json.dumps({"t": "resize", "cols": 100, "rows": 30}))
        time.sleep(1.0)
        assert _tam(sessao).startswith("100x30")


def test_pty_morto_fecha_o_socket(sessao):
    # O `fim` tem que ser esperado de verdade: sem isso o handler fica parado no receive() e o
    # painel congela pra sempre quando o attach sai (achado do pass).
    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        subprocess.run(["tmux", "detach-client", "-s", f"={sessao}"], capture_output=True)
        with pytest.raises(Exception):
            for _ in range(50):
                ws.receive_bytes()


def test_fechamento_feio_nao_deixa_zumbi_nem_trava_a_listagem(sessao):
    c = _client()
    for _ in range(5):
        with c.websocket_connect(
                f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
            ws.receive_bytes()
            ws.close()          # sem handshake limpo
            # Espera DENTRO do with, mesmo motivo do teste anterior: o `__exit__` do
            # WebSocketTestSession cancela o servidor logo depois de enfileirar o disconnect, sem
            # esperar o `_desmontar` terminar.
            _esperar(lambda: _clientes(sessao) == "")
    # A regressao do threadpool: se as bombas de bytes tivessem sido to_thread, os workers ficariam
    # parqueados e este GET seria o que trava.
    r = c.get("/api/sessions", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    assert termsock.clientes_ativos() == set()
    assert _clientes(sessao) == ""
