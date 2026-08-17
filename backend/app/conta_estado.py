"""Rotas do estado de conta — a lista de contas do servidor (aba Contas).

Cada conta da lista ganha: o caminho/rótulo/ativa (do list_config_dirs), o estado de login
(lido da CLI `claude auth status --json` rodando com CLAUDE_CONFIG_DIR apontando pra pasta da
conta — medido em 16/08: o CLI respeita a variável e responde sem sessão viva) e o último
limite lido com a idade (o sidecar de status mais recente dentro da conta, mesmo contrato de
statusline.py).

A Task 9 (faixa de cota) consome esta mesma rota: o campo `limite.linha` é a linha inteira da
statusline da sessão mais recente da conta, que o front já sabe parsear (lib/statusline.ts);
a Task 7 (entrar) também parte daqui. Por isso o shape é contrato: mudar um campo aqui é
mudar a Task do vizinho.

Saída que não é JSON (ou JSON do tipo errado) NUNCA derruba a listagem inteira — vira estado
nomeado `indisponivel`/`sem_leitura`, o mesmo precedente do statusline.read() exigindo dict.
"""
import json
import logging
import os
import subprocess
import threading
import time
from pathlib import Path
from typing import Literal

from fastapi import APIRouter, Depends, HTTPException
from pydantic import BaseModel, Field

from app import login_conta
from app.auth import require_auth
from app.config import list_config_dirs
from app.mensagens import erro

_log = logging.getLogger("claude_pocket.conta_estado")

conta_estado_router = APIRouter(prefix="/api/conta-estado")

# Mesmo subdiretório do statusline.py — é a FONTE da leitura de limite.
_STATUS_SUBDIR = ".claude-pocket-status"
# A chamada à CLI é um processo externo por conta, e a tela lista todas de uma vez: cache curto.
# 30s é curto o bastante pra um login novo (OAuth leva minutos) aparecer sem refresh manual e
# longo o bastante pra não pagar N subprocesses a cada montagem da aba.
_LOGIN_TTL = 30.0
_CLI_TIMEOUT = 10.0


class EstadoLogin(BaseModel):
    """Login de uma conta. `estado: "indisponivel"` = não deu pra ler (CLI ausente/falhou/saída
    estranha) — nunca exceção que derruba a lista; `motivo` é o código do porquê, estável."""

    estado: Literal["ok", "indisponivel"]
    loggedIn: bool | None = None
    email: str | None = None
    plano: str | None = None   # subscriptionType cru ("max"/"pro"/...) — dado do servidor
    motivo: str | None = None


class EstadoLimite(BaseModel):
    """Último limite lido da conta com a idade. `estado: "sem_leitura"` é explícito (nunca
    zero, nunca ausente): a conta existe mas nenhuma sessão rodou nela ainda."""

    estado: Literal["lido", "sem_leitura"]
    linha: str | None = None
    ts: float | None = None
    idade_s: float | None = None


class ContaEstado(BaseModel):
    path: str
    label: str
    active: bool
    login: EstadoLogin
    limite: EstadoLimite


def _parse_auth_status(stdout: str) -> dict | None:
    """Saída do `claude auth status --json` -> dict, ou None se não der pra confiar.

    Não-JSON e JSON válido do tipo errado caem no MESMO None (precedente statusline.read():
    JSON do tipo errado não levanta ValueError — viraria AttributeError lá na frente, bem pior).
    Um `claude` mais novo que mude o formato vira "indisponivel", nunca 500.
    """
    try:
        bruto = json.loads(stdout)
    except (json.JSONDecodeError, ValueError):
        return None
    if not isinstance(bruto, dict):
        return None
    return bruto


def _auth_status(dir_conta: Path) -> dict | None:
    """I/O: roda a CLI com o config dir da conta no ambiente. Trocada nos testes.

    O ambiente COPY é de propósito: a variável de OUTROS processos não pode ser afetada por
    uma leitura de estado (o backend é multiprocesso? não, mas o env do subprocess já nasce
    isolado — o ponto é não depender do env do processo pai pra apontar a conta certa).
    """
    env = dict(os.environ)
    env["CLAUDE_CONFIG_DIR"] = str(dir_conta)
    try:
        r = subprocess.run(
            ["claude", "auth", "status", "--json"],
            env=env, capture_output=True, text=True, timeout=_CLI_TIMEOUT,
        )
    except (OSError, subprocess.TimeoutExpired) as e:
        # FileNotFoundError (claude fora do PATH) é subclasse de OSError: cai aqui junto.
        _log.debug("auth status falhou para %s: %r", dir_conta, e)
        return None
    if r.returncode != 0:
        _log.debug("auth status rc=%s para %s: %s", r.returncode, dir_conta, r.stderr[:200])
        return None
    return _parse_auth_status(r.stdout)


def _estado_login(bruto: dict | None) -> EstadoLogin:
    """Decisão de estado em volta do dict da CLI (lógica pura, teste direto)."""
    if bruto is None:
        return EstadoLogin(estado="indisponivel", motivo="cli-indisponivel")
    logado = bruto.get("loggedIn")
    # Campo ausente ou do tipo errado NÃO é "deslogada": é formato que não dá pra confiar.
    # Afirmar "nunca entrou" sem prova é o mesmo defeito do front (ver parecer 17/08).
    if not isinstance(logado, bool):
        return EstadoLogin(estado="indisponivel", motivo="formato-desconhecido")
    email = bruto.get("email")
    plano = bruto.get("subscriptionType")
    return EstadoLogin(
        estado="ok",
        loggedIn=logado,
        email=email if isinstance(email, str) else None,
        plano=plano if isinstance(plano, str) else None,
    )


_login_cache: dict[str, tuple[float, EstadoLogin]] = {}
_login_lock = threading.Lock()


def _login_de(cfg) -> EstadoLogin:
    """Estado de login da conta, com o cache curto (chamada externa por conta)."""
    with _login_lock:
        agora = time.monotonic()
        hit = _login_cache.get(cfg.path)
        if hit is not None and agora - hit[0] < _LOGIN_TTL:
            return hit[1]
    estado = _estado_login(_auth_status(Path(cfg.path)))
    with _login_lock:
        _login_cache[cfg.path] = (time.monotonic(), estado)
    return estado


def _limite(dir_conta: Path) -> EstadoLimite:
    """Último limite lido: o sidecar de status mais recente DENTRO da conta.

    O publisher da statusline escreve em `<config>/.claude-pocket-status/<stem>.json` com o
    `ts` da escrita; a conta pode ter várias sessões (vários stems) — vale o mais novo, que é
    a leitura que o usuário viu por último. Sem teto de idade de propósito: "dado velho parece
    velho" — a idade vai no JSON e o front esmaece; jogar fora por idade faria a conta parecer
    "nunca leu" (mesma razão do statusline NÃO ter TTL curto, só o _MAX_AGE de 1 dia dele —
    aqui até o velho importa, porque é a leitura, não o texto).
    """
    pasta = dir_conta / _STATUS_SUBDIR
    try:
        arquivos = list(pasta.glob("*.json")) if pasta.is_dir() else []
    except OSError:
        arquivos = []
    melhor: tuple[float, str] | None = None
    for f in arquivos:
        try:
            o = json.loads(f.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue                 # ilegível: um sidecar ruim não derruba a conta
        if not isinstance(o, dict):
            continue                 # JSON válido do tipo errado: mesma regra do statusline
        ts, linha = o.get("ts"), o.get("line")
        if not isinstance(ts, (int, float)) or not isinstance(linha, str) or not linha.strip():
            continue
        if melhor is None or ts > melhor[0]:
            melhor = (float(ts), linha)
    if melhor is None:
        return EstadoLimite(estado="sem_leitura")
    ts, linha = melhor
    return EstadoLimite(estado="lido", linha=linha, ts=ts, idade_s=time.time() - ts)


@conta_estado_router.get("", dependencies=[Depends(require_auth)],
                         response_model=list[ContaEstado])
def listar_contas() -> list[ContaEstado]:
    """A lista de contas com estado de login e último limite — a fonte das abas Contas e da
    faixa de cota (Task 9). Conta deslogada CONTINUA na lista: `active`/`label`/`path` vêm do
    list_config_dirs, que não filtra por login."""
    return [
        ContaEstado(
            path=c.path, label=c.label, active=c.active,
            login=_login_de(c), limite=_limite(Path(c.path)),
        )
        for c in list_config_dirs()
    ]


# ----------------------------------------------------------------------- login remoto (Task 7)
#
# O fluxo de Entrar numa conta pelo app (sem terminal): `iniciar` abre a janela escondida e
# digita o comando de login; `passo` devolve o link de autorização; `confirmar` digita o código
# e confirma relendo o estado da conta; `cancelar` mata a janela. A confirmação NUNCA vem da
# aparência da tela (requisito do Step 1) e a janela nunca sobrevive ao fim do fluxo.

class LoginBody(BaseModel):
    """O código colado pelo usuário, para a confirmação."""
    codigo: str = Field(min_length=1, max_length=4096)


class PassoLogin(BaseModel):
    """Etapa atual do fluxo, lida do pane: `url` presente quando o CLI já imprimiu o
    endereço de autorização (o link tocável da tela)."""
    etapa: str
    url: str | None = None


def _conta_por_label(label: str):
    """A conta da lista por rótulo, ou None. O label é o NOME da conta (~/.claude-<nome>)."""
    for c in list_config_dirs():
        if c.label == label:
            return c
    return None


@conta_estado_router.post("/{label}/login", dependencies=[Depends(require_auth)])
def iniciar_login(label: str) -> dict:
    """Começa o login na conta: janela escondida + comando na CLI. 404 se a conta não existe."""
    conta = _conta_por_label(label)
    if conta is None:
        raise HTTPException(404, detail=erro("erro_conta_inexistente",
                                             f"conta {label} não existe", nome=label))
    try:
        return login_conta.iniciar(label, conta.path)
    except RuntimeError as e:
        # Já há tentativa em voo, ou a janela falhou: 409 com o motivo.
        raise HTTPException(409, detail=erro("erro_login_ja_em_curso", str(e))) from None


@conta_estado_router.post("/{label}/login/codigo", dependencies=[Depends(require_auth)])
def confirmar_login(label: str, body: LoginBody) -> dict:
    """Digita o código colado e espera a conta reler logada. Devolve e-mail e plano."""
    try:
        return login_conta.confirmar(label, body.codigo)
    except RuntimeError as e:
        # Sem tentativa em voo, ou a releitura falhou: 409 com o motivo.
        raise HTTPException(409, detail=erro("erro_login_sem_tentativa", str(e))) from None
    except TimeoutError as e:
        raise HTTPException(504, detail=erro("erro_login_timeout", str(e))) from None


@conta_estado_router.get("/{label}/login/passo", dependencies=[Depends(require_auth)],
                         response_model=PassoLogin)
def passo_login(label: str) -> PassoLogin:
    """A etapa atual do fluxo: o link de autorização quando já apareceu no pane."""
    p = login_conta.passo(label)
    return PassoLogin(etapa=p.get("etapa", "idle"), url=p.get("url"))


@conta_estado_router.post("/{label}/login/cancelar", dependencies=[Depends(require_auth)])
def cancelar_login(label: str) -> dict:
    """Cancela a tentativa em voo e mata a janela escondida. No-op sem tentativa."""
    return login_conta.cancelar(label)
