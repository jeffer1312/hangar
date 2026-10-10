"""Estado de login das contas Claude, para quem lista credenciais e escolhe provedor.

Quem lê o login é o Rust (`claude auth status` por conta, com cache); o Python só pergunta pela
ponte. Sem o Rust não há reserva: o login fica `indisponivel`, sem abrir o CLI aqui.
"""
from concurrent.futures import ThreadPoolExecutor
from typing import Literal

from pydantic import BaseModel


class EstadoLogin(BaseModel):
    """Login de uma conta. `estado: "indisponivel"` = não deu pra ler (CLI ausente/falhou/saída
    estranha, ou sem o servidor Rust) — nunca exceção que derruba a lista; `motivo` é o código do
    porquê, estável."""

    estado: Literal["ok", "indisponivel"]
    loggedIn: bool | None = None
    email: str | None = None
    plano: str | None = None   # subscriptionType cru ("max"/"pro"/...) — dado do servidor
    motivo: str | None = None
    # Vencimento do refresh token (epoch em SEGUNDOS). O refresh automático do CLI renova só o
    # access token; passado este prazo a conta exige /login de novo, e nada além dele estende.
    refreshExpiresAt: float | None = None


def _login_de(cfg) -> EstadoLogin:
    """Estado de login da conta, pelo Rust."""
    from app import account_bridge
    native = account_bridge.request_claude("auth", path=cfg.path)
    if native is None:
        return EstadoLogin(estado="indisponivel", motivo=account_bridge.NEED_RUST["code"])
    return EstadoLogin.model_validate(native)


def logins(cfgs) -> list[EstadoLogin]:
    """`_login_de` de várias contas ao mesmo tempo: a tela esperava a soma das idas."""
    cfgs = list(cfgs)
    if len(cfgs) < 2:
        return [_login_de(c) for c in cfgs]
    with ThreadPoolExecutor(max_workers=min(4, len(cfgs))) as pool:
        return list(pool.map(_login_de, cfgs))
