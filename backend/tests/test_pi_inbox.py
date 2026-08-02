import asyncio
import json
import os
import tempfile
import threading

from app.config import ConfigDirInfo
from app.pi_inbox import PiInbox


def _falsa():
    """Extensão de mentira: guarda o que recebeu e deixa o teste confirmar na mão."""
    recebidas = []

    async def envia(payload: dict) -> None:
        recebidas.append(payload)

    return recebidas, envia


async def _confirmar_quando_chegar(inbox, pane, recebidas, ok=True, erro=None, quantas=1):
    vistos = 0
    for _ in range(300):
        if len(recebidas) > vistos:
            inbox.confirmar(pane, recebidas[vistos]["id"], ok, erro)
            vistos += 1
            if vistos >= quantas:
                return
        await asyncio.sleep(0.005)


async def test_entrega_confirmada_vira_sent():
    inbox = PiInbox()
    recebidas, envia = _falsa()
    inbox.registrar("%1", envia)
    tarefa = asyncio.create_task(_confirmar_quando_chegar(inbox, "%1", recebidas))
    assert await inbox.entregar("%1", "oi") == "sent"
    await tarefa
    assert recebidas[0]["text"] == "oi"
    assert recebidas[0]["deliverAs"] == "steer"


async def test_sem_linha_nao_inventa_entrega():
    """`sem-linha` é o ÚNICO retorno que autoriza o chamador a cair pro caminho de tecla."""
    inbox = PiInbox()
    assert await inbox.entregar("%404", "oi") == "sem-linha"


async def test_recusa_da_extensao_vira_deferred():
    """`sendUserMessage` levanta erro quando a sessão está streamando sem deliverAs. A mensagem
    tem que ficar pendente, nunca sumir."""
    inbox = PiInbox()
    recebidas, envia = _falsa()
    inbox.registrar("%1", envia)
    tarefa = asyncio.create_task(
        _confirmar_quando_chegar(inbox, "%1", recebidas, ok=False, erro="Agent is already processing"))
    assert await inbox.entregar("%1", "oi") == "deferred"
    await tarefa


async def test_sem_confirmacao_no_prazo_vira_deferred(monkeypatch):
    """NUNCA 'sent' por otimismo, e NUNCA cai pra tecla: quem chamou precisa saber que houve
    tentativa, senão digitaria por cima e duplicaria."""
    inbox = PiInbox()
    monkeypatch.setattr("app.pi_inbox.PRAZO_ACK", 0.05)
    _, envia = _falsa()
    inbox.registrar("%1", envia)
    assert await inbox.entregar("%1", "oi") == "deferred"


async def test_segunda_linha_no_mesmo_pane_derruba_a_primeira():
    """Processo zumbi no mesmo pane: entregar pra ele é entregar pro nada."""
    inbox = PiInbox()
    _, envia1 = _falsa()
    recebidas2, envia2 = _falsa()
    inbox.registrar("%1", envia1)
    inbox.registrar("%1", envia2)
    tarefa = asyncio.create_task(_confirmar_quando_chegar(inbox, "%1", recebidas2))
    assert await inbox.entregar("%1", "oi") == "sent"
    await tarefa
    assert len(recebidas2) == 1


async def test_envio_que_explode_nao_derruba_o_chamador():
    """Falha da linha é falha de uma feature; o núcleo de entrega nunca quebra por isso."""
    inbox = PiInbox()

    async def envia(payload):
        raise OSError("socket fechou")

    inbox.registrar("%1", envia)
    assert await inbox.entregar("%1", "oi") == "deferred"
    assert inbox.tem_linha("%1") is False, "linha morta tem que sair do registro"


async def test_duas_mensagens_no_mesmo_pane_nao_se_cruzam():
    """Serializa por sessão, igual ao _send_lock do caminho de tecla."""
    inbox = PiInbox()
    recebidas, envia = _falsa()
    inbox.registrar("%1", envia)
    tarefa = asyncio.create_task(_confirmar_quando_chegar(inbox, "%1", recebidas, quantas=2))
    r = await asyncio.gather(inbox.entregar("%1", "um"), inbox.entregar("%1", "dois"))
    await tarefa
    assert r == ["sent", "sent"]
    assert [x["text"] for x in recebidas] == ["um", "dois"]


async def test_entregar_usa_o_msg_id_recebido_em_vez_de_gerar_um_novo():
    """Id ESTAVEL entre reentregas (achado ALTA da revisao 02/08/2026 — "Porta A"): quem tem uma
    entrada de fila (retry) passa o proprio id, e a extensao usa ELE pra reconhecer o retry (dedupe
    em cp-state.ts). Sem isto, cada tentativa gerava um uuid4 novo e a extensao nunca via duas vezes
    o MESMO id."""
    inbox = PiInbox()
    recebidas, envia = _falsa()
    inbox.registrar("%1", envia)
    tarefa = asyncio.create_task(_confirmar_quando_chegar(inbox, "%1", recebidas))
    assert await inbox.entregar("%1", "oi", msg_id="fixo-123") == "sent"
    await tarefa
    assert recebidas[0]["id"] == "fixo-123"


async def test_entregar_sem_msg_id_ainda_gera_um_uuid_por_tentativa():
    """Sem id estavel a oferecer (nenhuma PromptQueue por perto — ver risco no relatorio), o
    comportamento de sempre continua: um uuid4 por chamada, so serve pra casar pedido/resposta
    DENTRO desta tentativa."""
    inbox = PiInbox()
    recebidas, envia = _falsa()
    inbox.registrar("%1", envia)
    tarefa = asyncio.create_task(_confirmar_quando_chegar(inbox, "%1", recebidas))
    assert await inbox.entregar("%1", "oi") == "sent"
    await tarefa
    assert recebidas[0]["id"]   # tem algum id (uuid4), so nao e None/vazio


def test_entregar_sync_repassa_o_msg_id_estavel():
    """A ponte sincrona repassa o msg_id sem alterar — mesmo id que chega na extensao."""
    inbox = PiInbox()
    resultado = {}

    async def principal():
        inbox.ligar_loop(asyncio.get_running_loop())
        recebidas, envia = _falsa()
        inbox.registrar("%1", envia)
        tarefa = asyncio.create_task(_confirmar_quando_chegar(inbox, "%1", recebidas))
        resultado["r"] = await asyncio.to_thread(inbox.entregar_sync, "%1", "oi", "fixo-999")
        await tarefa
        resultado["id"] = recebidas[0]["id"]

    asyncio.run(principal())
    assert resultado["r"] == "sent"
    assert resultado["id"] == "fixo-999"


def test_entregar_sync_atravessa_do_mundo_de_thread():
    """O send_prompt é SÍNCRONO e roda em thread; a linha vive no loop do servidor. Sem esta ponte
    o código nem roda — é o detalhe que trava quem implementa."""
    inbox = PiInbox()
    pronto = threading.Event()
    resultado = {}

    async def principal():
        inbox.ligar_loop(asyncio.get_running_loop())
        recebidas, envia = _falsa()
        inbox.registrar("%1", envia)

        def de_outra_thread():
            pronto.wait(2)
            resultado["r"] = inbox.entregar_sync("%1", "oi")

        t = threading.Thread(target=de_outra_thread)
        t.start()
        pronto.set()
        await _confirmar_quando_chegar(inbox, "%1", recebidas)
        await asyncio.to_thread(t.join, 5)

    asyncio.run(principal())
    assert resultado["r"] == "sent"


def test_entregar_sync_sem_loop_nao_explode():
    """Backend sem loop ligado (script, teste) não pode virar exceção no caminho de entrega."""
    inbox = PiInbox()
    assert inbox.entregar_sync("%1", "oi") == "sem-linha"


def test_entregar_sync_cancela_a_corrotina_no_timeout(monkeypatch):
    """Achado da revisão final: sem o cancel(), a corrotina do `entregar` segue viva no loop e pode
    confirmar DEPOIS de o chamador já ter decidido `deferred` — a fila reenvia pela mesma linha e a
    mesma instrução chega duas vezes ao agente. Mocka `run_coroutine_threadsafe` pra não depender de
    tempo real (o teto de `entregar_sync` é PRAZO_ACK + 2.0, fixo no código)."""
    import concurrent.futures
    from app import pi_inbox as pi_inbox_mod

    class FalsoFuturoCruzado:
        def __init__(self, coro):
            coro.close()   # nunca agendada de verdade — evita "coroutine was never awaited"
            self.cancelado = False

        def result(self, timeout):
            raise concurrent.futures.TimeoutError()

        def cancel(self):
            self.cancelado = True

    capturado = {}

    def falso_run_coroutine_threadsafe(coro, loop):
        capturado["fut"] = FalsoFuturoCruzado(coro)
        return capturado["fut"]

    monkeypatch.setattr(pi_inbox_mod.asyncio, "run_coroutine_threadsafe",
                        falso_run_coroutine_threadsafe)
    inbox = PiInbox()
    inbox.ligar_loop(object())   # so precisa ser != None pro guard de entregar_sync
    inbox.registrar("%1", lambda payload: None)
    assert inbox.entregar_sync("%1", "oi") == "deferred"
    assert capturado["fut"].cancelado is True


def test_entregar_sync_sem_futuro_nao_explode(monkeypatch):
    """Achado da re-revisão final: se o PRÓPRIO run_coroutine_threadsafe levantar (loop fechou
    entre o guard de `loop is None` e a chamada — corrida real de restart/shutdown), `fut` nunca
    chega a existir. Sem o guard `if fut is not None`, o `fut.cancel()` do except estourava
    AttributeError e escapava de `entregar_sync`, quebrando o contrato "nunca levanta" que o
    broadcast depende (terminal_input.py/api.py não recapturam Exception genérica)."""
    from app import pi_inbox as pi_inbox_mod

    def explode(coro, loop):
        coro.close()   # nunca agendada — evita "coroutine was never awaited"
        raise RuntimeError("loop fechado")

    monkeypatch.setattr(pi_inbox_mod.asyncio, "run_coroutine_threadsafe", explode)
    inbox = PiInbox()
    inbox.ligar_loop(object())
    inbox.registrar("%1", lambda payload: None)
    assert inbox.entregar_sync("%1", "oi") == "deferred"


def test_endpoint_vai_pra_todos_os_config_dirs(tmp_path, monkeypatch):
    """Sessão de worktree com CLAUDE_CONFIG_DIR próprio precisa achar o arquivo — senão fica no
    fallback de tecla pra sempre, calada. Mesmo problema que o hook_installer.py:153 já resolve."""
    from app import pi_inbox

    a, b = tmp_path / "A", tmp_path / "B"
    a.mkdir()
    b.mkdir()
    monkeypatch.setattr("app.config.list_config_dirs",
                        lambda: [ConfigDirInfo(path=str(a), label="A", active=True),
                                 ConfigDirInfo(path=str(b), label="B", active=False)])
    escritos = pi_inbox.escrever_endpoint()
    assert len(escritos) == 2
    d = json.loads((a / ".claude-pocket-conn.json").read_text(encoding="utf-8"))
    assert d["url"].startswith("ws://127.0.0.1:")
    assert (a / ".claude-pocket-conn.json").stat().st_mode & 0o777 == 0o600, "tem token dentro"


def test_endpoint_tmp_nunca_nasce_com_permissao_frouxa(tmp_path, monkeypatch):
    """mkstemp cria o arquivo JÁ em 0600 (é o open() com O_EXCL que fixa o modo na criação, não
    um chmod depois) — nunca existe instante com o token num arquivo de modo mais frouxo.
    Espiona o fd que o mkstemp devolve e confere o modo ANTES de qualquer escrita: é
    determinístico porque o modo é fixado no ato da criação, não uma corrida a torcer pra
    flagrar."""
    from app import pi_inbox

    a = tmp_path / "A"
    a.mkdir()
    modos = []
    mkstemp_original = tempfile.mkstemp

    def espiao(*args, **kwargs):
        fd, nome = mkstemp_original(*args, **kwargs)
        modos.append(os.fstat(fd).st_mode & 0o777)
        return fd, nome

    monkeypatch.setattr(pi_inbox.tempfile, "mkstemp", espiao)
    monkeypatch.setattr("app.config.list_config_dirs",
                        lambda: [ConfigDirInfo(path=str(a), label="A", active=True)])
    pi_inbox.escrever_endpoint()
    assert modos == [0o600]


def test_endpoint_usa_o_bind_real_do_backend(tmp_path, monkeypatch):
    """Achado da revisão final: o uvicorn escuta em resolve_bind_ip(settings) (main.py), não em
    127.0.0.1 fixo. Com CP_LAN_BIND_IP=auto (modo celular documentado) ou IP fixo de LAN, o bind
    NÃO é loopback — apontar o sidecar pra 127.0.0.1 faria a extensão ser recusada em silêncio
    pra sempre."""
    from app import pi_inbox

    a = tmp_path / "A"
    a.mkdir()
    monkeypatch.setattr("app.config.list_config_dirs",
                        lambda: [ConfigDirInfo(path=str(a), label="A", active=True)])
    monkeypatch.setattr("app.config.resolve_bind_ip", lambda s: "192.168.1.50")
    pi_inbox.escrever_endpoint()
    d = json.loads((a / ".claude-pocket-conn.json").read_text(encoding="utf-8"))
    assert d["url"].startswith("ws://192.168.1.50:")


def test_endpoint_bind_0000_ainda_aponta_pro_loopback(tmp_path, monkeypatch):
    """0.0.0.0 escuta em TODA interface, incl. loopback — 127.0.0.1 continua um destino válido (e
    o único que dá pra usar: um socket cliente não conecta em 0.0.0.0)."""
    from app import pi_inbox

    a = tmp_path / "A"
    a.mkdir()
    monkeypatch.setattr("app.config.list_config_dirs",
                        lambda: [ConfigDirInfo(path=str(a), label="A", active=True)])
    monkeypatch.setattr("app.config.resolve_bind_ip", lambda s: "0.0.0.0")
    pi_inbox.escrever_endpoint()
    d = json.loads((a / ".claude-pocket-conn.json").read_text(encoding="utf-8"))
    assert d["url"].startswith("ws://127.0.0.1:")


def test_endpoint_ilegivel_nao_derruba_a_subida(monkeypatch):
    """Disco cheio / diretório só-leitura vira log, nunca exceção na subida do backend."""
    from app import pi_inbox

    monkeypatch.setattr("app.config.list_config_dirs",
                        lambda: [ConfigDirInfo(path="/proc/nao-da-pra-escrever", label="X", active=True)])
    assert pi_inbox.escrever_endpoint() == []


def test_endpoint_falha_generica_antes_do_loop_nao_derruba_a_subida(monkeypatch, caplog):
    """Achado MEDIA da revisao 02/08/2026: escrever_endpoint roda na SUBIDA (main.py:91), logo depois
    de dois hooks explicitamente "idempotente, fail-soft". Ate aqui so o OSError por diretorio (loop
    abaixo) era pego — qualquer excecao de list_config_dirs()/resolve_bind_ip() ANTES do loop subia
    CRUA e derrubava a subida INTEIRA do backend por causa de um sidecar auxiliar. Fail-soft de
    verdade: loga e devolve vazio, como os hooks vizinhos."""
    from app import pi_inbox

    def explode():
        raise RuntimeError("HOME ausente")

    monkeypatch.setattr("app.config.list_config_dirs", explode)
    with caplog.at_level("WARNING", logger="app.pi_inbox"):
        assert pi_inbox.escrever_endpoint() == []
    assert "nao consegui preparar" in caplog.text
