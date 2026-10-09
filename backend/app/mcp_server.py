"""Servidor MCP do Hangar: as operações do `hangar-send` como tools, montadas no backend em /mcp.

Quem chama se identifica pelos cabeçalhos X-Hangar-* (app.quem_chama); o token é o mesmo bearer
do backend, conferido ANTES do sub-app porque `app.mount()` passa por fora do `Depends`.
Cada tool chama a mesma função de rota que o CLI chama por HTTP. A exceção é `html_render`: a
página aparece no lugar da chamada da tool, e o CLI não tem esse lugar.
"""

from __future__ import annotations

import asyncio
import contextlib
import json
import re
import secrets
import time
from pathlib import Path
from typing import Any, Literal

from fastapi import HTTPException
from mcp.server import MCPServer
from mcp.server.mcpserver import Context
from mcp.server.mcpserver.exceptions import ToolError
from mcp.server.transport_security import TransportSecuritySettings

from app import auth as auth_mod
from app import external_pairs, navshell, peers, quem_chama
from app.config import settings

mcp = MCPServer("hangar")

# Nomes antigos das tools (português) → nome de hoje. Uma sessão já aberta carregou o catálogo na
# abertura e vai chamar pelo nome que conhece até ser reiniciada — e sessão de trabalho não fecha
# porque o servidor renomeou uma tool. Aqui o nome velho continua CHAMÁVEL sem aparecer no
# catálogo: quem abre agora vê só os nomes novos, e quem está no meio de uma tarefa não quebra.
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
                      "`voce: true` marca esta sessão; mesmo `grupo` = mesmo grupo de trabalho. "
                      "Equivale a `hangar-send --list` sem os servidores remotos.")
async def sessions(ctx: Context) -> list[dict[str, Any]]:
    from app import api
    try:
        eu = await _eu(ctx)
    except ToolError:
        eu = None
    infos = await api.list_sessions(ctx.request_context.request)
    return [{"name": s.name, "state": s.state, "cwd": s.cwd,
             "provider": s.provider, "headless": s.headless, "voce": s.name == eu,
             "grupo": s.pair_gid} for s in infos]


@mcp.tool(description="Manda um recado 1:1 pra outra sessão, como `hangar-send <sessao> <msg>`: "
                      "chega lá como `[de: <você>] texto`. `alvo` aceita `servidor::sessao` "
                      "pra outro servidor. O backend escolhe o transporte (socket nativo, plugin, "
                      "tmux ou fila) e diz se entregou. O texto vira prompt pago lá: só a informação "
                      "ou o pedido, sem saudação nem apresentação (o cabeçalho já diz quem manda).")
async def send(ctx: Context, alvo: str, texto: str, tmux: bool = False) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    if alvo == eu or (settings.server_id and alvo == f"{settings.server_id}::{eu}"):
        raise ToolError(f"recusado: '{alvo}' é esta sessão — o recado voltaria pra você. "
                        "Quem é quem: tool `sessions` (campo `voce`).")
    # Modelo que escreve "[de: eu] …" por conta própria não ganha o prefixo em dobro — com ou sem o
    # "<servidor>::" na frente, que é como o envio pra outro servidor qualifica o remetente.
    servidor = rf"(?:{re.escape(settings.server_id)}::)?" if settings.server_id else ""
    texto = re.sub(rf"^\s*\[de:\s*{servidor}{re.escape(eu)}\]\s*", "", texto)
    if peers.is_remote(alvo):
        if external_pairs.ambiguous(peers.split_addr(alvo)[0]):
            raise ToolError(f"'{alvo}': o nome é ao mesmo tempo uma máquina tua e um par externo; "
                            "renomeie a máquina no peers.json")
        rec = external_pairs.by_address(alvo)
        if rec is not None:
            from app import external_pair_api
            if rec.local_session != eu:
                raise ToolError(f"'{alvo}' é par externo de '{rec.local_session}', não desta sessão")
            try:
                resp = await external_pair_api.send_external(rec, texto)
            except HTTPException as e:
                raise ToolError(_detalhe(e)) from e
            return {"alvo": alvo, **resp}
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
                      "`[grupo: …]` com isto. O backend escolhe o transporte pra cada membro. Uma "
                      "frase direta, sem saudação: cada membro paga o texto como prompt.")
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
                      "Só quando o usuário pedir pareamento. `orq`: grupo de orquestração (skill "
                      "orquestrar), como `--pair --orq`; com `alvo` vazio o grupo tem só esta sessão.")
async def pair(ctx: Context, alvo: str = "", tarefa: str = "", substituir_tarefa: bool = False,
               orq: bool = False) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    try:
        return await api.pair_session(eu, api.PairBody(peer=alvo, task=tarefa, replace_task=substituir_tarefa,
                                                       orq=orq))
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
                      "`tmux new-session` cru. `provider`: claude|codex|pi|omp|kimi. O que for "
                      "omitido HERDA de quem chama: a conta (claude/pi/omp), o modo de permissão "
                      "(claude; `plan` herda o modo de base) e o sem terminal (`headless`, só "
                      "claude/codex). Parâmetro explícito vence: `headless: false` força terminal, "
                      "`conta` (caminho do config dir) força outra conta. Conta herdada com 95% ou "
                      "mais de uso nasce na conta de mais folga (como `--conta auto`); a resposta "
                      "devolve `config_dir` e `account_source` (`inherited` ou `quota`) — confira. "
                      "Pra conta que ainda precisa ser preparada, use o CLI "
                      "(`hangar-send --new --conta <nome>`). `jev`: a sessão nasce com a chave do "
                      "Jev no ambiente, e só aí o `hangar-preview objetivo` (o laço que navega e "
                      "preenche tela sozinho) funciona nela. Omitido, vale o padrão do servidor. "
                      "`service_tier`: `priority` liga o Fast, `default` desliga; só Codex ou "
                      "Claude com motor GPT no CLIProxyAPI local. Worktree: `branch` com "
                      "`new_branch: true` e `base` cria uma branch nova numa cópia separada de `cwd`; "
                      "`branch` sem `new_branch` usa uma branch que já existe. `codex_account`: id "
                      "da conta Codex cadastrada. `subagent_model`: modelo dos subagentes (claude "
                      "sem motor).")
async def new_session(ctx: Context, nome: str, cwd: str, provider: str | None = None, engine: str | None = None,
                      model: str | None = None, effort: str | None = None, permissao: str | None = None,
                      headless: bool | None = None, read_only: bool = False,
                      conta: str | None = None, jev: bool | None = None,
                      service_tier: Literal["default", "priority"] | None = None,
                      branch: str | None = None, new_branch: bool = False, base: str | None = None,
                      codex_account: str | None = None, subagent_model: str | None = None) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    if provider is None:
        provider = await api._default_session_provider(conta, engine)
    if headless and provider not in ("claude", "codex"):
        raise ToolError(f"headless só vale com provider claude ou codex (veio: {provider})")
    # Conta, modo e sem terminal omitidos herdam de `eu` dentro do create_session, que é o mesmo
    # caminho do `hangar-send --new`.
    try:
        info = await api.create_session(api.CreateBody(
            name=nome, cwd=cwd, provider=provider, engine=engine, model=model, effort=effort,
            permission_mode=permissao, headless=headless, read_only=read_only,
            config_dir=conta, jev=jev, service_tier=service_tier, branch=branch or None, new_branch=new_branch,
            base=base or None, codex_account=codex_account, subagent_model=subagent_model, creator=eu))
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e
    return {"name": info.name, "cwd": info.cwd, "provider": info.provider, "headless": info.headless,
            # Volta na resposta pra que herdar errado nunca mais passe despercebido.
            "config_dir": info.config_dir, "account_source": info.account_source,
            **({"avisos": info.avisos} if info.avisos else {})}


@mcp.tool(description="Fecha OUTRA sessão desta máquina, com ou sem terminal, como `hangar-send "
                      "--close <sessao>`: ela sai do grupo sem aviso. Nunca a própria. Use para a "
                      "sessão que você abriu e que já terminou o trabalho.")
async def close_session(ctx: Context, alvo: str) -> dict[str, Any]:
    from app import api
    if peers.is_remote(alvo):
        raise ToolError("close_session só fecha sessão desta máquina")
    eu = await _eu(ctx)
    if alvo == eu:
        raise ToolError(f"recusado: '{alvo}' é esta sessão — peça pra outra fechar")
    # O DELETE responde ok pra nome que não existe: nome errado não pode virar "fechada".
    if not await asyncio.to_thread(quem_chama._por_nome, alvo):
        raise ToolError(f"sessão '{alvo}' não existe (tool `sessions`)")
    try:
        # A rota DELETE recusa sessão no meio de uma troca de conta/agente; a chamada direta, não.
        await api._transfer_check(alvo)
        return await api.kill_session(alvo)
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e


VERBOS_NAV = ("snapshot", "click", "fill", "type", "press", "hover", "wait", "eval", "tema", "layout", "console",
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
                      "`hangar-preview open <url>`. No app nativo (Windows e Linux) o painel Navegador "
                      "não monta sozinho: diga que a página está aberta no navegador da sessão, nunca "
                      "que apareceu na tela do usuário. Só no Electron antigo o painel monta sozinho.")
async def browser_open(ctx: Context, url: str) -> dict[str, Any]:
    from app import api
    eu = await _eu(ctx)
    try:
        await api.abrir_nav_sessao(eu, api.NavBody(url=url))
    except HTTPException as e:
        raise ToolError(_detalhe(e)) from e
    return {"ok": True}


_HTML_RENDER = (
    "Mostra uma página HTML pronta (gráfico, tabela, diagrama, mock de tela, comparação) DENTRO desta conversa, "
    "no lugar desta chamada e acima do seu texto final; chame antes de escrever a resposta. O leitor já vê a página: "
    "a resposta não a anuncia, não diz onde ela está e não repete o que ela mostra — diga só o que ela não diz. "
    "Confira antes de publicar com draft=true: devolve `shot` (PNG, leia com a ferramenta de imagem), `console` e "
    "`heights`; com o app desktop aberto, passe a `url` do rascunho a browser_open exatamente como veio (caminho "
    "relativo; o servidor completa endereço e acesso) e use browser para passar o mouse e clicar. "
    "Página publicada não muda: corrigir é publicar de novo. "
    "url (http/https, no lugar de html) abre um site de verdade dentro da conversa, com o login do navegador do "
    "app; só aparece vivo no app desktop; use para app rodando em localhost ou site que exige login. "
    "PÁGINA: um documento só, com <style> e <script> embutidos. Imagem local por caminho absoluto "
    "(src=\"/abs/a.png\", url(/abs/b.webp) ou string JS) é embutida sozinha; arquivo que não é imagem é recusado. "
    "URL http(s) (biblioteca de gráfico em CDN) carrega como está; file: não. "
    "LAYOUT: a moldura não tem borda e fica sobre o fundo da conversa, na largura da coluna (cerca de 728px no "
    "desktop, 360px no celular). Deixe html, body e o elemento mais externo SEM cor de fundo. Largura fluida, sem "
    "padding horizontal no elemento externo, sem cartão, borda ou título de banner em volta: a página é parte da "
    "resposta. Caixa que precisa de fundo próprio leva padding de 16px ou mais e cantos var(--radius). Gráfico com "
    "altura fixa em pixel. Nada de 100vh nem height:100% em html/body: a moldura cresce com a página. "
    "TEMA: use as variáveis --foreground, --muted-foreground, --surface, --border, --accent, --accent-foreground, "
    "--danger, --warning, --success, --code-background, --chart-1 a --chart-4, --radius, --font-sans, --font-mono; "
    "--background é transparente. Elas seguem o tema claro/escuro do app; seu CSS pode sobrescrever. "
    "own_theme=true para mock de outro produto/site ou página que precisa das próprias cores: ela leva o próprio "
    "fundo e paleta, como foi desenhada, e não recebe o tema do app; sem ele, a página segue o tema do app. "
    "height (80-2000) só para limitar a moldura e deixar o resto rolar dentro dela.")


@mcp.tool(description=_HTML_RENDER)
async def html_render(ctx: Context, title: str, html: str | None = None, url: str | None = None,
                      height: int | None = None, draft: bool = False, own_theme: bool = False) -> dict[str, Any]:
    from app import pages_bridge
    if (html is None) == (url is None):
        raise ToolError("erro_pagina_html_ou_url: passe html ou url, nunca os dois")
    eu = await _eu(ctx)
    payload: dict[str, Any] = {"session": eu, "title": title, "draft": draft}
    if url is not None:
        payload["url"] = url
    else:
        payload["html"] = html
    # Só quando pedido: hangar-server antigo recusa campo desconhecido no corpo.
    if own_theme:
        payload["own_theme"] = True
    if height is not None:
        payload["height"] = height
    try:
        return await asyncio.to_thread(pages_bridge.publish, payload)
    except pages_bridge.PagesBridgeError as e:
        raise ToolError(f"{e.code}: {e.detail}" if e.detail else e.code) from e


@mcp.tool(description="Um verbo do navegador embutido desta sessão (`hangar-preview <verbo>`). "
                      "Verbos: snapshot (árvore com refs @eN), click/hover <ref>, fill <ref> <texto>, "
                      "type <texto>, press <tecla>, wait [--text|--url] <valor>, eval <js> (só estado "
                      "não-DOM, nunca pra clicar), console, network, text, url, shot (devolve o caminho "
                      "do PNG), tema <claro|escuro|sistema>, layout [mobile|desktop|<largura> <altura>], "
                      "close. Só no Electron antigo: tab-list, tab-new <url>, tab-switch <id>, "
                      "tab-close [id] e `aba` (age numa aba sem trocar a que o usuário vê); o app "
                      "nativo tem um navegador por sessão, sem abas.")
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
