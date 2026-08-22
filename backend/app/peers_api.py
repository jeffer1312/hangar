"""Rotas dos peers — máquinas que este servidor alcança (aba Servidores), e o identificador
desta máquina (gravado no .env, onde o cp-send e o pydantic na subida do backend o leem).

A Task 1 só registra o roteador com a listagem vazia. As Tasks 5 e 8 (Lote B) escrevem
aqui dentro, sem tocar em api.py.

Nome do módulo: `peers_api` de propósito — `app/peers.py` já é a lógica de pareamento
cross-server, e o módulo de rota não pode ocupar o nome dela.
"""
import logging
import os
import tempfile
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException

from app import atomico, peers, peers_check, runtime_config
from app.auth import require_auth
from app.config import settings
from app.mensagens import erro

peers_router = APIRouter(prefix="/api/peers")

_log = logging.getLogger("claude_pocket")

# .env do backend (o MESMO que o cp-send e o .env_file do Settings leem): é o único arquivo que
# os dois enxergam — o identificador gravado aqui vale pro script e pra próxima subida do serviço.
_ENV_ARQUIVO = Path(__file__).resolve().parent.parent / ".env"


def _lista() -> list[dict]:
    """Lista com a CREDENCIAL MASCARADA — token de peer nunca sai por HTTP em texto claro.\n
    Entrada malformada (não-dict, sem base_url) é skip: não dá pra exibir um endereço que não
    existe, e derrubar a lista inteira por uma entrada podre esconderia os peers bons."""
    saida: list[dict] = []
    for pid, cfg in peers.listar_peers().items():
        if not isinstance(cfg, dict) or not cfg.get("base_url"):
            continue
        entrada = {
            "id": pid,
            "base_url": cfg["base_url"],
            "token": runtime_config.mascarar(str(cfg.get("token") or "")),
        }
        if cfg.get("web_url"):
            entrada["web_url"] = cfg["web_url"]
        saida.append(entrada)
    return sorted(saida, key=lambda p: p["id"])


@peers_router.get("", dependencies=[Depends(require_auth)])
def listar_peers() -> list:
    return _lista()


@peers_router.post("", dependencies=[Depends(require_auth)])
def gravar_peer(body: dict) -> list:
    """Upsert: regravar o mesmo identificador substitui, não duplica. Valida ANTES de escrever —
    erro volta 400 e o arquivo fica intocado (recusa é recusa)."""
    try:
        peers.gravar_peer(
            body.get("id") or "",
            body.get("base_url") or "",
            body.get("token") or "",
            body.get("web_url"))
    except ValueError as e:
        raise HTTPException(400, detail=erro("peers_registro_invalido", str(e))) from e
    return _lista()


@peers_router.delete("/{server_id}", dependencies=[Depends(require_auth)])
def apagar_peer(server_id: str) -> list:
    try:
        peers.remover_peer(server_id)
    except ValueError as e:
        raise HTTPException(404, detail=erro("peers_desconhecido", str(e))) from e
    return _lista()


@peers_router.get("/check", dependencies=[Depends(require_auth)])
def checar_peer(url: str = "", id: str = "") -> dict:
    """Estado de UM lado de um peer registrado (Task 8): responde? é a máquina esperada?

    Devolve o estado nomeado de `peers_check.checar_peer` — o front traduz pelo `code`, e
    o bloco de correção de endereço só aparece quando um lado falha. URL/id obrigatórios:
    sem eles não há o que checar (400 nomeado, não um teste de rede sem alvo).
    """
    if not url:
        raise HTTPException(400, detail=erro("peers_check_url_obrigatoria", "url obrigatória"))
    if not id:
        raise HTTPException(400, detail=erro("peers_check_id_obrigatorio", "id obrigatório"))
    return peers_check.checar_peer(url, id)


@peers_router.get("/identificador", dependencies=[Depends(require_auth)])
def get_identificador() -> dict:
    return {"identificador": settings.server_id or ""}


@peers_router.put("/identificador", dependencies=[Depends(require_auth)])
def put_identificador(body: dict) -> dict:
    """Grava o identificador desta máquina. Antes era somente-leitura (CP_SERVER_ID no .env);
    agora a tela escreve — o valor vai pro .env (fonte do cp-send) E pro processo em memória,
    pra o pareamento cruzar a porta sem reiniciar o serviço. Vazio remove (pareamento desligado)."""
    valor = body.get("identificador") or ""
    if valor:
        try:
            peers.validar_id(valor)
        except ValueError as e:
            raise HTTPException(400, detail=erro("peers_id_invalido", str(e))) from e
    _escrever_id_env(valor)
    settings.server_id = valor
    return {"identificador": valor}


def _escrever_id_env(valor: str) -> None:
    """Grava a linha CP_SERVER_ID no .env mantendo o resto byte a byte. Vazio remove a linha.

    Seam de I/O trocado no teste (régua da casa: uma função privada de I/O por módulo). Escrita
    atômica (tmp + os.replace) com o PID no nome do temporário — o mesmo formato do peers.json;
    ​​o .env nasce 0600 e permanece (guarda CP_AUTH_TOKEN e as chaves VAPID)."""
    # Lock sidecar cobrindo LEITURA e escrita (mesmo desenho de peers._mutar): a tela não é o
    # único escritor do .env (instalador, editor do usuário); sem trava, duas gravações
    # concorrentes liam o mesmo estado e a última apagava a mudança da outra, calado.
    #
    # A trava vem IMPORTADA de peers.py, não copiada: este arquivo tinha um `fcntl.flock` direto
    # com `except ImportError: fcntl = None`, que no Windows virava no-op silencioso — e o
    # comentário dizia "mesmo padrão de peers.py", frase que deixou de ser verdade quando o
    # peers.py ganhou o fallback de `msvcrt.locking` e ficou pior que não ter comentário nenhum.
    # O que se perde aqui é mais sensível que o peers.json: este .env é o do CP_AUTH_TOKEN (a
    # única credencial do app) e o das chaves VAPID. Uma verdade só, num lugar só.
    lock_path = _ENV_ARQUIVO.with_name(_ENV_ARQUIVO.name + ".lock")
    _ENV_ARQUIVO.parent.mkdir(parents=True, exist_ok=True)
    # "a+" e não "w" pelo motivo que o peers.py mede: no Windows a trava é de REGIÃO (o
    # `msvcrt.locking` tranca N bytes a partir da posição atual), e truncar o arquivo de lock
    # debaixo de outro processo que já o tem travado é pedir confusão.
    with open(lock_path, "a+", encoding="utf-8") as lock:
        peers._travar(lock)
        try:
            _escrever_id_env_travado(valor)
        finally:
            peers._destravar(lock)


def _escrever_id_env_travado(valor: str) -> None:
    """O corpo do `_escrever_id_env`, já sob a trava — separado só pra o unlock caber num
    `finally` sem aninhar o resto (mesma separação de `peers._mutar_travado`)."""
    linhas: list[str] = []
    try:
        linhas = _ENV_ARQUIVO.read_text(encoding="utf-8").splitlines(keepends=True)
    except FileNotFoundError:
        pass
    chave = "CP_SERVER_ID"
    achou = False
    novas: list[str] = []
    for linha in linhas:
        if linha.startswith(chave + "="):
            achou = True
            if valor:
                novas.append(f"{chave}={valor}\n")
            # vazio: a linha some — pydantic lê campo ausente como "" (pareamento desligado)
        else:
            novas.append(linha)
    if not achou and valor:
        if novas and not novas[-1].endswith("\n"):
            novas[-1] += "\n"
        novas.append(f"{chave}={valor}\n")
    # Órfão de processo morto entre o write e o replace (kill -9/OOM não roda except): contém
    # a malha INTEIRA do .env — CP_AUTH_TOKEN e chaves VAPID — não pode apodrecer no disco
    # esperando a próxima gravação. Dentro do lock: a limpeza de um processo não apaga o
    # temporário vivo de outro.
    for stale in _ENV_ARQUIVO.parent.glob(_ENV_ARQUIVO.name + ".*.tmp"):
        stale.unlink(missing_ok=True)
    fd, tmp_name = tempfile.mkstemp(
        dir=str(_ENV_ARQUIVO.parent),
        prefix=f"{_ENV_ARQUIVO.name}.{os.getpid()}.",
        suffix=".tmp")
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            fh.writelines(novas)
        os.chmod(tmp, 0o600)
        atomico.substituir(tmp, _ENV_ARQUIVO)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    try:
        os.chmod(_ENV_ARQUIVO, 0o600)
    except OSError as e:
        _log.warning("não consegui garantir 0600 em %s: %r", _ENV_ARQUIVO, e)