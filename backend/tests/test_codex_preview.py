"""Testes do CodexPreviewSource: mesma interface publica do PreviewBroker (get/subscribe,
version + Condition + ref-count), mas alimentado por PUSH (deltas do app-server) em vez de
POLL do pane (Codex nao tem pane de tmux)."""
import asyncio

import pytest

from app.adapters.codex.preview import CodexPreviewSource


@pytest.mark.asyncio
async def test_push_before_subscribe_is_seen_as_current_text():
    # Mesma semantica do PreviewBroker: subscribe() sempre entrega o snapshot ATUAL no 1o yield
    # (version=0 != last=-1 dispara sem esperar notify), sem precisar de push posterior.
    name = "push-basic"
    src = CodexPreviewSource.get(name)
    await src.push("ok")
    text = await asyncio.wait_for(src.subscribe().__anext__(), timeout=1)
    assert text == "ok"


@pytest.mark.asyncio
async def test_push_wakes_a_subscriber_already_waiting():
    name = "push-wake"
    src = CodexPreviewSource.get(name)
    agen = src.subscribe()
    await agen.__anext__()  # consome o snapshot inicial ("") -> subscriber passa a esperar mudanca
    task = asyncio.create_task(agen.__anext__())
    await asyncio.sleep(0.01)  # deixa o subscriber entrar no wait_for antes do push
    await src.push("ok")
    text = await asyncio.wait_for(task, timeout=1)
    assert text == "ok"
    await agen.aclose()


@pytest.mark.asyncio
async def test_two_pushes_coalesce_to_last_for_slow_subscriber():
    name = "push-coalesce"
    src = CodexPreviewSource.get(name)
    agen = src.subscribe()

    await src.push("o")
    await src.push("ok")  # subscriber ainda nao leu nada -> so ve o ultimo (full-replace)

    text = await asyncio.wait_for(agen.__anext__(), timeout=1)
    assert text == "ok"
    await agen.aclose()


@pytest.mark.asyncio
async def test_get_returns_same_instance_for_same_name():
    name = "push-samename"
    assert CodexPreviewSource.get(name) is CodexPreviewSource.get(name)


@pytest.mark.asyncio
async def test_last_subscriber_leaving_drops_the_instance():
    name = "push-refcount"
    src = CodexPreviewSource.get(name)
    agen = src.subscribe()
    await agen.__anext__()  # entra no generator (roda o try, incrementa _subs)
    await agen.aclose()     # dispara o finally -> _subs volta a 0 -> registry limpa
    assert name not in CodexPreviewSource._sources
