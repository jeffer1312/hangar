"""Testes do AppServerClient: framing NDJSON + correlacao request<->response por id, via
transporte fake em memoria (sem spawnar o binario `codex` real, ver docs/codex-app-server-contract.md)."""
import asyncio
import json
import os
import shutil

import pytest

from app.adapters.codex.appserver import AppServerClient


class _FakeWriter:
    """Escreve em memoria - substitui asyncio.StreamWriter (que exige um transport real de
    verdade pra existir). So guarda o que foi escrito pra o teste inspecionar."""

    def __init__(self) -> None:
        self.lines: list[bytes] = []
        self.closed = False

    def write(self, data: bytes) -> None:
        self.lines.append(data)

    async def drain(self) -> None:
        pass

    def close(self) -> None:
        self.closed = True

    async def wait_closed(self) -> None:
        pass


def _fake_reader(*chunks: bytes) -> asyncio.StreamReader:
    """asyncio.StreamReader alimentado manualmente com feed_data/feed_eof - stub padrao da
    stdlib pra simular stdout do subprocess sem precisar de pipe/socket real."""
    reader = asyncio.StreamReader()
    for chunk in chunks:
        reader.feed_data(chunk)
    return reader


def _client_with(reader: asyncio.StreamReader, writer: _FakeWriter) -> AppServerClient:
    client = AppServerClient()
    client._attach(reader, writer)  # seam de teste: injeta transporte sem spawnar subprocess
    return client


async def test_request_writes_ndjson_line_and_resolves_by_id():
    writer = _FakeWriter()
    reader = _fake_reader()  # sem EOF ainda: mantem a leitura viva ate a resposta chegar
    client = _client_with(reader, writer)

    req_task = asyncio.create_task(client.request("m", {}))
    await asyncio.sleep(0)  # deixa o write acontecer antes de alimentar a resposta

    assert len(writer.lines) == 1
    line = writer.lines[0]
    assert line.endswith(b"\n")
    assert json.loads(line) == {"jsonrpc": "2.0", "id": 1, "method": "m", "params": {}}

    reader.feed_data(b'{"jsonrpc":"2.0","id":1,"result":{"ok":true}}\n')
    result = await req_task
    assert result == {"ok": True}

    await client.close()


async def test_message_without_id_goes_to_notifications_not_request():
    writer = _FakeWriter()
    reader = _fake_reader(
        b'{"jsonrpc":"2.0","method":"remoteControl/status/changed","params":{"x":1}}\n'
    )
    client = _client_with(reader, writer)

    notif = await asyncio.wait_for(client.notifications().__anext__(), timeout=1)
    assert notif == {"jsonrpc": "2.0", "method": "remoteControl/status/changed", "params": {"x": 1}}

    await client.close()


async def test_pending_request_rejected_on_stream_eof():
    """Sem Future orfao: se o stream fecha antes da resposta chegar, a request pendente
    precisa destravar com erro, nao ficar pendurada pra sempre."""
    writer = _FakeWriter()
    reader = _fake_reader()
    client = _client_with(reader, writer)

    req_task = asyncio.create_task(client.request("m", {}))
    await asyncio.sleep(0)
    reader.feed_eof()

    with pytest.raises(ConnectionError):
        await req_task

    await client.close()


@pytest.mark.integration
@pytest.mark.skipif(
    os.environ.get("CP_CODEX_INTEGRATION") != "1" or shutil.which("codex") is None,
    reason="smoke manual: requer CP_CODEX_INTEGRATION=1 e `codex` logado no PATH",
)
async def test_real_codex_initialize_smoke():
    client = AppServerClient()
    await client.start()
    try:
        result = await client.request("initialize", {
            "clientInfo": {"name": "claude-pocket-test", "title": None, "version": "0.0.1"},
            "capabilities": None,
        })
        assert result
    finally:
        await client.close()
