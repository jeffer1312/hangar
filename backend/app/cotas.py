"""Cota por credencial: o que sobrou no Python depois que o Rust passou a ler todas as fontes.

Contas e cotas de todas as fontes são do Rust (`crates/hangar-server/src/accounts/`), dono do cache
`cotas-cache.json`, do TTL e da espera após 429. Sem o Rust não há reserva: `listar_cotas` e
`cotas_claude` devolvem nada. Ficam aqui só:

 - os leitores Codex que a transferência de conversa usa (`conversation_transfer.py`);
"""
import json
import logging
import math
from pathlib import Path
from typing import Literal

from pydantic import BaseModel

from app import codex_appserver, codex_contas

_log = logging.getLogger("hangar.cotas")

# Idade a partir da qual uma leitura do Rust é velha: o TTL do cache dele.
_TTL_S = 300.0
_HTTP_TIMEOUT = 8.0

Estado = Literal["lida", "sem_credencial", "expirada", "indisponivel"]
Provedor = Literal["claude", "kimi", "opencode", "commandcode", "codex"]


class JanelaCota(BaseModel):
    """Uma janela de limite. `rotulo` é dado do provedor ("5h"/"7d"), não texto de interface."""

    rotulo: str
    pct: float
    reset_ts: float | None = None
    # Janela de UM modelo (o `weekly_scoped` do Fable): só aperta a sessão que roda nele. A
    # pílula do topo precisa disto pra não mostrar "Fable 100%" numa sessão Opus com 5h a 15%.
    por_modelo: bool = False


class ResetCredit(BaseModel):
    id: str
    expires_at: int | None = None
    title: str | None = None
    description: str | None = None
    status: Literal["available", "redeeming", "redeemed", "unknown"] = "unknown"


class ResetCredits(BaseModel):
    available_count: int
    credits: list[ResetCredit] | None = None


class CotaConta(BaseModel):
    """Cota de UMA credencial. `estado` distingue os quatro casos que a tela precisa separar:
    lida, conta sem credencial no disco, credencial expirada e falha de leitura."""

    id: str
    label: str
    provedor: Provedor
    # Conta-base do app (o `~/.claude` de `list_config_dirs`) — é a que uma sessão nova nasce
    # usando, e a faixa a marca. Não é "a conta da sessão em foco": isso é do chat, não da faixa.
    ativa: bool = False
    estado: Estado
    janelas: list[JanelaCota] = []
    ts: float | None = None
    idade_s: float | None = None
    motivo: str | None = None
    reset_credits: ResetCredits | None = None
    # Só Claude: vencimento do refresh token (segundos). Preenchido na saída, não no cache — o
    # /login muda o arquivo e a leitura é barata.
    refresh_expires_at: float | None = None


# Resultado cru de um leitor: (estado, janelas, motivo).
_Leitura = tuple[Estado, list[JanelaCota], str | None]


def _num(v: object) -> float | None:
    """O Kimi manda limite/restante como STRING ("100"). Aceita os dois, recusa o resto."""
    if isinstance(v, bool):
        return None
    if isinstance(v, (int, float)):
        return float(v)
    if isinstance(v, str):
        try:
            return float(v)
        except ValueError:
            return None
    return None


def _rotulo_janela(minutos: object) -> str:
    """Duração em minutos -> rótulo curto ("300" -> "5h"). Desconhecida vira "janela"."""
    n = _num(minutos)
    if n is None or n <= 0:
        return "janela"
    if n >= 1440 and n % 1440 == 0:
        return f"{int(n // 1440)}d"
    if n >= 60 and n % 60 == 0:
        return f"{int(n // 60)}h"
    return f"{int(n)}min"


# Presença da credencial do Codex, cacheada por raiz e assinatura do `auth.json`.
_cred_codex_cache: dict[str, tuple[tuple[int, int], bool]] = {}


def _codex_home(home: Path | str | None = None) -> Path:
    return (codex_contas.default_home() if home is None else Path(home)).expanduser().absolute()


def _auth_codex(home: Path | str | None = None) -> Path:
    """O `auth.json` da raiz Codex selecionada."""
    return _codex_home(home) / "auth.json"


def _tem_credencial_codex(home: Path | str | None = None) -> bool:
    """Par OAuth presente no disco. Sem isto não há o que perguntar — e perguntar custa um processo
    de ~1,2s, então a checagem vem antes.

    Cache pelo mtime do arquivo, mesma razão do `_mapa_pi`: `id_conta_codex` roda POR SESSÃO Codex
    a cada varredura da lista, e ler+parsear um JSON de 4KB nesse laço é o tipo de custo que o tick
    do SSE não pode pagar (o `_mtimes` sobra um `stat`).
    """
    global _cred_codex_cache
    if not isinstance(_cred_codex_cache, dict):
        _cred_codex_cache = {}
    auth_path = _auth_codex(home)
    chave = str(auth_path.parent.resolve(strict=False))
    try:
        stat = auth_path.stat()
        assinatura = (stat.st_mtime_ns, stat.st_size)
    except OSError:
        assinatura = (0, 0)
    hit = _cred_codex_cache.get(chave)
    if hit is not None and hit[0] == assinatura:
        tem = hit[1]
    else:
        try:
            auth = json.loads(auth_path.read_text(encoding="utf-8"))
            tokens = auth.get("tokens")
            tem = isinstance(tokens, dict) and bool(tokens.get("access_token"))
        except (OSError, ValueError):
            tem = False
        _cred_codex_cache[chave] = (assinatura, tem)
    return tem


def id_conta_codex(home: Path | str | None = None) -> str | None:
    """O id desta credencial na lista de cotas do Rust, ou None quando não há credencial.

    Uma função só porque o id vive em DOIS lugares: a fonte, aqui, e o campo `conta` da sessão
    Codex (`registry.list`). Ids diferentes fariam a pílula do topo procurar uma linha que a faixa
    desenha com outro nome, e cair no pior-geral sem ninguém entender — o mesmo cuidado que o
    comentário do `chave:<motor>` já registra.
    """
    raiz = _codex_home(home).resolve(strict=False)
    return f"codex:{raiz}" if _tem_credencial_codex(raiz) else None


def _janela_codex(o: object) -> JanelaCota | None:
    """O percentual já vem PRONTO (`usedPercent`), e a janela se identifica pela duração em minutos
    — o mesmo `_rotulo_janela` do Kimi. `resetsAt` é epoch em SEGUNDOS, ao contrário do
    CommandCode: dividir por 1000 aqui poria o reset em 1970."""
    if not isinstance(o, dict):
        return None
    pct = _num(o.get("usedPercent"))
    if pct is None:
        return None
    reset = _num(o.get("resetsAt"))
    return JanelaCota(rotulo=_rotulo_janela(o.get("windowDurationMins")),
                      pct=max(0.0, min(100.0, pct)), reset_ts=reset or None)


def _janela_http_codex(o: object) -> dict | None:
    if not isinstance(o, dict):
        return None
    segundos = o.get("limit_window_seconds")
    return {"usedPercent": o.get("used_percent"), "resetsAt": o.get("reset_at"),
            "windowDurationMins": int(segundos // 60) if isinstance(segundos, (int, float))
            and not isinstance(segundos, bool) and math.isfinite(segundos) else None}


class _Http429Codex(Exception):
    pass


def _rate_limits_http_codex(raiz: Path) -> dict | str | None:
    """`/wham/usage` no formato do `account/rateLimits/read`.

    None = este caminho não serve agora e o app-server responde no lugar; "http-429" = o backend
    pediu pra parar, e o app-server bate no MESMO backend com o MESMO token — cair nele só
    renovaria o 429, então a espera após 429 vale para as duas rotas.
    """
    def get(caminho: str) -> object:
        status, corpo = codex_appserver.backend_get(caminho, codex_home=raiz, timeout=_HTTP_TIMEOUT)
        if status == 429:
            raise _Http429Codex
        if status != 200 or not isinstance(corpo, dict):
            raise codex_appserver.CodexIndisponivel(f"http {status}")
        return corpo

    try:
        uso = get("/wham/usage")
        limites = uso.get("rate_limit")
        if not isinstance(limites, dict):
            raise codex_appserver.CodexIndisponivel("formato-desconhecido")
        r: dict = {"rateLimits": {"primary": _janela_http_codex(limites.get("primary_window")),
                                  "secondary": _janela_http_codex(limites.get("secondary_window"))}}
        if not any(_janela_codex(j) for j in r["rateLimits"].values()):
            raise codex_appserver.CodexIndisponivel("formato-desconhecido")
    except _Http429Codex:
        return "http-429"
    except codex_appserver.CodexIndisponivel as e:
        # info e não debug: cair calado no app-server é a regressão que ninguém veria.
        _log.info("cota: codex %s pelo app-server (http: %s)", raiz, e)
        return None
    _log.debug("cota: codex %s por http", raiz)
    return r


def _ler_codex(home: Path | str | None = None) -> _Leitura:
    """Cota da conta do Codex: `/wham/usage` do backend do ChatGPT, com o app-server efêmero
    (`account/rateLimits/read`) de reserva.

    A rota HTTP é a que o próprio binário chama com o token da conta; ela devolve o mesmo dado
    em ~0,5s, sem subir processo. O app-server continua para quando ela não serve: token vencido
    (quem renova é o CLI), credencial no keyring, resposta fora do formato. Ver "Cota e catálogo
    do Codex por HTTP" em docs/decisoes/harnesses.md.

    Nada aqui levanta, mesma regra do `_get_json`: um provedor que não responde não pode derrubar a
    lista das outras contas.
    """
    # Sem credencial não há o que perguntar: o processo seria pago à toa e voltaria "falhou" no
    # lugar de "não há credencial".
    raiz = _codex_home(home)
    if not _tem_credencial_codex(raiz):
        return "sem_credencial", [], None
    r = _rate_limits_http_codex(raiz)
    if r == "http-429":
        return "indisponivel", [], "http-429"
    if r is None:
        try:
            # Mesmo teto das fontes HTTP: quem lê espera a resposta antes de seguir.
            kwargs = {"codex_home": raiz} if home is not None else {}
            r = codex_appserver.perguntar("account/rateLimits/read", timeout=_HTTP_TIMEOUT,
                                          **kwargs)
        except codex_appserver.CodexAusente:
            return "indisponivel", [], "codex-ausente"
        except (RuntimeError, OSError) as e:
            _log.info("cota: codex nao respondeu: %r", e)
            return "indisponivel", [], "sem-resposta"
    limites = r.get("rateLimits")
    limites = limites if isinstance(limites, dict) else {}
    janelas = [j for j in (_janela_codex(limites.get("primary")),
                           _janela_codex(limites.get("secondary"))) if j is not None]
    if not janelas:
        return "indisponivel", [], "formato-desconhecido"
    return "lida", janelas, None


def listar_cotas(forcar: bool = False) -> list[CotaConta]:
    """Cotas do Rust, dono do cache com TTL de 5 min e espera após 429; sem ele, nenhuma."""
    from app import account_bridge
    owned = account_bridge.request_quotas(force=forcar)
    return [CotaConta.model_validate(row) for row in owned or []]


def cotas_claude(atualizar: bool = False) -> list[CotaConta]:
    """Sem atualizar, só o que o Rust já tem em cache; sem o Rust, nenhuma."""
    from app import account_bridge
    owned = account_bridge.request_quotas(cached_only=not atualizar)
    return [CotaConta.model_validate(row) for row in owned or [] if row.get("provedor") == "claude"]


class SugestaoConta(BaseModel):
    id: str
    label: str
    path: str
    ativa: bool
    # Percentual que sobra na janela mais cheia da conta — a que aperta primeiro.
    folga: float


def sugerir_claude(contas_lidas: list[CotaConta]) -> SugestaoConta | None:
    """A conta Claude com mais folga. Empate fica com a conta padrão (sessão nasce nela sem flag).

    Só conta com leitura vale: expirada ou indisponível não tem número, e chutar seria mandar
    uma sessão nascer numa conta que talvez peça login.
    """
    melhor: tuple[float, bool, CotaConta] | None = None
    for c in contas_lidas:
        if c.provedor != "claude" or c.estado != "lida" or not c.janelas:
            continue
        folga = 100.0 - max(j.pct for j in c.janelas)
        chave = (folga, c.ativa)
        if melhor is None or chave > (melhor[0], melhor[1]):
            melhor = (folga, c.ativa, c)
    if melhor is None:
        return None
    folga, _, c = melhor
    return SugestaoConta(id=c.id, label=c.label, path=c.id.removeprefix("claude:"),
                         ativa=c.ativa, folga=folga)
