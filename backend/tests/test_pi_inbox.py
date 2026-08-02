import asyncio
import json
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


def test_endpoint_ilegivel_nao_derruba_a_subida(monkeypatch):
    """Disco cheio / diretório só-leitura vira log, nunca exceção na subida do backend."""
    from app import pi_inbox

    monkeypatch.setattr("app.config.list_config_dirs",
                        lambda: [ConfigDirInfo(path="/proc/nao-da-pra-escrever", label="X", active=True)])
    assert pi_inbox.escrever_endpoint() == []
