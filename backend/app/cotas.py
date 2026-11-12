"""Cota por credencial: o que sobrou no Python depois que o Rust passou a ler todas as fontes.

Contas e cotas de todas as fontes são do Rust (`crates/hangar-server/src/accounts/`), dono do cache
`cotas-cache.json`, do TTL e da espera após 429. Sem o Rust não há reserva: `listar_cotas` e
`cotas_claude` devolvem nada.
"""
from typing import Literal

from pydantic import BaseModel


# Idade a partir da qual uma leitura do Rust é velha: o TTL do cache dele.
_TTL_S = 300.0

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
