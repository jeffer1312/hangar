"""Servidor MCP do Hangar: as operações do `hangar-send` como tools, montadas no backend em /mcp.

Quem chama se identifica pelos cabeçalhos X-Hangar-* (app.quem_chama); o token é o mesmo bearer
do backend, conferido ANTES do sub-app porque `app.mount()` passa por fora do `Depends`.
Cada tool chama a mesma função de rota que o CLI chama por HTTP — nada de funcionalidade só do MCP.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import re
import secrets
import time
from pathlib import Path
from typing import Any

from fastapi import HTTPException
from mcp.server import MCPServer
from mcp.server.mcpserver import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp.server.transport_security import TransportSecuritySettings

from app import auth as auth_mod
from app import navshell, peers, quem_chama
from app.config import settings

mcp = MCPServer("hangar")

# Nomes antigos das tools (português) → nome de hoje. Uma sessão já aberta carregou o catálogo na
# abertura e vai chamar pelo nome que conhece até ser reiniciada — e sessão de trabalho não fecha
# porque o servidor renomeou uma tool. Aqui o nome velho continua CHAMÁVEL sem aparecer no
# catálogo: quem abre agora vê só os dez nomes novos, e quem está no meio de uma tarefa não quebra.
#
# Some quando não houver mais sessão viva de antes do rename — é ponte, não API.
_NOMES_ANTIGOS = {
    "quem_sou": "who_am_i",
    "sessoes": "sessions",
    "enviar": "send",
    "grupo": "group",
    "parear": "pair",
    "desparear": "unpair",
    "nova_sessao": "new_session",
    "nav_abrir": "browser_open",
    "nav": "browser",
    "nav_lote": "browser_batch",
}

_chamar_tool = mcp.call_tool


async def _call_tool_com_nome_antigo(name, arguments, context=None, **kw):
    # Tradução ANTES do despacho: o handler do protocolo chama `self.call_tool(params.name, ...)`,
    # então é aqui que o nome velho vira o novo sem duplicar nada no catálogo.
    return await _chamar_tool(_NOMES_ANTIGOS.get(name, name), arguments, context, **kw)


mcp.call_tool = _call_tool_com_nome_antigo


def _cabecalhos(ctx: Context) -> dict[str, str]:
    req = getattr(ctx.request_context, "request", None)
    return dict(req.headers) if req is not None else {}


async def _eu(ctx: Context) -> str:
    try:
        nome, _ = await asyncio.to_thread(quem_chama.resolver, _cabecalhos(ctx))
    except quem_chama.SessaoDesconhecida as e:
        raise ToolError(f"sessao_desconhecida: {e}") from e
    return nome


def _detalhe(e: HTTPException) -> str:
    d = e.detail
    return d.get("msg", str(d)) if isinstance(d, dict) else str(d)


@mcp.tool(description="Quem é esta sessão no Hangar (nome e por qual cabeçalho foi resolvida). "
                      "Diagnóstico; equivale a `hangar-send` descobrindo a própria sessão.")
async def who_am_i(ctx: Context) -> dict[str, str]:
    try:
        nome, origem = await asyncio.to_thread(quem_chama.resolver, _cabecalhos(ctx))
    except quem_chama.SessaoDesconhecida as e:
        raise ToolError(f"sessao_desconhecida: {e}") from e
    return {"name": nome, "origem": origem}


@mcp.tool(description="Lista as sessões vivas nesta máquina (nome, estado, cwd, provider). "
                      "`voce: true` marca esta sessão. "
                      "Equivale a `hangar-send --list` sem os servidores remotos.")
async def sessions(ctx: Context) -> list[dict[str, Any]]:
    from app import api
    try:
        eu = await _eu(ctx)
    except ToolError:
        eu = None
    infos = await api.list_sessions()
    return [{"name": s.name, "state": s.state, "cwd": s.cwd,
             "provider": s.provider, "headless": s.headless, "voce": s.name == eu} for s in infos]


@mcp.tool(description="Manda um recado 1:1 pra outra sessão, como `hangar-send <sessao> <msg>`: "
                      "chega lá como `[de: <você>] texto`. `alvo` aceita `servidor::sessao` "
                      "pra outro servidor. Recusa alvo Claude local com caminho nativo "
                      "(use SendMessage) a menos que `tmux=true`.")
async def send(ctx: Context, alvo: str, texto: str, tmux: bool = False) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    if alvo == eu or (settings.server_id and alvo == f"{settings.server_id}::{eu}"):
        raise ToolError(f"recusado: '{alvo}' é esta sessão — o recado voltaria pra você. "
                        "Quem é quem: tool `sessoes` (campo `voce`).")
    # Modelo que escreve "[de: eu] …" por conta própria não ganha o prefixo em dobro — com ou sem o
    # "<servidor>::" na frente, que é como o envio pra outro servidor qualifica o remetente.
    servidor = rf"(?:{re.escape(settings.server_id)}::)?" if settings.server_id else ""
    texto = re.sub(rf"^\s*\[de:\s*{servidor}{re.escape(eu)}\]\s*", "", texto)
    if peers.is_remote(alvo):
        srv, sess = peers.split_addr(alvo)
        if not settings.server_id:
            raise ToolError("CP_SERVER_ID ausente no backend/.env — obrigatório pra envio cross-server")
        corpo = {"text": f"[de: {settings.server_id}::{eu}] {texto}", "steer": True}
        try:
            _, resp = await asyncio.to_thread(peers.call, srv, "POST", f"/api/sessions/{sess}/input", corpo)
        except peers.PeerError as e:
            raise ToolError(str(e)) from e
        return {"alvo": alvo, **(resp or {})}
    # `tmux` fica aceito por compatibilidade: o backend já escolhe o transporte (socket nativo,
    # plugin, tmux, fila) e nunca devolve o envio pro modelo fazer por outra ferramenta.
    try:
        resp = await api.input_prompt(alvo, api.InputBody(text=f"[de: {eu}] {texto}", steer=True))
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e
    return {"alvo": alvo, **resp}


@mcp.tool(description="Aviso pro grupo de pareamento desta sessão, como `hangar-send --group <msg>`: "
                      "chega como `[grupo: <você>]` nos demais. Marco, não conversa: NUNCA responda um "
                      "`[grupo: …]` com isto. O backend escolhe o transporte pra cada membro.")
async def group(ctx: Context, texto: str, tmux: bool = False) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    try:
        return await api.group_message(eu, api.GroupMsgBody(text=texto, forcar_tmux=tmux))
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e


@mcp.tool(description="Pareia esta sessão com outra pra uma tarefa, como `hangar-send --pair <sessao> "
                      "<tarefa>`: registra no app e injeta o protocolo nos dois lados. `alvo` aceita "
                      "`servidor::sessao`. `tarefa` é o título do grupo na lista: chave + assunto "
                      "numa linha (ex: `ABC-1234 Tela de login`); o combinado vai por `send`. "
                      "Só quando o usuário pedir pareamento.")
async def pair(ctx: Context, alvo: str, tarefa: str = "", substituir_tarefa: bool = False) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    try:
        return await api.pair_session(eu, api.PairBody(peer=alvo, task=tarefa, replace_task=substituir_tarefa))
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e


@mcp.tool(description="Desfaz o pareamento desta sessão (`hangar-send --unpair`).")
async def unpair(ctx: Context) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    try:
        return await api.unpair_session(eu)
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e


@mcp.tool(description="Cria outra sessão nesta máquina, como `hangar-send --new <nome> <cwd>`. Nunca "
                      "`tmux new-session` cru. `provider`: claude|codex|pi|omp|kimi; `headless` só "
                      "claude/codex. A sessão nasce na MESMA conta de quem chama, e a resposta "
                      "devolve o `config_dir` usado — confira. `conta` (caminho do config dir) "
                      "força outra; pra conta que ainda precisa ser preparada, use o CLI "
                      "(`hangar-send --new --conta <nome>`). `jev`: a sessão nasce com a chave do "
                      "Jev no ambiente, e só aí o `hangar-preview objetivo` (o laço que navega e "
                      "preenche tela sozinho) funciona nela. Omitido, vale o padrão do servidor.")
async def new_session(ctx: Context, nome: str, cwd: str, provider: str = "claude", engine: str | None = None,
                      model: str | None = None, effort: str | None = None, permissao: str | None = None,
                      headless: bool = False, read_only: bool = False,
                      conta: str | None = None, jev: bool | None = None) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    if headless and provider not in ("claude", "codex"):
        raise ToolError(f"headless só vale com provider claude ou codex (veio: {provider})")
    # A conta da sessão nova é a de QUEM CHAMA. Antes o nome resolvido era descartado e o
    # `config_dir` ia vazio: o backend caía na conta padrão (~/.claude), e uma sessão que vive
    # noutra conta criava a irmã na conta errada — dizendo, pela descrição desta tool, que tinha
    # herdado. Gasta a cota de quem ninguém escolheu e só aparece quando alguém confere.
    config_dir = conta
    if config_dir is None:
        cfg, confiavel = await asyncio.to_thread(api._caller_config_dir, eu)
        if not confiavel:
            # Não deu pra ler a conta de quem chama. Criar assim mesmo repetiria o bug de cima,
            # só que calado; quem quiser seguir escolhe a conta no parâmetro.
            raise ToolError(f"não consegui confirmar a conta da sessão '{eu}' — passe `conta` "
                            f"com o caminho do config dir, ou use `hangar-send --new --conta`")
        config_dir = str(cfg) if cfg else None
    try:
        info = await api.create_session(api.CreateBody(
            name=nome, cwd=cwd, provider=provider, engine=engine, model=model, effort=effort,
            permission_mode=permissao, headless=headless, read_only=read_only,
            config_dir=config_dir, jev=jev))
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e
    return {"name": info.name, "cwd": info.cwd, "provider": info.provider, "headless": info.headless,
            # Volta na resposta pra que herdar errado nunca mais passe despercebido.
            "config_dir": config_dir}


VERBOS_NAV = ("snapshot", "click", "fill", "type", "press", "hover", "wait", "eval", "layout", "console",
              "network", "text", "url", "shot", "close", "tab-list", "tab-new", "tab-switch", "tab-close")
# Duas chamadas do mesmo turno não podem intercalar `click` e `snapshot`: o CLI serializa por
# processo, aqui é uma trava por sessão.
_travas_nav: dict[str, asyncio.Lock] = {}


def _pasta_shots(sessao: str) -> Path:
    return Path.home() / ".hangar" / "nav" / "shots" / re.sub(r"\W+", "-", sessao)


async def _verbo_nav(sessao: str, verbo: str, args: list[str], aba: int | None) -> str:
    if verbo not in VERBOS_NAV:
        raise ToolError(f"verbo desconhecido: {verbo} (aceitos: {', '.join(VERBOS_NAV)})")
    if verbo == "eval" and not args:
        raise ToolError("eval precisa de um trecho JS")
    if verbo == "shot":
        # O shell grava onde mandarem; caminho nosso, estável, que o app do celular consegue servir.
        pasta = _pasta_shots(sessao)
        await asyncio.to_thread(pasta.mkdir, parents=True, exist_ok=True)
        args = [str(pasta / f"{int(time.time() * 1000)}.png")]
    try:
        texto = await asyncio.to_thread(navshell.verbo, sessao, verbo, args, aba)
    except navshell.ShellIndisponivel as e:
        raise ToolError(str(e)) from e
    if texto.startswith("erro:"):
        raise ToolError(texto)
    return texto


@mcp.tool(description="Abre o navegador embutido desta sessão no app desktop do Hangar, como "
                      "`hangar-preview open <url>`. O painel monta na tela do usuário: avise-o.")
async def browser_open(ctx: Context, url: str) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    try:
        await api.abrir_nav_sessao(eu, api.NavBody(url=url))
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e
    return {"ok": True, "aviso": "a janela do usuário muda: o painel do navegador abre agora"}


@mcp.tool(description="Um verbo do navegador embutido desta sessão (`hangar-preview <verbo>`). "
                      "Verbos: snapshot (árvore com refs @eN), click/hover <ref>, fill <ref> <texto>, "
                      "type <texto>, press <tecla>, wait [--text|--url] <valor>, eval <js> (só estado "
                      "não-DOM, nunca pra clicar), console, network, text, url, shot (devolve o caminho "
                      "do PNG), layout [mobile|desktop|<largura> <altura>], close, tab-list, "
                      "tab-new <url>, tab-switch <id>, tab-close [id]. "
                      "`aba` age numa aba sem trocar a que o usuário vê.")
async def browser(ctx: Context, verbo: str, args: list[str | int] | None = None, aba: int | None = None) -> str:
    eu = await _eu(ctx)
    async with _travas_nav.setdefault(eu, asyncio.Lock()):
        return await _verbo_nav(eu, verbo, [str(a) for a in args or []], aba)


@mcp.tool(description="Vários verbos do navegador em sequência, como `hangar-preview batch`: para no "
                      "primeiro que falhar e diz em qual. Cada passo é {verbo, args?, aba?}.")
async def browser_batch(ctx: Context, passos: list[dict[str, Any]]) -> dict[str, Any]:
    eu = await _eu(ctx)
    feitos: list[str] = []
    async with _travas_nav.setdefault(eu, asyncio.Lock()):
        for i, p in enumerate(passos):
            verbo = str(p.get("verbo") or "")
            try:
                feitos.append(await _verbo_nav(eu, verbo, [str(a) for a in p.get("args") or []], p.get("aba")))
            except ToolError as e:
                raise ToolError(f"parou no passo {i} ({verbo}): {e}\nfeitos antes: {json.dumps(feitos, ensure_ascii=False)}") from e
    return {"feitos": feitos}


# O gerenciador de sessões do SDK só roda UMA vez por instância: o sub-app nasce no lifespan.
_sub = None


async def _responder(send, status: int, corpo: bytes) -> None:
    await send({"type": "http.response.start", "status": status,
                "headers": [(b"content-type", b"application/json")]})
    await send({"type": "http.response.body", "body": corpo})


async def asgi(scope, receive, send):
    """Sub-app com o bearer conferido na porta: mount não passa pelo `require_auth` das rotas."""
    if scope["type"] != "http":
        return
    # Só sessões desta máquina falam com o MCP (mesma regra do `require_loopback`): o celular e
    # os peers usam a API normal.
    cliente = scope.get("client")
    if not cliente or cliente[0] not in auth_mod._LOOPBACK:
        await _responder(send, 403, b'{"detail":"so na maquina do backend"}')
        return
    auth = dict(scope["headers"]).get(b"authorization", b"")
    token = auth[7:] if auth.startswith(b"Bearer ") else b""
    if not token or not settings.auth_token or not secrets.compare_digest(token, settings.auth_token.encode()):
        await _responder(send, 401, b'{"detail":"unauthorized"}')
        return
    if _sub is None:
        await _responder(send, 503, b'{"detail":"mcp ainda nao subiu"}')
        return
    await _sub(scope, receive, send)


@contextlib.asynccontextmanager
async def lifespan():
    global _sub
    # Só sessões desta máquina falam com o /mcp; o Host de fora é recusado (DNS rebinding).
    seguranca = TransportSecuritySettings(
        enable_dns_rebinding_protection=True,
        allowed_hosts=["127.0.0.1", "127.0.0.1:*", "localhost", "localhost:*", "[::1]", "[::1]:*"],
        allowed_origins=["http://127.0.0.1:*", "http://localhost:*", "http://[::1]:*"])
    sub = mcp.streamable_http_app(streamable_http_path="/", transport_security=seguranca)
    async with sub.router.lifespan_context(sub):
        _sub = sub
        try:
            yield
        finally:
            _sub = None
