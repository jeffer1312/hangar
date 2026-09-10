"""A lista ÚNICA de credenciais do servidor — conta do Claude e chave de API na mesma tabela.

Decisão de desenho de 18/08/2026, do usuário: "a tela de contas deveria ser tudo igual, onde ficam
as contas do Claude e as de API também". Antes eram duas telas com dois vocabulários — Contas
(login do Claude, pasta no disco) e Motores (base_url + chave) — e a mesma pergunta ("quanto sobrou
nessa credencial?") só tinha resposta numa delas.

O que esta rota NÃO faz, de propósito: criar, apagar ou editar. Isso já existe e continua onde
estava — `POST/DELETE /api/claude-configs` pra conta do Claude, `PUT/DELETE /api/engines/{nome}`
pra chave. Duplicar o caminho de escrita pra "unificar" a tela criaria dois donos do mesmo dado,
que é exatamente o defeito que a unificação existe pra tirar. A única escrita daqui é o APELIDO,
que não é de nenhum dos dois lados: é do app.

`usos` é a resposta pra "onde essa chave é usada". Hoje só existe um uso ligado de verdade —
`claude_code`, que é a presença da chave no engines.json. Pi e Kimi CLI ainda NÃO são gravados por
aqui, e por isso não aparecem na lista: caixa marcada que não faz nada é mentira, não é promessa.
"""
import asyncio
import logging
from pathlib import Path
from typing import Literal

from fastapi import APIRouter, Depends, HTTPException, Request
from pydantic import BaseModel, Field

from app import agentes_sync, apelidos, contas, cotas, engines, oauth_codex, opencode_cota, codex_contas
from app.auth import require_auth
from app import engine_probe
from app.config import list_config_dirs
from app.mensagens import erro
from app.conta_estado import EstadoLogin, _login_de
from app.adapters.kimi.sessions import kimi_home

_log = logging.getLogger("hangar.credenciais")

credenciais_router = APIRouter(prefix="/api/credenciais")

Tipo = Literal["claude", "chave", "codex"]


class CotaResumo(BaseModel):
    """A mesma leitura da faixa do rodapé (app/cotas.py) — uma fonte só pros dois lugares."""

    estado: cotas.Estado
    janelas: list[cotas.JanelaCota] = []
    ts: float | None = None
    idade_s: float | None = None
    motivo: str | None = None
    # Rótulo que a FONTE deu à credencial. Serve às linhas que só a cota conhece (o provider do
    # Kimi, o OAuth do Codex): sem ele o nome sai do id, e `codex:/home/u/.codex` viraria um
    # caminho cru na tela onde a fonte já dizia "Codex".
    label: str | None = None


class Credencial(BaseModel):
    """Uma linha da tela. Os campos exclusivos de um tipo vêm como None no outro — a alternativa
    (dois modelos numa union) faria o front destrinchar tipo antes de desenhar a linha, e a linha
    é a MESMA nos dois casos: avatar, nome, subtítulo, cota, menu."""

    id: str
    tipo: Tipo
    auth_method: Literal["oauth", "api_key", "none", "unknown"] = "unknown"
    codex_account: str | None = None
    nome: str                     # o que a tela mostra: apelido, se houver
    nome_natural: str             # o que o disco diz (pasta / nome do motor)
    apelido: str | None = None    # só quando a pessoa deu um; a tela usa pra saber se pode limpar
    ativa: bool = False           # conta-base do app (só faz sentido no tipo claude)
    # tipo claude
    path: str | None = None
    login: EstadoLogin | None = None
    # tipo chave
    base_url: str | None = None
    chave_mascarada: str | None = None
    usos: list[str] = []
    cota: CotaResumo | None = None
    # Só pro OpenCode: ele não tem rota de cota, a leitura é a página do painel com o cookie de
    # sessão (ver app/opencode_cota.py). A tela precisa saber DUAS coisas diferentes — que esta
    # credencial aceita cookie, e se já tem um — pra oferecer o campo sem prometer o que não dá.
    aceita_cookie: bool = False
    cookie_definido: bool = False
    # Chave que o app tem como apagar: cadastrada nele (engines.json) ou bloco que ele mesmo gravou
    # na config de outro agente. O provedor nativo do Kimi é da pessoa — a tela não oferece Apagar.
    gerenciada: bool = True


def _mascarar(chave: str) -> str:
    """`sk-kimi-…4f2a`. Nunca devolve a chave inteira: esta rota é lida pela tela, e a tela vive
    num navegador — o valor cru só existe no arquivo 0600 e no processo da sessão."""
    if len(chave) <= 10:
        return "•" * len(chave)
    return f"{chave[:7]}••••{chave[-4:]}"


def _cota_por_id(forcar: bool = False) -> dict[str, CotaResumo]:
    """Cota de todas as credenciais, indexada pelo mesmo id desta lista. Uma chamada só: o
    `cotas.listar_cotas()` já é cache de 5 min e faz as leituras em paralelo; `forcar` é o
    botão "atualizar" da tela pedindo leitura nova em vez do cache."""
    fora = {}
    # Nomeado, não posicional: a assinatura da rota pode ganhar outro query param e um bool
    # posicional escorregaria pra ele em silêncio (achado da revisão).
    for c in cotas.listar_cotas(forcar=forcar):
        fora[c.id] = CotaResumo(estado=c.estado, janelas=c.janelas, ts=c.ts,
                                idade_s=c.idade_s, motivo=c.motivo, label=c.label)
    return fora


@credenciais_router.get("", dependencies=[Depends(require_auth)],
                        response_model=list[Credencial])
async def listar_endpoint(request: Request, forcar: bool = False) -> list[Credencial]:
    service = getattr(request.app.state, "codex_contas_login", None)
    async def snapshot(account):
        auth = service.cached_auth(account)
        if service.preparation_status(account).get("status") != "running":
            try:
                async with asyncio.timeout(3):
                    auth = await service.read_auth(account, refresh=forcar)
            except TimeoutError:
                _log.warning("leitura de autenticação Codex excedeu o prazo: %s", account.id)
        return {"id": account.id, "auth": auth or {"method": "unknown", "status": "unavailable"}}

    snapshots = await asyncio.gather(*(snapshot(a) for a in codex_contas.list_accounts())) if service else []
    return await asyncio.to_thread(listar, forcar, codex_snapshots=snapshots)


def listar(forcar: bool = False, *, codex_snapshots: list[dict] | tuple = ()) -> list[Credencial]:
    """Contas do Claude e chaves de API na mesma lista, com apelido e cota.

    `?forcar=true` re-lê a cota na hora (ignora o cache de 5 min) — é o botão "atualizar"
    da tela. A listagem em si (contas, chaves, login) nunca é cache."""
    nomes = apelidos.ler()
    cota = _cota_por_id(forcar)
    saida: list[Credencial] = []

    # Contas do Claude: mesmo filtro da aba antiga — conta de verdade (carimbada pelo app) ou a
    # base do app. Pasta de backup continua fora: a tela não conseguiria apagá-la.
    for c in list_config_dirs():
        if not (contas.e_conta(Path(c.path)) or c.active):
            continue
        cid = f"claude:{c.path}"
        saida.append(Credencial(
            id=cid, tipo="claude", nome=nomes.get(cid) or c.label, nome_natural=c.label,
            apelido=nomes.get(cid), ativa=bool(c.active), path=c.path,
            login=_login_de(c), cota=cota.get(cid), auth_method="oauth",
        ))

    # Chaves de API: o engines.json é o cadastro que já existe. Cada motor é uma credencial cujo
    # uso `claude_code` está ligado — é literalmente o que estar nesse arquivo significa.
    cookies = opencode_cota.ler_configs()
    for nome, dados in engines.listar().items():
        cid = f"chave:{nome}"
        chave = dados.get("api_key")
        natural = dados.get("label") or nome
        base = dados.get("base_url") or ""
        aceita = "opencode.ai" in base
        saida.append(Credencial(
            id=cid, tipo="chave", nome=nomes.get(cid) or natural, nome_natural=natural,
            apelido=nomes.get(cid), base_url=dados.get("base_url"),
            chave_mascarada=_mascarar(chave) if isinstance(chave, str) and chave else None,
            usos=["claude_code"], cota=cota.get(cid), auth_method="api_key",
            aceita_cookie=aceita, cookie_definido=aceita and cid in cookies,
        ))

    snapshots = {a["id"]: a for a in codex_snapshots}
    for account in codex_contas.list_accounts():
        cid = f"codex:{account.home.expanduser().resolve(strict=False)}"
        auth = snapshots.get(account.id, {}).get("auth", {})
        status = auth.get("status", "unavailable")
        saida.append(Credencial(
            id=cid, tipo="codex", nome=nomes.get(cid) or account.id,
            nome_natural=account.id, apelido=nomes.get(cid),
            codex_account=account.id, path=str(account.home), ativa=account.is_default,
            auth_method=auth.get("method", "unknown"), usos=["codex_cli"], cota=cota.get(cid),
            login=EstadoLogin(estado="indisponivel" if status == "unavailable" else "ok",
                             loggedIn=None if status == "unavailable" else status == "connected",
                             email=auth.get("email"), plano=auth.get("plan")),
        ))

    # Cota sem cadastro continua visível, mas não comprova autenticação do Codex.
    usos_por_prefixo = {"kimi:": "kimi_cli", "codex:": "codex_cli"}
    ja = {c.id for c in saida}
    for cid, resumo in cota.items():
        uso = next((u for p, u in usos_por_prefixo.items() if cid.startswith(p)), None)
        if cid in ja or uso is None:
            continue
        natural = resumo.label or cid.split(":", 1)[1]
        saida.append(Credencial(
            id=cid, tipo="codex" if uso == "codex_cli" else "chave",
            auth_method="unknown" if uso == "codex_cli" else "api_key",
            nome=nomes.get(cid) or natural, nome_natural=natural,
            apelido=nomes.get(cid), usos=[uso], cota=resumo,
            gerenciada=uso != "kimi_cli" or agentes_sync.bloco_gerenciado(
                kimi_home() / "config.toml", cid.split(":", 1)[1]),
        ))
    return saida


class ApelidoBody(BaseModel):
    """`apelido` vazio APAGA o apelido — é o botão "voltar ao nome original" sem uma rota só pra
    isso. O id vem no corpo e não no caminho porque ele carrega `/` (o path da conta)."""

    id: str = Field(min_length=1, max_length=512)
    apelido: str = Field(default="", max_length=40)


@credenciais_router.put("/apelido", dependencies=[Depends(require_auth)])
def definir_apelido(body: ApelidoBody) -> dict:
    """Renomeia a credencial NA TELA. Não toca na pasta nem no engines.json: renomear pasta de
    conta mexeria em caminho que um CLI vivo tem aberto, e renomear motor quebraria o
    `hangar-engine --exec <nome>` das sessões que já estão rodando nele."""
    mapa = apelidos.definir(body.id, body.apelido)
    return {"id": body.id, "apelido": mapa.get(body.id)}


class CookieBody(BaseModel):
    """Cookie de sessão do painel do OpenCode. Os dois vazios APAGAM a configuração.

    Não é chave de API: é a sessão do navegador na conta, e é por isso que ela não entra no
    engines.json junto da key — arquivo à parte, 0600, e o valor nunca volta pra tela.
    """

    id: str = Field(min_length=1, max_length=512)
    workspace_id: str = Field(default="", max_length=200)
    auth_cookie: str = Field(default="", max_length=4096)


@credenciais_router.put("/cookie", dependencies=[Depends(require_auth)])
def definir_cookie(body: CookieBody) -> dict:
    """Guarda (ou apaga) o cookie do painel do OpenCode desta credencial e devolve se ficou algum.

    Devolve só o booleano de propósito: confirmar gravação não precisa do valor, e ecoá-lo poria
    a sessão do navegador no corpo de uma resposta HTTP que a tela guarda em memória.
    """
    opencode_cota.definir_config(body.id, body.workspace_id, body.auth_cookie)
    # Invalida a leitura em cache: sem isto o cookie novo só valeria no próximo ciclo de 5 min, e
    # a pessoa acabou de colar justamente pra ver o número aparecer.
    with cotas._lock:
        cotas._cache.pop(body.id, None)
    return {"id": body.id, "cookie_definido": body.id in opencode_cota.ler_configs()}


class SyncBody(BaseModel):
    """Grava uma credencial JÁ CADASTRADA na configuração dos outros agentes.

    Só o `id` viaja: a chave o servidor já tem (engines.json). Mandá-la de novo no corpo poria o
    segredo num request que a tela guarda em memória, sem necessidade nenhuma.
    """

    id: str = Field(min_length=1, max_length=512)
    alvos: list[str] = Field(default_factory=lambda: list(agentes_sync.ALVOS))


@credenciais_router.post("/sincronizar", dependencies=[Depends(require_auth)])
def sincronizar_nos_agentes(body: SyncBody) -> dict:
    """Escreve a credencial no Pi, no Kimi e no Codex (o Claude Code já é o engines.json).

    O nome que vai pros outros agentes é o ID do motor, não o rótulo bonito: eles têm alfabeto
    próprio pra nome de provedor, e o rótulo do usuário ("PMédico 01") seria recusado.
    """
    if not body.id.startswith("chave:"):
        raise HTTPException(400, detail=erro("erro_credencial_sem_chave",
                                             "só credencial de chave de API pode ser sincronizada"))
    nome = body.id[len("chave:"):]
    dados = engines.listar().get(nome)
    if not dados:
        raise HTTPException(404, detail=erro("erro_credencial_inexistente",
                                             f"credencial {nome} não existe", nome=nome))
    base_url, api_key = dados.get("base_url") or "", dados.get("api_key") or ""
    if not base_url or not api_key:
        raise HTTPException(409, detail=erro("erro_credencial_incompleta",
                                             "credencial sem endereço ou sem chave"))
    # Os modelos vêm do PROVEDOR, não do que está salvo: é a lista que o Pi e o Kimi precisam ter, e
    # perguntar agora é o que garante que ela reflete a chave de verdade. Falhar aqui não impede a
    # gravação — provedor sem /v1/models ainda vale como provedor cadastrado, só sem lista.
    try:
        modelos = engine_probe.listar_modelos(base_url, api_key)
    except Exception as e:                                   # noqa: BLE001 - opcional por natureza
        _log.debug("sincronizar: sem modelos para %s: %r", nome, e)
        modelos = []
    alvos = tuple(a for a in body.alvos if a in agentes_sync.ALVOS) or agentes_sync.ALVOS
    return {"id": body.id, "modelos": len(modelos),
            "resultado": agentes_sync.sincronizar(nome, base_url, api_key, modelos, alvos)}


# ---------------------------------------------------------------- login OAuth do ChatGPT (Codex)
# O app faz o fluxo de código de dispositivo e espalha o resultado pro Codex, Pi e omp
# (app/oauth_codex.py). O poll é do front: `GET /codex/login` a cada 2s até `concluido`.

@credenciais_router.get("/codex", dependencies=[Depends(require_auth)])
def codex_estado() -> dict:
    return oauth_codex.estado()


@credenciais_router.post("/codex/login", dependencies=[Depends(require_auth)])
def codex_login_iniciar() -> dict:
    try:
        return oauth_codex.iniciar()
    except RuntimeError as e:
        raise HTTPException(409, detail=erro("erro_codex_login", str(e), motivo=str(e)))


@credenciais_router.get("/codex/login", dependencies=[Depends(require_auth)])
def codex_login_passo() -> dict:
    return oauth_codex.passo()


@credenciais_router.delete("/codex/login", dependencies=[Depends(require_auth)])
def codex_login_cancelar() -> dict:
    return oauth_codex.cancelar()


@credenciais_router.delete("/kimi/{nome}", dependencies=[Depends(require_auth)])
def apagar_provedor_kimi(nome: str) -> dict:
    """Só o bloco que o app gravou no config do Kimi (provedor órfão de um motor já apagado).
    Provedor do usuário nunca é tocado: 404, não remoção."""
    ok, motivo = agentes_sync.remover_kimi(nome)
    if not ok:
        raise HTTPException(404 if motivo in ("nao-gerenciado", "nao-instalado") else 400,
                            detail=erro("erro_credencial_nao_gerenciada", "credencial nao gerenciada pelo app"))
    return {"ok": True}
