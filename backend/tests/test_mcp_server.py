import contextlib
from pathlib import Path

import httpx2
import pytest
from mcp import ClientSession
from mcp.client.streamable_http import streamable_http_client

from app import mcp_server, quem_chama
from app.config import settings

pytestmark = pytest.mark.asyncio


@contextlib.asynccontextmanager
async def sessao_mcp(cabecalhos: dict[str, str]):
    settings.auth_token = "secret"
    async with mcp_server.lifespan():
        transporte = httpx2.ASGITransport(app=mcp_server.asgi)
        async with httpx2.AsyncClient(transport=transporte, base_url="http://127.0.0.1",
                                      headers={"Authorization": "Bearer secret", **cabecalhos}) as hc:
            async with streamable_http_client("http://127.0.0.1/", http_client=hc) as (r, w, *_):
                async with ClientSession(r, w) as s:
                    await s.initialize()
                    yield s


@pytest.fixture
def identidade(monkeypatch):
    monkeypatch.setattr(quem_chama, "resolver",
                        lambda h: ("eu", "pane") if h.get("x-hangar-pane") == "%3" else
                        (_ for _ in ()).throw(quem_chama.SessaoDesconhecida(quem_chama.DICA)))


async def test_sem_token_401_e_fora_da_maquina_403():
    settings.auth_token = "secret"
    async with mcp_server.lifespan():
        async with httpx2.AsyncClient(transport=httpx2.ASGITransport(app=mcp_server.asgi),
                                      base_url="http://127.0.0.1") as hc:
            assert (await hc.post("/", json={})).status_code == 401
        settings.auth_token = ""
        async with httpx2.AsyncClient(transport=httpx2.ASGITransport(app=mcp_server.asgi),
                                      base_url="http://127.0.0.1", headers={"Authorization": "Bearer "}) as hc:
            assert (await hc.post("/", json={})).status_code == 401
        settings.auth_token = "secret"
        async with httpx2.AsyncClient(transport=httpx2.ASGITransport(app=mcp_server.asgi, client=("10.0.0.5", 1)),
                                      base_url="http://127.0.0.1", headers={"Authorization": "Bearer secret"}) as hc:
            assert (await hc.post("/", json={})).status_code == 403


async def test_lista_tools_e_quem_sou(identidade):
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        nomes = {t.name for t in (await s.list_tools()).tools}
        assert nomes == {"who_am_i", "sessions", "send", "group", "pair", "unpair", "new_session",
                         "close_session", "browser_open", "browser", "browser_batch", "html_render"}
        res = await s.call_tool("who_am_i", {})
        assert not res.is_error and res.structured_content == {"name": "eu", "origem": "pane"}


async def test_nome_antigo_ainda_chama_mas_some_do_catalogo(identidade):
    """Sessão aberta ANTES do rename segue chamando pelo nome que carregou no catálogo dela.

    Ela não vai fechar só porque o servidor renomeou uma tool, e o catálogo dela é do momento da
    abertura. O nome velho continua chamável; o catálogo de quem abre agora não o mostra, senão
    toda sessão nova carregaria vinte tools pra sustentar dez.
    """
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        nomes = {t.name for t in (await s.list_tools()).tools}
        assert "quem_sou" not in nomes and "nav_abrir" not in nomes
        res = await s.call_tool("quem_sou", {})
        assert not res.is_error and res.structured_content == {"name": "eu", "origem": "pane"}


async def test_sem_identidade_e_erro_nao_cli(identidade):
    async with sessao_mcp({}) as s:
        res = await s.call_tool("quem_sou", {})
        assert res.is_error and "sessao_desconhecida" in res.content[0].text


async def test_enviar_prefixa_de_e_nao_recusa_caminho_nativo(identidade, monkeypatch):
    from app import api
    enviados = []

    async def input_prompt(name, body):
        enviados.append((name, body.text, body.steer))
        return {"delivered": True, "steered": True}

    uds = {"valor": None}
    monkeypatch.setattr(api, "input_prompt", input_prompt)
    monkeypatch.setattr(api, "peer_address", lambda name: _coro({"uds": uds["valor"]}))
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        res = await s.call_tool("enviar", {"alvo": "outra", "texto": "oi"})
        assert not res.is_error and res.structured_content["steered"] is True
        assert enviados == [("outra", "[de: eu] oi", True)]
        # Socket nativo nos dois lados: o backend escolhe o transporte, a tool nunca devolve o
        # envio pro modelo fazer por SendMessage.
        uds["valor"] = "/tmp/x.sock"
        res = await s.call_tool("enviar", {"alvo": "outra", "texto": "oi"})
        assert not res.is_error and len(enviados) == 2
        res = await s.call_tool("enviar", {"alvo": "outra", "texto": "oi", "tmux": True})
        assert not res.is_error and len(enviados) == 3
        res = await s.call_tool("enviar", {"alvo": "eu", "texto": "oi", "tmux": True})
        assert res.is_error and "é esta sessão" in res.content[0].text and len(enviados) == 3
        monkeypatch.setattr(settings, "server_id", "srv1")
        res = await s.call_tool("enviar", {"alvo": "srv1::eu", "texto": "oi"})
        assert res.is_error and "é esta sessão" in res.content[0].text and len(enviados) == 3


async def test_enviar_nao_dobra_o_prefixo_escrito_pelo_modelo(identidade, monkeypatch):
    from app import api, peers
    locais, remotos = [], []

    async def input_prompt(name, body):
        locais.append(body.text)
        return {"delivered": True}

    monkeypatch.setattr(api, "input_prompt", input_prompt)
    monkeypatch.setattr(peers, "call", lambda srv, metodo, rota, corpo: remotos.append(corpo["text"]) or (200, {}))
    monkeypatch.setattr(settings, "server_id", "srv1")
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        await s.call_tool("send", {"alvo": "outra", "texto": "[de: srv1::eu] oi"})
        await s.call_tool("send", {"alvo": "srv2::outra", "texto": "[de: eu] oi"})
        await s.call_tool("send", {"alvo": "srv2::outra", "texto": "[de: ::eu] oi"})
    assert locais == ["[de: eu] oi"]
    assert remotos == ["[de: srv1::eu] oi", "[de: srv1::eu] [de: ::eu] oi"]


async def test_sessoes_marca_a_propria(identidade, monkeypatch):
    from types import SimpleNamespace
    from app import api
    infos = [SimpleNamespace(name=n, state="idle", cwd="/x", provider="claude", headless=False,
                             pair_gid=g)
             for n, g in (("eu", "g1"), ("outra", None))]
    # Com a assinatura real da rota: o stub sem `request` escondeu a tool quebrada.
    monkeypatch.setattr(api, "list_sessions", lambda request: _coro(infos))
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        res = await s.call_tool("sessoes", {})
        assert {d["name"]: d["voce"] for d in res.structured_content["result"]} == {"eu": True, "outra": False}
        # Entrada e saída não são avisadas: o grupo se consulta aqui.
        assert {d["name"]: d["grupo"] for d in res.structured_content["result"]} == {"eu": "g1", "outra": None}
    async with sessao_mcp({}) as s:
        res = await s.call_tool("sessoes", {})
        assert not res.is_error
        assert not any(d["voce"] for d in res.structured_content["result"])


async def test_nav_sem_app_desktop_da_o_erro_do_cli(identidade, monkeypatch):
    def sem_shell(*a, **k):
        raise mcp_server.navshell.ShellIndisponivel("o app desktop não está aberto nesta máquina")
    monkeypatch.setattr(mcp_server.navshell, "verbo", sem_shell)
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        res = await s.call_tool("nav", {"verbo": "snapshot"})
        assert res.is_error and "app desktop não está aberto" in res.content[0].text
        res = await s.call_tool("nav", {"verbo": "voar"})
        assert res.is_error and "verbo desconhecido" in res.content[0].text


async def test_nav_shot_gera_caminho_e_aba_passa(identidade, monkeypatch, tmp_path):
    chamadas = []

    def verbo(name, v, args=None, aba=None):
        chamadas.append((name, v, args, aba))
        return f"ok: {v} {args[0] if args else ''}"

    monkeypatch.setattr(mcp_server.navshell, "verbo", verbo)
    monkeypatch.setattr(mcp_server.Path, "home", lambda: tmp_path)
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        res = await s.call_tool("nav", {"verbo": "shot", "aba": 2})
        assert not res.is_error
        nome, v, args, aba = chamadas[0]
        assert (nome, v, aba) == ("eu", "shot", 2)
        assert args[0].startswith(str(tmp_path / ".hangar" / "nav" / "shots" / "eu")) and args[0].endswith(".png")
        assert (tmp_path / ".hangar" / "nav" / "shots" / "eu").is_dir()


async def test_nav_layout_encaminha_tamanho_personalizado(identidade, monkeypatch):
    chamadas = []

    def verbo(name, v, args=None, aba=None):
        chamadas.append((name, v, args, aba))
        return "layout: 1366x768"

    monkeypatch.setattr(mcp_server.navshell, "verbo", verbo)
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        res = await s.call_tool("nav", {"verbo": "layout", "args": [1366, 768]})

    assert not res.is_error and res.content[0].text == "layout: 1366x768"
    assert chamadas == [("eu", "layout", ["1366", "768"], None)]


async def test_nav_lote_para_no_primeiro_erro(identidade, monkeypatch):
    def verbo(name, v, args=None, aba=None):
        return "erro: ref @e9 nao existe" if v == "click" else f"ok: {v}"

    monkeypatch.setattr(mcp_server.navshell, "verbo", verbo)
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        res = await s.call_tool("nav_lote", {"passos": [{"verbo": "snapshot"}, {"verbo": "click", "args": ["@e9"]},
                                                        {"verbo": "text"}]})
        assert res.is_error
        txt = res.content[0].text
        assert "parou no passo 1 (click)" in txt and "@e9" in txt and "ok: snapshot" in txt
        res = await s.call_tool("nav_lote", {"passos": [{"verbo": "snapshot"}, {"verbo": "text"}]})
        assert not res.is_error and res.structured_content == {"feitos": ["ok: snapshot", "ok: text"]}


async def test_browser_open_completa_pagina_propria_e_recusa_outro_caminho(identidade, monkeypatch):
    from app import api
    gravados = []
    monkeypatch.setattr(api, "_session_exists", lambda name: True)
    monkeypatch.setattr(api, "nav_pendente", lambda name, url: gravados.append((name, url)))
    monkeypatch.setattr(api, "resolve_bind_ip", lambda s: "0.0.0.0")
    monkeypatch.setattr(settings, "port", 8765)
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        # O Rust codifica o nome inteiro (`%65u` é `eu`).
        for url in ("/api/sessions/eu/pages/abc-1", "/api/sessions/%65u/pages/abc-1"):
            assert not (await s.call_tool("browser_open", {"url": url})).is_error
        for url in ("/api/sessions/outra/pages/abc-1", "/api/sessions/eu/files", "/api/sessions/eu/pages/../x"):
            res = await s.call_tool("browser_open", {"url": url})
            assert res.is_error and "página desta sessão" in res.content[0].text
    assert gravados == [("eu", "http://127.0.0.1:8765/api/sessions/eu/pages/abc-1?token=secret"),
                        ("eu", "http://127.0.0.1:8765/api/sessions/%65u/pages/abc-1?token=secret")]


async def test_grupo_parear_nova_sessao_chamam_as_rotas_como_eu(identidade, monkeypatch):
    from app import api
    from app.models import CreatedSessionInfo
    chamadas = {}

    async def group_message(name, body):
        chamadas["grupo"] = (name, body.text, body.forcar_tmux); return {"peers": ["x"], "pulados": []}

    async def pair_session(name, body):
        chamadas["parear"] = (name, body.peer, body.task); return {"ok": True}

    async def unpair_session(name):
        chamadas["desparear"] = name; return {"ok": True}

    async def create_session(body):
        # `config_dir` entra na tupla porque a conta da sessão nova é o contrato da tool: sem ele
        # aqui, criar na conta padrão em vez da de quem chama voltaria a passar no teste.
        chamadas["nova"] = (body.name, body.cwd, body.provider, body.headless, body.config_dir, body.creator)
        return CreatedSessionInfo(name=body.name, cwd=body.cwd, provider=body.provider,
                                  headless=body.headless, config_dir=body.config_dir)

    for n, f in (("group_message", group_message), ("pair_session", pair_session),
                 ("unpair_session", unpair_session), ("create_session", create_session)):
        monkeypatch.setattr(api, n, f)
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        assert not (await s.call_tool("grupo", {"texto": "marco"})).is_error
        assert not (await s.call_tool("parear", {"alvo": "outra", "tarefa": "t"})).is_error
        assert not (await s.call_tool("desparear", {})).is_error
        res = await s.call_tool("new_session", {"nome": "nova", "cwd": "/tmp", "provider": "codex",
                                                "headless": True, "conta": "/home/x/.claude-outra"})
        assert res.structured_content["name"] == "nova"
        # A conta usada volta na resposta: herdar errado calado foi o bug que isto fecha.
        assert res.structured_content["config_dir"] == "/home/x/.claude-outra"
        res = await s.call_tool("new_session", {"nome": "n2", "cwd": "/tmp", "provider": "pi", "headless": True})
        assert res.is_error and "headless só vale" in res.content[0].text
    assert chamadas == {"grupo": ("eu", "marco", False), "parear": ("eu", "outra", "t"), "desparear": "eu",
                        "nova": ("nova", "/tmp", "codex", True, "/home/x/.claude-outra", "eu")}


async def test_close_session_recusa_a_propria_remota_e_inexistente(identidade, monkeypatch):
    from app import api
    fechadas = []

    async def kill_session(name, by=None):
        fechadas.append(name); return {"ok": True, "warning": None}

    monkeypatch.setattr(api, "kill_session", kill_session)
    monkeypatch.setattr(api, "_transfer_check", lambda name: _coro(None))
    monkeypatch.setattr(quem_chama, "_por_nome", lambda nome: nome if nome == "outra" else None)
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        for alvo, erro in (("eu", "é esta sessão"), ("srv::outra", "desta máquina"), ("sumiu", "não existe")):
            res = await s.call_tool("close_session", {"alvo": alvo})
            assert res.is_error and erro in res.content[0].text
        assert not (await s.call_tool("close_session", {"alvo": "outra"})).is_error
    assert fechadas == ["outra"]


async def _coro(v):
    return v


@pytest.mark.parametrize("mode", [None, False, True])
async def test_new_session_preserves_mode_omission(identidade, monkeypatch, mode):
    from app import api
    from app.models import CreatedSessionInfo
    received = []
    async def create(body):
        received.append(body.headless)
        return CreatedSessionInfo(name=body.name, cwd=body.cwd, headless=bool(body.headless))
    monkeypatch.setattr(api, "create_session", create)
    args = {"nome": "test-mode", "cwd": "/tmp", "conta": "/tmp/test-account"}
    if mode is not None:
        args["headless"] = mode
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as session:
        result = await session.call_tool("new_session", args)
    assert not result.is_error
    assert received == [mode]


async def test_html_render_sends_own_theme_only_when_asked(identidade, monkeypatch):
    from app import pages_bridge
    sent = []
    monkeypatch.setattr(pages_bridge, "publish", lambda payload: sent.append(payload) or {"hangar_page": {"id": "a"}})
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        assert not (await s.call_tool("html_render", {"html": "<p></p>", "title": "t"})).is_error
        assert not (await s.call_tool("html_render", {"html": "<p></p>", "title": "t", "own_theme": True})).is_error
    assert "own_theme" not in sent[0] and sent[1]["own_theme"] is True


async def test_html_render_takes_html_or_url_never_both(identidade, monkeypatch):
    from app import pages_bridge
    sent = []
    monkeypatch.setattr(pages_bridge, "publish", lambda payload: sent.append(payload) or {"hangar_page": {"id": "a"}})
    async with sessao_mcp({"X-Hangar-Pane": "%3"}) as s:
        url = "http://localhost:3000/cidades"
        assert not (await s.call_tool("html_render", {"url": url, "title": "t"})).is_error
        assert (await s.call_tool("html_render", {"url": url, "html": "<p></p>", "title": "t"})).is_error
        assert (await s.call_tool("html_render", {"title": "t"})).is_error
    assert sent == [{"session": sent[0]["session"], "title": "t", "draft": False, "url": url}]
