import asyncio
import contextlib
import json
import logging
import os
import subprocess
import tempfile
import time
import types
import anyio
import pytest
from fastapi.testclient import TestClient
from starlette.websockets import WebSocket

from app import termsock

from tmux_teste import matar_servidor, matar_sessao, novo_socket
from app.config import settings

SESS = "cp-test-termsock"

# Diretorio de trabalho dos shells escondidos deste arquivo. Era "/tmp" cru: no Windows esse caminho
# nao existe, o `new-session -c /tmp` cai no cwd do processo e o `#{session_path}` volta
# outro caminho qualquer — ai o guard "o shell e deste diretorio?" via divergencia onde nao havia e
# recriava a sessao, derrubando tres casos por um motivo que nao e o deles. `gettempdir()` e "/tmp"
# no Linux (o mesmo caminho de antes) e o Temp do perfil no Windows.
DIR_NEUTRO = tempfile.gettempdir()

# O painel tem DOIS motores desde 22/08/2026 (pty.fork no POSIX, ConPTY no Windows), entao sao
# dois marcadores, e a diferenca entre eles importa:
#
# `com_painel` — o caso vale nos DOIS sistemas e roda onde houver painel. Era o que o comentario
#   antigo daqui prometia ("se um dia o painel ganhar motor pro Windows, e este marcador que sai —
#   os casos ja estao escritos"), e e o que aconteceu.
# `so_com_pty` — o caso e do MOTOR POSIX, nao do painel: ele afirma coisa que so existe la (fd
#   herdavel do `pty.fork`, zumbi/`waitpid`, repor o tamanho da janela no desmonte). Nao e "o
#   Windows nao chegou la": no Windows aquilo nao tem contraparte, e forcar uma seria inventar
#   afirmacao. Onde ha contraparte de verdade, ela esta escrita como caso proprio no fim do arquivo.
com_painel = pytest.mark.skipif(not termsock.painel_disponivel(),
                                reason="esta maquina nao abre painel de terminal")
so_com_pty = pytest.mark.skipif(os.name != "posix",
                                reason="afirma coisa do motor POSIX (fd do pty.fork, waitpid, "
                                       "repor tamanho) — no Windows nao ha contraparte")


def _emulador_falso(tmp_path, nome, exit_code=0, mensagem=None):
    """Emulador de terminal de mentira que a rota consiga RODAR nesta plataforma.

    A rota `open-terminal` executa o candidato de verdade (e o que ela existe pra provar: um
    emulador que sai cedo nao pode virar 200). Um `#!/bin/sh` no Windows nem chega a rodar — vira
    WinError 193/216 e a rota devolve `erro_terminal_abertura_falhou`, que e outro codigo. Mesma
    solucao do tests/test_cli_probe.py: `.cmd` la, script sh aqui.
    """
    if os.name == "nt":
        alvo = tmp_path / f"{nome}.cmd"
        linhas = ["@echo off"]
        if mensagem:
            linhas.append(f"echo {mensagem} 1>&2")
        linhas.append(f"exit /b {exit_code}")
        alvo.write_text("\n".join(linhas) + "\n", encoding="ascii")
        return alvo
    alvo = tmp_path / nome
    corpo = "#!/bin/sh\n"
    if mensagem:
        corpo += f"echo '{mensagem}' >&2\n"
    corpo += f"exit {exit_code}\n"
    alvo.write_text(corpo)
    alvo.chmod(0o755)
    return alvo


def _client(host="127.0.0.1"):
    """Sem `with`: o lifespan do app sobe watchers, um registry.list() real e DUAS chamadas de rede
    (pricing, usd_brl). Nenhuma rota daqui precisa disso. `client=` explicito porque o TestClient
    conecta como host 'testclient' por padrao — mesmo contorno de tests/test_pi_inbox_route.py:5-13.
    """
    settings.auth_token = "secret"
    from app.api import app
    return TestClient(app, client=(host, 12345))


@contextlib.contextmanager
def _um_laco_so(c):
    """Poe TODAS as conexoes deste cliente no MESMO laco de eventos — que e o que a producao tem.

    Sem `with`, o `TestClient` cria um portal (thread + laco de eventos) POR `websocket_connect`.
    Com uma conexao so isso e invisivel; com DUAS ao mesmo tempo, os dois handlers passam a viver
    em lacos DIFERENTES, e a derrubada (que mexe em objetos do handler antigo: `cancel()` na task
    escritora, `pause_reading()` no transporte) vira chamada cross-thread. O que ela agenda e um
    `call_soon`, que enfileira no laco do outro sem ACORDAR o IOCP dele — e o handler derrubado so
    volta a rodar quando algo mais o acorda. Medido em 22/08/2026 com marcadores de tempo: a thread
    do desmonte dele terminava em 1ms e a corrotina ficava 550ms parada num `await` (ate num
    `asyncio.sleep(0)`), ate o `with` do teste sair e cancelar tudo — e ai o future do portal volta
    CANCELADO e o teste falha em 2 de 3 corridas, sem que nada do caminho de producao esteja errado.
    No Linux o mesmo desenho passa por sorte de plataforma: a morte do pty gera evento no fd que o
    handler antigo ainda observa, entao o laco dele acorda sozinho.

    Um uvicorn tem UM laco pra todas as conexoes. Compartilhar o portal aqui e o teste ficando mais
    parecido com producao, nao menos: `TestClient.portal` e o mesmo atributo que o `__enter__` dele
    preenche — a diferenca e que aqui nao roda o lifespan (ver `_client`).
    """
    import anyio.from_thread

    with anyio.from_thread.start_blocking_portal(**c.async_backend) as portal:
        c.portal = portal
        try:
            yield c
        finally:
            c.portal = None


@pytest.fixture
def sessao():
    matar_sessao(SESS)
    matar_sessao(f"term-{SESS}")
    subprocess.run(["tmux", "new-session", "-d", "-s", SESS, "-x", "200", "-y", "50"], check=True)
    yield SESS
    # Achado da revisao (I3): a limpeza do shell escondido tem que estar AQUI (teardown do
    # fixture, roda sempre), nao no fim do corpo de cada teste -- um assert falhando pulava a
    # linha e "term-cp-test-termsock" vazava pro servidor tmux DEFAULT do usuario, invisivel no
    # app (marcada @cp_hidden) e por isso indetectavel sem olhar o tmux na mao.
    matar_sessao(SESS)
    matar_sessao(f"term-{SESS}")


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


def _anexado(nome):
    """Tem cliente anexado? — a pergunta que vale nos DOIS multiplexadores.

    `_clientes()` NAO serve pra isso fora do Linux: medido no psmux 3.3.7 (22/08/2026), com a
    sessao comprovadamente sem ninguem anexado (`#{session_attached}` = 0) o `list-clients -t =S`
    ainda devolve rc=0 e UMA LINHA — `/dev/pts/0: <sessao>: powershell [200x50] (utf8)` —, um
    cliente que nao existe, com o tty ficticio de sempre. Um `assert _clientes(x) == ""` la nao
    falha por regressao: falha porque a pergunta nao tem resposta naquele comando (o que o psmux
    de fato marca no cliente vivo e o sufixo `[activity=...]`, e nas sessoes o `(attached)`).
    `#{session_attached}` responde certo nos dois.
    """
    cp = subprocess.run(["tmux", "display", "-p", "-t", f"={nome}:", "#{session_attached}"],
                        capture_output=True, text=True)
    return cp.stdout.strip() not in ("", "0")


def _tam_bate(nome, cols, rows):
    """A janela ficou do tamanho do painel? — com a linha de status descontada onde ela existe.

    A largura e exata nos dois multiplexadores. A ALTURA nao: o psmux reserva uma linha pra barra
    de status, entao um cliente de 80x24 deixa a janela em 80x23 (medido), enquanto o tmux desta
    suite relata 80x24. Cobrar `rows` cravado seria cobrar do Windows uma linha que a barra dele
    ocupa — falha de contabilidade, nao de redimensionamento. Aceita `rows` ou `rows-1` e continua
    exigindo a largura exata, que e o que de fato prova que o resize chegou.
    """
    atual = _tam(nome).split()[0] if _tam(nome) else ""
    return atual in (f"{cols}x{rows}", f"{cols}x{rows - 1}")


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


@com_painel
def test_anexa_recebe_bytes_e_solta_a_sessao_ao_sair(sessao):
    """A metade do caso que vale nos dois motores: anexa, pinta, encolhe a janela, e SOLTA.

    A outra metade (repor 200x50 + `window-size latest` no desmonte) virou caso proprio logo
    abaixo, marcado `so_com_pty`, porque no Windows ela nao existe — ver o motivo la.
    """
    c = _client()
    assert _tam(sessao).startswith("200x50")
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        assert len(ws.receive_bytes()) > 0          # tmux pintou a tela
        _esperar(lambda: _tam_bate(sessao, 80, 24))          # encolheu, como desenhado
        assert sessao in termsock.clientes_ativos()
        assert _anexado(sessao)
        ws.close()
        # Esperar AINDA DENTRO do `with`, mesmo motivo do caso de baixo: o `__exit__` do
        # WebSocketTestSession so ENFILEIRA o disconnect e cancela o servidor em seguida.
        _esperar(lambda: not _anexado(sessao))
        assert termsock.clientes_ativos() == set()


@so_com_pty
def test_desmonte_repoe_o_tamanho_da_janela(sessao):
    """So no motor POSIX, e a ausencia do outro lado e MEDIDA, nao pendencia (22/08/2026):

    no psmux o tamanho da janela acompanha o cliente anexado no momento — o painel sai deixando
    80x23 gravado, o proximo cliente anexa a 200x50 e a janela vira 200x49 sozinha. Nao ha o que
    repor. E nem daria: `resize-window` e `setw window-size latest` voltam rc=0 la e nao fazem
    NADA. Escrever este caso pros dois seria exigir do Windows um passo que, se existisse, seria
    um par de comandos mentindo que funcionou.
    """
    c = _client()
    assert _tam(sessao).startswith("200x50")
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        assert len(ws.receive_bytes()) > 0
        time.sleep(1.0)
        assert _tam(sessao).startswith("80x24")
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


@com_painel
def test_segunda_conexao_derruba_a_primeira(sessao):
    url = f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24"
    # O unico caso do arquivo com DUAS conexoes vivas ao mesmo tempo, e por isso o unico que precisa
    # do laco compartilhado — ver `_um_laco_so`.
    with _um_laco_so(_client()) as c, c.websocket_connect(url) as a:
        a.receive_bytes()
        with c.websocket_connect(url) as b:
            # B tem que receber bytes DE VERDADE depois de derrubar A — nao so existir. Sem isso
            # o C3 (reader do fd reusado engolido) passava verde por sorte de numeracao de fd: a
            # contagem de "1 cliente" abaixo ja era verdade mesmo com o terminal do B mudo,
            # contanto que o tmux visse 1 attach (achado da revisao).
            assert len(b.receive_bytes()) > 0
            # `clientes_ativos()` e chaveado por NOME: contar 1 seria verdade mesmo sem derrubar
            # ninguem (achado do pass). Quem prova e o tmux: um cliente anexado, uma linha.
            # O `_anexado` entra junto porque a CONTAGEM sozinha nao distingue 1 de 0 no psmux —
            # com ninguem anexado o `list-clients` ainda devolve uma linha fantasma (ver `_anexado`).
            _esperar(lambda: _anexado(sessao) and len(_clientes(sessao).splitlines()) == 1)
            # E A tem que ter sido FECHADO de verdade (I1) — sem aviso, quem foi derrubado ficava
            # com o terminal congelado pra sempre em vez de ver a desconexao (achado da revisao).
            # Drena com PRAZO, nao com numero fixo de mensagens: pode sobrar no ar um pedaco
            # legitimo de tela que o PTY mandou pra A antes do close tomar efeito, e o ConPTY
            # entrega essa tela inicial em bem mais pedacos que o pty do Linux — um
            # `for _ in range(10)` esgotava as iteracoes ainda dentro do conteudo legitimo e nunca
            # alcancava o quadro de close (medido no Windows, 22/08/2026).
            fechou = False
            limite = time.monotonic() + 15.0
            while time.monotonic() < limite:
                try:
                    _receive_bytes_com_teto(a, segundos=2.0)
                except Exception:
                    fechou = True
                    break
            assert fechou, "a conexao derrubada nunca recebeu o aviso de fechamento (I1)"


@com_painel
def test_resize_chega_no_pty(sessao):
    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        ws.send_text(json.dumps({"t": "resize", "cols": 100, "rows": 30}))
        _esperar(lambda: _tam_bate(sessao, 100, 30))


@com_painel
def test_frame_de_controle_torto_nao_derruba_o_terminal(sessao):
    """Achado da revisao: JSON valido que nao e OBJETO (`5`, `null`, `[1,2]`) levantava
    AttributeError no `.get`, e `cols` com lista levantava TypeError no `int()` — nenhum dos dois
    estava no `except` do leitor, entao UM frame torto matava a task e fechava a CONEXAO inteira.
    Prova de verdade: depois da rajada de frames tortos, um resize VALIDO ainda chega no pty.
    """
    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        for torto in ("5", "null", "[1,2]", "isto nao e json",
                      json.dumps({"t": "resize", "cols": [1], "rows": 30}),
                      json.dumps({"t": "resize", "rows": 30})):
            ws.send_text(torto)
        ws.send_text(json.dumps({"t": "resize", "cols": 100, "rows": 30}))
        _esperar(lambda: _tam_bate(sessao, 100, 30))


@so_com_pty
def test_master_do_pty_nao_e_herdavel(sessao):
    """C1: `pty.fork()` (os.forkpty) NAO aplica o PEP 446 que `os.openpty`/`os.pipe` aplicam — o
    mestre nasce herdavel. Com o backend guardando uma Sessao viva por conexao, o `tmux attach` da
    conexao SEGUINTE herdava o fd do PTY da anterior: ler e injetar bytes no terminal alheio.
    """
    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        assert os.get_inheritable(termsock._ativos[sessao].master) is False


@so_com_pty
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
        # L26 da revisao final: procurar o marcador DENTRO de cada chunk falhava sem bug nenhum --
        # com `_LEITURA=64` ele cai numa fronteira ("PASSOU_DO" | "_TETO") e nenhuma mensagem
        # sozinha o contem. Acumula e procura no acumulado; o laco fecha por TEMPO (nao por
        # numero de mensagens), senao um teto de iteracoes vira flake do outro lado.
        acumulado = b""
        limite = time.monotonic() + 15.0
        while time.monotonic() < limite and b"PASSOU_DO_TETO" not in acumulado:
            try:
                acumulado += _receive_bytes_com_teto(ws, segundos=2.0)
            except Exception:
                break
        assert b"PASSOU_DO_TETO" in acumulado


@so_com_pty
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


@com_painel
def test_fechamento_feio_nao_deixa_zumbi_nem_trava_a_listagem(sessao):
    """Vale nos dois motores, por motivos diferentes — e e por isso que ele roda nos dois.

    No POSIX o risco e o `waitpid` bloqueante prendendo um worker do threadpool pra sempre; no
    Windows e o `WaitForSingleObject` do `ConPty.encerrar()`, que roda no mesmo `to_thread` e tem
    o mesmo prazo. A afirmacao final e a mesma nos dois: cinco desmontes feios e a listagem ainda
    responde.
    """
    c = _client()
    for _ in range(5):
        with c.websocket_connect(
                f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
            ws.receive_bytes()
            ws.close()          # sem handshake limpo
            # Espera DENTRO do with, mesmo motivo do teste anterior: o `__exit__` do
            # WebSocketTestSession cancela o servidor logo depois de enfileirar o disconnect, sem
            # esperar o `_desmontar` terminar.
            _esperar(lambda: not _anexado(sessao))
    # A regressao do threadpool: se as bombas de bytes tivessem sido to_thread, os workers ficariam
    # parqueados e este GET seria o que trava.
    r = c.get("/api/sessions", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    assert termsock.clientes_ativos() == set()
    assert not _anexado(sessao)


def test_selecionar_opcao_recusa_com_painel_aberto(sessao, monkeypatch):
    monkeypatch.setitem(termsock._ativos, sessao, object())
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/select",
               json={"option": 1}, headers={"Authorization": "Bearer secret"})
    assert r.status_code == 409
    assert r.json()["detail"]["code"] == "erro_terminal_aberto"


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
    # kill do shell fica pro teardown do fixture `sessao` (achado da revisao, I3): fazer aqui
    # some se o assert de cima falhar, e a sessao marcada @cp_hidden vaza invisivel no app.


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
    # kill do shell fica pro teardown do fixture `sessao` (mesmo motivo do teste acima, I3).


def test_shell_recusa_sequestrar_sessao_de_terceiro(sessao):
    # Achado da revisao (I1): "term-<nome>" pode ja existir como sessao de TERCEIRO --
    # `sanitize_session_name` aceita hifen, entao e alcancavel so pela UI (criar "foo", criar
    # "term-foo", abrir o shell de "foo"). Sem a checagem em `abrir_shell`, o `has_session` dentro
    # de `new_hidden_shell` pulava a criacao e marcava essa sessao ALHEIA como escondida -- ela
    # sumia da lista/board/canvas e a aba de shell anexava no terminal de outra sessao.
    alheia = f"term-{sessao}"
    matar_sessao(alheia)
    subprocess.run(["tmux", "new-session", "-d", "-s", alheia, "-x", "80", "-y", "24"], check=True)
    try:
        c = _client()
        r = c.post(f"/api/sessions/{sessao}/shell", headers={"Authorization": "Bearer secret"})
        assert r.status_code == 409
        # A sessao alheia continua VISIVEL -- nao foi sequestrada/marcada @cp_hidden.
        from app.registry import SessionRegistry
        assert alheia in [i.name for i in SessionRegistry().list()]
    finally:
        matar_sessao(alheia)


def test_shell_recusa_sequestrar_sessao_codex_de_terceiro(sessao):
    # Achado da revisao (rodada 2, "o mecanismo"): a 1a versao do I1 inferia colisao perguntando
    # "esse nome esta em registry.list()?" -- um proxy que amarra a checagem a bookkeeping do
    # registry que nao tem nada a ver com o /shell (a entrada de uma sessao Codex em list() vem
    # do SIDECAR, art. 766, nao de reconhecer o pane tmux; um dia em que o sidecar sumir orfao ou
    # mudar de forma, o proxy vale outra coisa). A pergunta DIRETA ao tmux (has_session + is_hidden)
    # nao depende de NADA disso -- so importa se a sessao "term-<nome>" existe e e nossa. Este
    # teste fixa o caso que motivou a troca: colisao com uma sessao que tambem tem sidecar Codex.
    from app.adapters.codex import sessions as codex_sessions
    alheia = f"term-{sessao}"
    matar_sessao(alheia)
    subprocess.run(["tmux", "new-session", "-d", "-s", alheia, "-x", "80", "-y", "24"], check=True)
    codex_sessions.save(alheia, "tid-fake", "/tmp/rollout-fake.jsonl", "/tmp")
    try:
        c = _client()
        r = c.post(f"/api/sessions/{sessao}/shell", headers={"Authorization": "Bearer secret"})
        assert r.status_code == 409
        # A sessao alheia continua VISIVEL e sem a marca -- nao foi sequestrada.
        from app import tmux as tmux_mod
        assert not tmux_mod.is_hidden(alheia)
    finally:
        codex_sessions.delete(alheia)
        matar_sessao(alheia)


def test_sessao_escondida_nao_muda_o_custo_da_listagem(monkeypatch, tmp_path):
    # Achado da revisao (I4): a versao original rodava contra o tmux DEFAULT e o ~/.claude/projects
    # de verdade, exigindo exatamente 1 `list-panes` — mas `resolve_tracked` chama
    # `_cwd_has_siblings` (um SEGUNDO `list-panes -a`) sempre que uma sessao real do dev resolve
    # por `--session-id` do cmdline sem marcador de hook nem fd. O teste passava por sorte do
    # estado da maquina. Mesmo padrao de test_registry_agent_pane.test_list_nao_faz_fork_por_sessao:
    # socket `-L` privado + sessao `sleep` pura (sem `--session-id` no cmdline, entao
    # `_cwd_has_siblings` nunca dispara) isola o teste do resto do tmux do usuario.
    import uuid
    from app import tmux as tmux_mod
    from app.registry import SessionRegistry
    sock = novo_socket()
    sess = f"cp-test-shell-custo-{uuid.uuid4().hex[:6]}"
    cwd = tmp_path
    chamadas: list = []
    try:
        subprocess.run(["tmux", "-L", sock, "new-session", "-d", "-s", sess, "-c", str(cwd),
                        "-x", "200", "-y", "50", "sleep 600"], check=True)

        def _espiao(args, **kw):
            chamadas.append(args)
            # `new_hidden_shell` pode prefixar com `_scope_prefix()` (systemd-run ... --), entao
            # "tmux" nao e necessariamente args[0] — acha o indice de verdade em vez de assumir
            # posicao fixa (achado do proprio teste: assumir args[0]=="tmux" deixava a criacao
            # escapar pro socket DEFAULT do usuario sempre que o probe do systemd-run desse certo).
            # Achado da revisao (rodada 2, Quebra 3): `_scope_prefix` chama `_scope_probe`, que roda
            # `_run([*_SCOPE, "true"])` -- SEM nenhum "tmux" no comando -- sempre que o cache de
            # processo `_scope_usavel` ainda esta `None`. Na suite cheia passava por sorte (algum
            # teste anterior ja tinha preenchido o cache global); isolado (`-k`, `--lf`, xdist,
            # ou este teste sozinho), `args.index("tmux")` levantava ValueError. Comandos sem
            # "tmux" nao sao desta chamada -- passam direto, sem redirecionar pro socket privado.
            if "tmux" not in args:
                return orig(args, **kw)
            i = args.index("tmux")
            real = [*args[:i + 1], "-L", sock, *args[i + 1:]]
            return orig(real, **kw)

        orig = tmux_mod.RUN
        monkeypatch.setattr(tmux_mod, "RUN", _espiao)

        alvo = tmux_mod.new_hidden_shell(sess, str(cwd))
        assert alvo is not None

        chamadas.clear()
        SessionRegistry(projects_dir=tmp_path).list()

        assert sum(1 for a in chamadas if "list-panes" in a) == 1
    finally:
        matar_sessao(sess, sock)
        matar_sessao(f"term-{sess}", sock)
        matar_servidor(sock)     # no psmux a ultima sessao nao leva o servidor junto


def test_open_terminal_sem_emulador_devolve_erro_visivel(sessao, monkeypatch):
    monkeypatch.delenv("CP_TERMINAL", raising=False)   # o codigo checa a env ANTES do PATH
    # `app.api.shutil` E o modulo `shutil` (nao ha copia por-modulo): este patch e GLOBAL durante
    # o teste, tambem alcancaria `tmux.py` se o codigo chegasse la. Inocuo aqui porque a rota sai
    # com 503 antes de tocar em `tmux._scope_prefix`/`_wayland_display`, mas o alcance real do
    # monkeypatch e o do MODULO, nao o de "so este import".
    monkeypatch.setattr("app.api.shutil.which", lambda _: None)
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/open-terminal", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 503
    assert r.json()["detail"]["code"] == "erro_terminal_ausente"


def test_open_terminal_detecta_emulador_que_morre_logo_apos_abrir(sessao, monkeypatch, tmp_path):
    # Achado da revisao (I5): o `Popen` so levanta se o BINARIO nao existe -- um emulador que
    # executa e morre logo depois (ex: sem DISPLAY/WAYLAND_DISPLAY, "cannot open display") saia
    # sozinho e a rota devolvia {"ok": true} pra uma janela que nunca abriu de verdade.
    script = _emulador_falso(tmp_path, "fake-term", exit_code=1, mensagem="cannot open display")
    import app.api as api_mod
    import app.tmux as tmux_mod
    monkeypatch.setattr(api_mod, "_EMULADORES", {"fake-term": lambda alvo: [str(script)]})
    monkeypatch.setattr(api_mod, "_ORDEM_PROBE", ["fake-term"])
    monkeypatch.setattr(api_mod.shutil, "which", lambda n: str(script) if n == "fake-term" else None)
    monkeypatch.setattr(tmux_mod, "_scope_prefix", lambda: [])   # fora do escopo deste teste (I5)
    monkeypatch.delenv("CP_TERMINAL", raising=False)
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/open-terminal", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 503
    d = r.json()["detail"]
    assert d["code"] == "erro_terminal_saiu_cedo"
    assert "display" in d["params"]["saida"].lower()   # stderr do emulador vai no params


def test_open_terminal_aceita_emulador_que_sai_0_logo_apos_abrir(sessao, monkeypatch, tmp_path):
    # Achado da revisao (rodada 2, Quebra 1): sair com rc=0 em poucos ms e comportamento NORMAL de
    # cliente D-Bus/instancia unica -- `gnome-terminal` (na sonda de verdade) abre no
    # `gnome-terminal-server` e sai 0 na hora; `wezterm start`/`konsole` com instancia ja de pe
    # fazem o mesmo. Tratar QUALQUER saida como erro (o teste com `exit 1` acima nao pega isso)
    # devolvia 503 pra uma janela que abriu certo.
    script = _emulador_falso(tmp_path, "fake-term-ok", exit_code=0)
    import app.api as api_mod
    import app.tmux as tmux_mod
    monkeypatch.setattr(api_mod, "_EMULADORES", {"fake-term-ok": lambda alvo: [str(script)]})
    monkeypatch.setattr(api_mod, "_ORDEM_PROBE", ["fake-term-ok"])
    monkeypatch.setattr(api_mod.shutil, "which",
                        lambda n: str(script) if n == "fake-term-ok" else None)
    monkeypatch.setattr(tmux_mod, "_scope_prefix", lambda: [])
    monkeypatch.delenv("CP_TERMINAL", raising=False)
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/open-terminal", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    assert r.json() == {"ok": True}


def test_kill_mata_o_shell_escondido_junto(sessao):
    # Achado da revisao (I2): a sessao de shell escondida nao tem afordancia na UI pra matar (e
    # escondida por construcao) -- sem limpar no kill, sobreviveria ao agente pra sempre.
    from app.registry import SessionRegistry
    c = _client()
    r = c.post(f"/api/sessions/{sessao}/shell", headers={"Authorization": "Bearer secret"})
    alvo = r.json()["shell"]
    assert subprocess.run(["tmux", "has-session", "-t", f"={alvo}"],
                          capture_output=True).returncode == 0
    SessionRegistry().kill(sessao)
    assert subprocess.run(["tmux", "has-session", "-t", f"={alvo}"],
                          capture_output=True).returncode != 0


def test_kill_nao_mata_sessao_de_terceiro_chamada_term_nome(sessao):
    # Achado da revisao (rodada 2, Quebra 2): o kill do I2 era incondicional -- existindo uma
    # sessao de VERDADE chamada "term-<nome>" (o mesmo cenario que o I1 reconheceu como
    # alcancavel pela UI), encerrar o agente pelo app derrubava ela JUNTO, com trabalho rodando e
    # so um `_log.debug` como registro. Agora so mata se a marca @cp_hidden confirmar que e nossa.
    from app.registry import SessionRegistry
    alheia = f"term-{sessao}"
    matar_sessao(alheia)
    subprocess.run(["tmux", "new-session", "-d", "-s", alheia, "-x", "80", "-y", "24"], check=True)
    try:
        SessionRegistry().kill(sessao)
        assert subprocess.run(["tmux", "has-session", "-t", f"={alheia}"],
                              capture_output=True).returncode == 0
    finally:
        matar_sessao(alheia)


def test_new_hidden_shell_mata_sessao_recem_criada_se_a_marca_falhar(sessao, monkeypatch):
    # Achado da revisao (rodada 2, Quebra 5): antes, o retorno do `set-option` era descartado --
    # se a marca falhasse (tmux ocupado, timeout de 5s do `_run`), sobrava um "term-<nome>" vivo e
    # VISIVEL (card nas tres views), e todo POST seguinte respondia 409 com um texto mentiroso (a
    # sessao nao e de terceiro, e nossa, so a marca falhou). Agora honra o rc: se a sessao acabou
    # de nascer AGORA e a marca falha, mata em vez de deixar o fantasma.
    from app import tmux as tmux_mod
    orig = tmux_mod.RUN

    def _falha_set_option(args, **kw):
        if "set-option" in args:
            return subprocess.CompletedProcess(args, 1, stdout="", stderr="tmux ocupado (fake)")
        return orig(args, **kw)

    monkeypatch.setattr(tmux_mod, "RUN", _falha_set_option)
    alvo = tmux_mod.new_hidden_shell(sessao, DIR_NEUTRO)
    assert alvo is None
    assert subprocess.run(["tmux", "has-session", "-t", f"=term-{sessao}"],
                          capture_output=True).returncode != 0


def _id_da_sessao(nome):
    # session_id (`$N`) muda quando a sessao e RECRIADA; o nome, nao. E a unica prova de "e a
    # mesma sessao" que sobrevive a um kill+new com o mesmo nome.
    return subprocess.run(["tmux", "display", "-p", "-t", f"={nome}:", "#{session_id}"],
                          capture_output=True, text=True).stdout.strip()


def test_shell_escondido_orfa_nao_reata_no_cwd_errado(sessao, tmp_path):
    """I3 da revisao final: `term-<nome>` sobrevive quando a sessao do agente morre FORA do app
    (`exit` no terminal, `tmux kill-session` na mao) -- o `_kill_hidden_shell` so roda pelo kill()
    daqui. Criar depois outra sessao com o MESMO nome noutro repo e abrir a aba Shell devolvia o
    shell do repo ANTIGO, rotulado com a sessao nova."""
    from app import tmux as tmux_mod
    alvo = tmux_mod.new_hidden_shell(sessao, DIR_NEUTRO)
    assert alvo == f"term-{sessao}"
    primeiro = _id_da_sessao(alvo)

    # Mesmo cwd -> REATA (idempotencia; nao pode matar o shell de ninguem a toa).
    assert tmux_mod.new_hidden_shell(sessao, DIR_NEUTRO) == alvo
    assert _id_da_sessao(alvo) == primeiro

    # cwd diferente (o nome foi reusado por outro repo) -> recria naquele diretorio.
    assert tmux_mod.new_hidden_shell(sessao, str(tmp_path)) == alvo
    assert _id_da_sessao(alvo) != primeiro
    caminho = subprocess.run(["tmux", "display", "-p", "-t", f"={alvo}:", "#{session_path}"],
                             capture_output=True, text=True).stdout.strip()
    assert os.path.normcase(caminho) == os.path.normcase(str(tmp_path))
    # kill do shell fica pro teardown do fixture `sessao`.


def test_rename_leva_o_shell_escondido_junto(sessao):
    """L71 da revisao final: o shell e keyed por NOME. Sem isto, renomear o agente deixava
    `term-<velho>` vivo pra sempre -- invisivel no app (marcado @cp_hidden) e fora do alcance do
    `kill()`, que so procura `term-<novo>`. Ramo FELIZ: o shell e RENOMEADO, nao morto -- o que
    estivesse rodando nele (um `npm run dev`) sobrevive, e o cwd nao muda com o rename."""
    from app import tmux as tmux_mod
    from app.registry import SessionRegistry
    assert tmux_mod.new_hidden_shell(sessao, DIR_NEUTRO) == f"term-{sessao}"
    antes = _id_da_sessao(f"term-{sessao}")
    novo = f"{sessao}-renomeada"
    try:
        assert tmux_mod.rename_session(sessao, novo) is True
        SessionRegistry().rename(sessao, novo)
        assert not tmux_mod.has_session(f"term-{sessao}")
        # MESMA sessao (o `$N` sobrevive ao rename; um kill+recria o mudaria), ainda marcada e no
        # mesmo diretorio -- e agora alcancavel pelo kill() do nome novo.
        assert _id_da_sessao(f"term-{novo}") == antes
        assert tmux_mod.is_hidden(f"term-{novo}")
        caminho = subprocess.run(["tmux", "display", "-p", "-t", f"=term-{novo}:",
                                  "#{session_path}"], capture_output=True, text=True).stdout.strip()
        assert os.path.normcase(caminho) == os.path.normcase(DIR_NEUTRO)
    finally:
        # A sessao mudou de nome -> o teardown do fixture (que mira o nome antigo) nao a alcanca.
        matar_sessao(novo)
        matar_sessao(f"term-{novo}")


def test_rename_mata_o_shell_quando_o_nome_novo_ja_esta_ocupado(sessao):
    """Ramo de FALLBACK do mesmo bloco: `term-<novo>` ja existe (shell de uma vida anterior daquele
    nome), o `rename-session` falha -- e deixar o velho vivo devolveria o orfa que o bloco existe
    pra evitar. Entao mata."""
    from app import tmux as tmux_mod
    from app.registry import SessionRegistry
    novo = f"{sessao}-renomeada"
    assert tmux_mod.new_hidden_shell(sessao, DIR_NEUTRO) == f"term-{sessao}"
    ocupante = tmux_mod.new_hidden_shell(novo, DIR_NEUTRO)   # ocupa `term-<novo>` de proposito
    assert ocupante == f"term-{novo}"
    ocupante_id = _id_da_sessao(ocupante)
    try:
        SessionRegistry().rename(sessao, novo)
        assert not tmux_mod.has_session(f"term-{sessao}")   # o velho saiu
        assert _id_da_sessao(f"term-{novo}") == ocupante_id  # e o ocupante NAO foi tocado
    finally:
        matar_sessao(f"term-{novo}")


def test_config_expoe_capacidade_do_painel_de_terminal():
    """A chave e CAPACIDADE ("da pra abrir painel aqui?"), nao nome de sistema.

    A afirmacao antiga era `== (os.name == "posix")`, que descrevia o gate de quando so havia
    motor POSIX — e virou MENTIRA no dia em que o ConPTY entrou (22/08/2026). Trocar por
    `== painel_disponivel()` seria tautologia (a rota devolve exatamente essa funcao); o que vale
    afirmar e o fato: nas duas plataformas em que este projeto roda existe motor, entao a chave e
    True nas duas, e continua sendo um booleano em `somente_leitura` porque o app nunca decide
    isto sozinho.
    """
    c = _client()
    r = c.get("/api/config", headers={"Authorization": "Bearer secret"})
    assert r.status_code == 200
    valor = r.json()["somente_leitura"]["terminal_panel"]
    assert isinstance(valor, bool)
    assert valor is True


def test_origem_mesma_do_host_e_aceita_mesmo_com_public_url_diferente(monkeypatch):
    # Regressao de producao: `public_url` aponta pro nome do Tailscale, mas o dono abre o app em
    # http://127.0.0.1:8765. Comparar SO com a public_url recusava a origem local com 403 e o painel
    # so funcionava pelo endereco publico. Passou por todas as revisoes porque na instancia de teste
    # a public_url estava VAZIA e a checagem inteira era pulada.
    from app import termsock as ts
    monkeypatch.setattr(ts.settings, "public_url", "https://notebook.tailnet.ts.net", raising=False)
    assert ts._origem_aceita("http://127.0.0.1:8765", "127.0.0.1:8765") is True
    assert ts._origem_aceita("http://192.168.15.28:8765", "192.168.15.28:8765") is True
    assert ts._origem_aceita("https://notebook.tailnet.ts.net", "127.0.0.1:8765") is True


def test_origem_de_terceiro_continua_recusada(monkeypatch):
    from app import termsock as ts
    monkeypatch.setattr(ts.settings, "public_url", "https://notebook.tailnet.ts.net", raising=False)
    # Sufixo colado no dominio legitimo: o `startswith` de antes deixava passar.
    assert ts._origem_aceita("https://notebook.tailnet.ts.net.evil.com", "127.0.0.1:8765") is False
    assert ts._origem_aceita("https://evil.com", "127.0.0.1:8765") is False


def test_sem_public_url_so_mesma_origem_passa(monkeypatch):
    from app import termsock as ts
    monkeypatch.setattr(ts.settings, "public_url", "", raising=False)
    assert ts._origem_aceita("http://127.0.0.1:8766", "127.0.0.1:8766") is True
    assert ts._origem_aceita("https://evil.com", "127.0.0.1:8766") is False


def test_origem_de_qualquer_peer_da_malha_e_aceita(monkeypatch):
    # O app e servido de UMA maquina e fala com VARIAS: o PWA do celular carrega de um host e
    # conversa com este backend por outro endereco. So mesma-origem + public_url recusaria o celular
    # com 403 — mesma classe do bug que a public_url sozinha causou no desktop.
    from app import termsock as ts
    monkeypatch.setattr(ts.settings, "public_url", "https://notebook.tailnet.ts.net", raising=False)
    monkeypatch.setattr(ts, "_peers_conhecidos",
                        lambda: ["http://100.64.0.2:8766", "https://win-x.tailnet.ts.net"])
    assert ts._origem_aceita("http://100.64.0.2:8766", "127.0.0.1:8765") is True
    assert ts._origem_aceita("https://win-x.tailnet.ts.net", "127.0.0.1:8765") is True
    assert ts._origem_aceita("https://outra-maquina.tailnet.ts.net", "127.0.0.1:8765") is False


def test_origem_extra_declarada_e_aceita(monkeypatch):
    # O front pode ser servido de uma maquina que NAO e peer nenhum (o PWA da VPS carrega de la e
    # fala com este backend pelo Tailscale): a Origin dele nao e mesma-origem, nao e a public_url e
    # nao esta no peers.json — o terminal do celular levava 403 no handshake. CP_TERM_ORIGINS e a
    # unica forma de declarar essa origem, e sem ela nada muda.
    from app import termsock as ts
    monkeypatch.setattr(ts.settings, "public_url", "https://notebook.tailnet.ts.net", raising=False)
    monkeypatch.setattr(ts, "_peers_conhecidos", lambda: [])
    monkeypatch.setattr(ts.settings, "term_origins",
                        "https://pocket.exemplo.com, http://127.0.0.1:5173", raising=False)
    assert ts._origem_aceita("https://pocket.exemplo.com", "127.0.0.1:8765") is True
    assert ts._origem_aceita("http://127.0.0.1:5173", "127.0.0.1:8765") is True
    # Declarar uma origem nao abre as outras — inclusive o dominio colado no legitimo.
    assert ts._origem_aceita("https://pocket.exemplo.com.evil.com", "127.0.0.1:8765") is False
    assert ts._origem_aceita("https://evil.com", "127.0.0.1:8765") is False


def test_sem_term_origins_nada_muda(monkeypatch):
    # Vazio (o default) nao pode virar "aceita qualquer um": o handshake tambem autentica pelo
    # cookie `cp_token`, entao origem arbitraria seria qualquer site abrindo um terminal na maquina.
    from app import termsock as ts
    monkeypatch.setattr(ts.settings, "public_url", "", raising=False)
    monkeypatch.setattr(ts, "_peers_conhecidos", lambda: [])
    monkeypatch.setattr(ts.settings, "term_origins", "", raising=False)
    assert ts._origem_aceita("https://qualquer.com", "127.0.0.1:8765") is False


def test_malha_ilegivel_nao_derruba_o_painel(monkeypatch):
    # peers.json ausente/corrompido: sobra mesma-origem + public_url, que ja cobrem a maquina local.
    from app import termsock as ts
    monkeypatch.setattr(ts.settings, "public_url", "", raising=False)
    def explode():
        raise OSError("peers.json ilegivel")
    monkeypatch.setattr(ts, "_peers_conhecidos", explode)
    try:
        ok = ts._origem_aceita("http://127.0.0.1:8765", "127.0.0.1:8765")
    except OSError:
        ok = "explodiu"
    assert ok is True, "mesma-origem tem que passar ANTES de tocar na malha"


# ===========================================================================================
# Contrapartes do MOTOR DO WINDOWS. Cada uma existe porque o caso POSIX equivalente afirma algo
# que la nao tem contraparte (fd herdavel, `waitpid`, `detach-client`), e nao porque o painel
# seja diferente — o que o usuario ve e o mesmo.
# ===========================================================================================
so_windows = pytest.mark.skipif(os.name != "nt", reason="motor de ConPTY (Windows)")


@so_windows
def test_win_conpty_nao_herda_o_stdio_do_backend(tmp_path):
    """A ARMADILHA de quem segue o exemplo oficial da Microsoft. Guarda de regressao do achado.

    Sem `STARTF_USESTDHANDLES`, o `CreateProcess` propaga os std handles do PAI pro filho: num
    backend rodando como servico (stdout indo pro log) o filho escreve NO LOG e o pseudoconsole
    renderiza tela VAZIA. O sintoma aponta pro lugar errado — dentro do filho o `mode con` ja diz
    o tamanho certo, ou seja, o attach estava correto o tempo todo. Limpar `HANDLE_FLAG_INHERIT`
    dos nossos std handles NAO resolve (medido); o que resolve e ligar o flag com os TRES NULOS.

    O caso roda o ConPTY num processo FILHO cujo stdout e um pipe nosso — que e exatamente a forma
    do backend em producao — e cobra as DUAS metades: a marca sai pelo ConPTY **e** nao sai pelo
    stdout. So a primeira metade passaria verde com o bug, porque um `cmd /c echo` escrevendo no
    stdout herdado tambem "funciona" — do ponto de vista de quem so olha o processo.
    """
    import sys

    # A pasta do `backend`, tirada do proprio modulo importado — nao do cwd, que o pytest pode
    # mudar entre invocacoes.
    raiz = os.path.dirname(os.path.dirname(os.path.abspath(termsock.__file__)))
    corpo = '''
import os, sys, time, _winapi
sys.path.insert(0, RAIZ)
from app import conpty
env = {k: v for k, v in os.environ.items() if k != "PSMUX_SESSION"}
p = conpty.abrir("cmd.exe /c echo MARCA_NO_CONPTY", 120, 30, env)
lido = b""
fim = time.monotonic() + 8.0
while time.monotonic() < fim and b"MARCA_NO_CONPTY" not in lido:
    try:
        d = _winapi.ReadFile(p.saida, 4096)[0]
    except OSError:
        break
    if not d:
        break
    lido += d
p.encerrar()
# O veredito vai pelo STDERR de proposito: o stdout deste processo e justamente a superficie
# que o teste inspeciona, e escrever nele aqui misturaria a prova com o que ela mede.
sys.stderr.write("CONPTY_VIU=%d" % (b"MARCA_NO_CONPTY" in lido))
'''.replace("RAIZ", repr(raiz))
    script = tmp_path / "roda_conpty.py"
    script.write_text(corpo, encoding="utf-8")
    r = subprocess.run([sys.executable, str(script)], capture_output=True, text=True, timeout=90)
    assert "CONPTY_VIU=1" in r.stderr, f"a marca nao saiu pelo ConPTY (stderr={r.stderr[:400]!r})"
    # A metade que pega o bug: com o stdio herdado, o `echo` do filho aterrissa AQUI.
    assert "MARCA_NO_CONPTY" not in r.stdout, (
        "o filho escreveu no stdout do processo pai — e o stdio herdado; num servico isso vai "
        f"pro log e o painel fica em branco (stdout={r.stdout[:400]!r})")


@so_windows
def test_win_attach_morto_fecha_o_socket(sessao):
    """Contraparte do `test_pty_morto_fecha_o_socket`.

    La o gatilho e `detach-client -s`, que no psmux nao serve: com o alvo exato ele responde
    `no session '=<nome>'` (rc=1) — o `=` nao e honrado por esse comando, mesma familia do
    `kill-session` —, e sem o `=` derrubaria TODOS os clientes da sessao, inclusive um `tmux
    attach` nativo do dono. Aqui o gatilho e matar o NOSSO processo de attach, que e o unico
    desmonte que aquele multiplexador permite com seguranca, e e o mesmo que o `_desmontar_windows`
    usa em producao.
    """
    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        termsock._ativos[sessao].pty.encerrar()
        with pytest.raises(Exception):
            for _ in range(50):
                _receive_bytes_com_teto(ws)


@so_windows
def test_win_saida_acima_do_teto_reata_a_leitura(sessao, monkeypatch):
    """Contraparte do `test_saida_acima_do_teto_reata_o_reader_depois_de_drenar`.

    A invariante e a mesma — quem PAUSA marca o flag, e o escritor so reata depois de drenar —, mas
    aqui a pausa e `transporte.pause_reading()` em vez de `remove_reader`, e a rajada tem que sair
    de um PowerShell (o pane do psmux nao e bash). Sem `_LEITURA`: no Windows quem decide o tamanho
    de cada leitura e o transporte do Proactor, nao a gente.
    """
    monkeypatch.setattr(termsock, "_SAIDA_MAX", 200)
    original = WebSocket.send_bytes

    async def _lento(self, data):
        await asyncio.sleep(0.03)
        await original(self, data)

    monkeypatch.setattr(WebSocket, "send_bytes", _lento)

    c = _client()
    with c.websocket_connect(f"/api/sessions/{sessao}/term?token=secret&cols=80&rows=24") as ws:
        ws.receive_bytes()
        subprocess.run(["tmux", "send-keys", "-t", f"={sessao}:", "-l",
                        "1..400 | ForEach-Object { 'XXXXXXXXXXXXXXXXXXXX' }"], check=True)
        subprocess.run(["tmux", "send-keys", "-t", f"={sessao}:", "Enter"], check=True)
        limite = time.monotonic() + 8.0
        while time.monotonic() < limite:
            try:
                _receive_bytes_com_teto(ws, segundos=0.5)
            except Exception:
                break
        # PROVA: a sessao ainda responde depois da rajada — a leitura foi reatada de verdade.
        subprocess.run(["tmux", "send-keys", "-t", f"={sessao}:", "-l", "echo PASSOU_DO_TETO"],
                       capture_output=True)
        subprocess.run(["tmux", "send-keys", "-t", f"={sessao}:", "Enter"], capture_output=True)
        acumulado = b""
        limite = time.monotonic() + 15.0
        while time.monotonic() < limite and b"PASSOU_DO_TETO" not in acumulado:
            try:
                acumulado += _receive_bytes_com_teto(ws, segundos=2.0)
            except Exception:
                break
        assert b"PASSOU_DO_TETO" in acumulado


# ===========================================================================================
# `ConPty.encerrar` com FALSOS — roda nos dois sistemas de proposito. O bug que estes casos
# guardam nao precisa de Windows pra existir: `TerminateProcess` e `restype = BOOL`, entao falha
# volta como 0 e nunca como excecao, e era um `except OSError` (codigo morto) que fingia trata-la.
# ===========================================================================================
class _FalsoPI:
    dwProcessId = 4242
    hProcess = 11
    hThread = 12


class _FalsoK32:
    def __init__(self, mata=True):
        self._mata = mata
        self.fechou_pseudoconsole = False

    def TerminateProcess(self, h, code):
        return 1 if self._mata else 0

    def ClosePseudoConsole(self, hpc):
        self.fechou_pseudoconsole = True


class _FalsoWinapi:
    WAIT_OBJECT_0 = 0

    def __init__(self, saiu=True):
        self._saiu = saiu

    def WaitForSingleObject(self, h, ms):
        return 0 if self._saiu else 0x102        # WAIT_TIMEOUT

    def CloseHandle(self, h):
        pass


def _conpty_falso(monkeypatch, *, mata, saiu):
    from app import conpty as mod
    k = _FalsoK32(mata=mata)
    monkeypatch.setattr(mod, "_k32", lambda: k)
    monkeypatch.setattr(mod, "_winapi", _FalsoWinapi(saiu=saiu), raising=False)
    monkeypatch.setattr(mod, "ctypes", types.SimpleNamespace(
        WinError=lambda e: OSError(e, "falso"), get_last_error=lambda: 5), raising=False)
    return mod.ConPty(hpc=1, pi=_FalsoPI(), saida=0, entrada=0), k


def test_conpty_encerrar_fecha_o_pseudoconsole_quando_o_filho_sai(monkeypatch):
    pty_, k = _conpty_falso(monkeypatch, mata=True, saiu=True)
    pty_.encerrar()
    assert k.fechou_pseudoconsole


def test_conpty_encerrar_e_silencioso_com_o_filho_ja_saido_sozinho(monkeypatch, caplog):
    """Fechar o painel depois de um `exit` e o caminho NORMAL, e nao pode virar aviso.

    Com o filho ja morto, `TerminateProcess` devolve 0 com ERROR_ACCESS_DENIED — medido 3 de 3 no
    Windows. Avisar ali punha um WARNING em todo fechamento de painel, e uma falha de verdade
    ficaria indistinguivel do ruido.
    """
    pty_, k = _conpty_falso(monkeypatch, mata=False, saiu=True)
    with caplog.at_level(logging.WARNING):
        pty_.encerrar()
    assert k.fechou_pseudoconsole
    assert caplog.text == ""


def test_conpty_encerrar_nao_fecha_o_pseudoconsole_com_o_filho_vivo(monkeypatch, caplog):
    """`ClosePseudoConsole` TRAVA esperando o cliente sair (microsoft/terminal#17716).

    Esta thread e um worker do `to_thread` do backend inteiro, entao pendurar aqui custa mais do
    que vazar um `conhost.exe`. No codigo velho, a falha do `TerminateProcess` passava batida
    (retorno ignorado, `except OSError` inalcancavel) e o fechamento acontecia assim mesmo.
    """
    pty_, k = _conpty_falso(monkeypatch, mata=False, saiu=False)
    with caplog.at_level(logging.WARNING):
        pty_.encerrar()
    assert not k.fechou_pseudoconsole
    assert "nao saiu em 3s" in caplog.text
    assert "TerminateProcess" in caplog.text      # o erro de matar vai junto, e util aqui
