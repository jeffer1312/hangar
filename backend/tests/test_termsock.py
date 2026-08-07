import asyncio
import json
import subprocess
import time
import anyio
import pytest
from fastapi.testclient import TestClient
from starlette.websockets import WebSocket

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


def _receive_bytes_com_teto(ws, segundos=5.0):
    """`ws.receive_bytes()` do Starlette bloqueia SEM prazo (portal.call num stream sem timeout).
    Se o handler nunca fechar o socket (regressao), um `for _ in range(N)` sozinho so limita
    ITERACOES — cada chamada individual ainda pendura pra sempre, e a suite INTEIRA trava atras
    dela (achado da revisao). `anyio.fail_after` da o prazo de verdade.
    """
    async def _rx():
        with anyio.fail_after(segundos):
            msg = await ws._send_rx.receive()
        ws._raise_on_close(msg)
        return msg["bytes"]
    return ws.portal.call(_rx)


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
            # B tem que receber bytes DE VERDADE depois de derrubar A — nao so existir. Sem isso
            # o C3 (reader do fd reusado engolido) passava verde por sorte de numeracao de fd: a
            # contagem de "1 cliente" abaixo ja era verdade mesmo com o terminal do B mudo,
            # contanto que o tmux visse 1 attach (achado da revisao).
            assert len(b.receive_bytes()) > 0
            # `clientes_ativos()` e chaveado por NOME: contar 1 seria verdade mesmo sem derrubar
            # ninguem (achado do pass). Quem prova e o tmux: um cliente anexado, uma linha.
            _esperar(lambda: len(_clientes(sessao).splitlines()) == 1)
            # E A tem que ter sido FECHADO de verdade (I1) — sem aviso, quem foi derrubado ficava
            # com o terminal congelado pra sempre em vez de ver a desconexao (achado da revisao).
            # Loop com teto (nao so a proxima chamada): pode ainda sobrar no ar um pedaco legitimo
            # de tela que o PTY mandou pra A antes do close tomar efeito.
            with pytest.raises(Exception):
                for _ in range(10):
                    _receive_bytes_com_teto(a, segundos=2.0)


def test_resize_chega_no_pty(sessao):
    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        ws.send_text(json.dumps({"t": "resize", "cols": 100, "rows": 30}))
        time.sleep(1.0)
        assert _tam(sessao).startswith("100x30")


def test_saida_acima_do_teto_reata_o_reader_depois_de_drenar(sessao, monkeypatch):
    """Q1 da rodada 2 de revisao: `pausado` recalculado no TOPO do laco do escritor (em vez de
    marcado por QUEM pausa, o `do_pty`) perdia uma pausa que acontecesse NO MEIO do dreno — dentro
    do `await ws.send_bytes`, que e exatamente onde o laco de eventos roda callbacks de reader. O
    reader sumia de vez e o terminal ficava mudo pra sempre (reproduzido em
    scratchpad/sim_i8.py, achado da revisao). Teto e leitura BEM pequenos pra cruzar o limite em
    varias leituras curtas (nao numa so), e `send_bytes` propositalmente lento — o mesmo "celular
    em 4G" do sim_i8 — pra forcar a pausa a cair no meio de um envio. Prova de verdade: nao so que
    o handler nao caiu, mas que BYTES NOVOS chegam depois do burst.
    """
    monkeypatch.setattr(termsock, "_SAIDA_MAX", 200)
    monkeypatch.setattr(termsock, "_LEITURA", 64)
    original = WebSocket.send_bytes

    async def _lento(self, data):
        await asyncio.sleep(0.03)
        await original(self, data)

    monkeypatch.setattr(WebSocket, "send_bytes", _lento)

    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()                       # tela inicial
        # burst bem maior que o teto de 200B, numa leva so
        subprocess.run(["tmux", "send-keys", "-t", f"={sessao}:", "-l",
                         "yes XXXXXXXXXX | head -c 20000"], check=True)
        subprocess.run(["tmux", "send-keys", "-t", f"={sessao}:", "Enter"], check=True)
        # drena o que o burst mandou ate a torneira secar (nao ate um numero fixo de mensagens —
        # com o bug, secava cedo demais e ficava mudo; sem bug, so para quando nao chega mais nada
        # por 0.5s). Com teto: se o bug voltar, isto NAO trava a suite.
        limite = time.monotonic() + 8.0
        while time.monotonic() < limite:
            try:
                _receive_bytes_com_teto(ws, segundos=0.5)
            except Exception:
                break
        # PROVA: sessao ainda responde depois do burst — o reader voltou de verdade.
        subprocess.run(["tmux", "send-keys", "-t", f"={sessao}:", "-l", "echo PASSOU_DO_TETO"],
                        capture_output=True)
        subprocess.run(["tmux", "send-keys", "-t", f"={sessao}:", "Enter"], capture_output=True)
        visto = False
        for _ in range(50):
            if b"PASSOU_DO_TETO" in _receive_bytes_com_teto(ws, segundos=2.0):
                visto = True
                break
        assert visto


def test_pty_morto_fecha_o_socket(sessao):
    # O `fim` tem que ser esperado de verdade: sem isso o handler fica parado no receive() e o
    # painel congela pra sempre quando o attach sai (achado do pass).
    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        subprocess.run(["tmux", "detach-client", "-s", f"={sessao}"], capture_output=True)
        with pytest.raises(Exception):
            for _ in range(50):
                _receive_bytes_com_teto(ws)


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


def test_selecionar_opcao_recusa_com_painel_aberto(sessao, monkeypatch):
    monkeypatch.setitem(termsock._ativos, sessao, object())
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/select",
               json={"option": 1}, headers={"Authorization": "Bearer secret"})
    assert r.status_code == 409
    assert "terminal" in r.json()["detail"].lower()


def test_mandar_prompt_continua_funcionando_com_painel_aberto(sessao, monkeypatch):
    # Enviar e NUCLEO: nao pode ficar esperando feature (incidente de 2026-07-23).
    monkeypatch.setitem(termsock._ativos, sessao, object())
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/input",
               json={"text": "oi"}, headers={"Authorization": "Bearer secret"})
    assert r.status_code != 409


def test_shell_cria_sessao_escondida_que_nao_aparece_na_lista(sessao):
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/shell", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    nome_shell = r.json()["shell"]
    # A sessao EXISTE no tmux — o usuario alcanca pelo painel e pelo terminal nativo...
    assert subprocess.run(["tmux", "has-session", "-t", f"={nome_shell}"],
                          capture_output=True).returncode == 0
    # ...e NAO aparece no app. Sem isto ela viraria card nas tres views (lista, board, canvas),
    # porque registry.py:240 trata pane nao reconhecido como Claude por padrao.
    from app.registry import SessionRegistry
    nomes = [i.name for i in SessionRegistry().list()]
    assert nome_shell not in nomes
    assert sessao in nomes                     # a sessao do agente continua aparecendo
    subprocess.run(["tmux", "kill-session", "-t", f"={nome_shell}"], capture_output=True)


def test_shell_e_idempotente(sessao):
    # Reatar em vez de criar outra: o painel reabre depois de recarregar a pagina e encontra o
    # mesmo shell, com o comando ainda rodando.
    c = _client()
    a = c.post(f"/api/sessions/{sessao}/shell", headers={"Authorization": "Bearer secret"}).json()
    b = c.post(f"/api/sessions/{sessao}/shell", headers={"Authorization": "Bearer secret"}).json()
    assert a["shell"] == b["shell"]
    saida = subprocess.run(["tmux", "list-sessions", "-F", "#{session_name}"],
                           capture_output=True, text=True).stdout.splitlines()
    assert saida.count(a["shell"]) == 1
    subprocess.run(["tmux", "kill-session", "-t", f"={a['shell']}"], capture_output=True)


def test_sessao_escondida_nao_muda_o_custo_da_listagem(sessao):
    # A marca e lida no MESMO `list-panes -a` que ja rodava: filtrar nao pode custar fork novo.
    c = _client()
    c.post(f"/api/sessions/{sessao}/shell", headers={"Authorization": "Bearer secret"})
    from app import tmux as tmux_mod
    from app.registry import SessionRegistry
    chamadas = []
    orig = tmux_mod.RUN
    tmux_mod.RUN = lambda args, **kw: (chamadas.append(args), orig(args, **kw))[1]
    try:
        SessionRegistry().list()
    finally:
        tmux_mod.RUN = orig
    assert sum(1 for a in chamadas if "list-panes" in a) == 1
    subprocess.run(["tmux", "kill-session", "-t", f"=term-{sessao}"], capture_output=True)


def test_open_terminal_sem_emulador_devolve_erro_visivel(sessao, monkeypatch):
    monkeypatch.delenv("CP_TERMINAL", raising=False)   # o codigo checa a env ANTES do PATH
    monkeypatch.setattr("app.api.shutil.which", lambda _: None)   # so o do api, nao o do tmux.py
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/open-terminal", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 503
    assert "emulador" in r.json()["detail"].lower()
