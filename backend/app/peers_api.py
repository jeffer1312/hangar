"""Rotas dos peers — máquinas que este servidor alcança (aba Servidores), e o identificador
desta máquina (gravado no .env, onde o hangar-send e o pydantic na subida do backend o leem).

A Task 1 só registra o roteador com a listagem vazia. As Tasks 5 e 8 (Lote B) escrevem
aqui dentro, sem tocar em api.py.

Nome do módulo: `peers_api` de propósito — `app/peers.py` já é a leitura do
peers.json e as chamadas a outras máquinas, e o módulo de rota não pode ocupar o nome dela.
"""
import hashlib
import hmac
import logging
import os
import tempfile
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException

from app import alcance, atomico, descoberta, peers, peers_check, runtime_config, user_data
from app.auth import require_auth
from app.config import detect_lan_ip, resolve_bind_ip, settings
from app.mensagens import erro

peers_router = APIRouter(prefix="/api/peers")

_log = logging.getLogger("hangar")

# O `.env` (o MESMO que o hangar-send e o Settings leem): é o único arquivo que os dois enxergam —
# o identificador gravado aqui vale pro script e pra próxima subida do serviço.
_ENV_ARQUIVO = user_data.path(".env")


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
        if isinstance(cfg.get("enabled"), bool):
            entrada["enabled"] = cfg["enabled"]
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


@peers_router.get("/app-list", dependencies=[Depends(require_auth)])
def ler_lista_app() -> dict:
    """A lista de máquinas do app, com os tokens: é dela que o app abre a conexão de cada uma.
    Só o dono chega aqui (a porta do convidado não tem /api/peers)."""
    try:
        rev, lista = peers.ler_lista_app()
    except ValueError as e:
        raise HTTPException(503, detail=erro("peers_arquivo_ilegivel", str(e))) from e
    return {"rev": rev, "servers": lista}


@peers_router.put("/app-list", dependencies=[Depends(require_auth)])
def gravar_lista_app(body: dict) -> dict:
    rev = body.get("rev")
    if not isinstance(rev, str):
        raise HTTPException(400, detail=erro("peers_registro_invalido", "rev obrigatório"))
    try:
        rev, lista = peers.gravar_lista_app(rev, body.get("servers"))
    except peers.ListaMudou as e:
        raise HTTPException(409, detail=erro("peers_lista_mudou", "a lista mudou em outro app")) from e
    except ValueError as e:
        raise HTTPException(400, detail=erro("peers_registro_invalido", str(e))) from e
    return {"rev": rev, "servers": lista}


@peers_router.put("/{server_id}/enabled", dependencies=[Depends(require_auth)])
def set_peer_enabled(server_id: str, body: dict) -> list:
    enabled = body.get("enabled")
    if not isinstance(enabled, bool):
        raise HTTPException(400, detail=erro("peers_registro_invalido", "enabled precisa ser true ou false"))
    try:
        peers.set_peer_enabled(server_id, enabled)
    except ValueError as e:
        raise HTTPException(404, detail=erro("peers_desconhecido", str(e))) from e
    return _lista()


@peers_router.get("/{server_id}/token", dependencies=[Depends(require_auth)])
def token_do_peer(server_id: str) -> dict:
    """Token inteiro de UM peer, pedido de propósito: a tela o usa para passar a mostrar as sessões
    dessa máquina sem pedir o token de novo. Quem tem o token deste servidor já lê o peers.json
    pelas sessões dele. A listagem continua mascarada."""
    cfg = peers.peer_cfg(server_id)
    if cfg is None:
        raise HTTPException(404, detail=erro("peers_desconhecido", f"peer {server_id!r} desconhecido"))
    return {"token": cfg[1]}


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


@peers_router.get("/ping")
def ping() -> dict:
    """Sem auth de propósito: é o que a descoberta de outra máquina bate pra saber se aqui tem
    Hangar. Passar pelo `require_auth` sem token contaria como tentativa errada e bloqueava o
    IP de quem só perguntou. Não diz nada além de "sou Hangar"."""
    return {"hangar": True}


@peers_router.get("/descobrir", dependencies=[Depends(require_auth)])
def descobrir_maquinas() -> list[dict]:
    """Máquinas online na tailnet que respondem como Hangar. Sob demanda (botão), nunca na
    abertura da tela: cada chamada bate em todos os peers online."""
    try:
        return descoberta.descobrir()
    except descoberta.SemTailscale as e:
        raise HTTPException(503, detail=erro("descoberta_sem_tailscale", "tailscale não respondeu")) from e


@peers_router.get("/identificador", dependencies=[Depends(require_auth)])
def get_identificador() -> dict:
    return {"identificador": settings.server_id or "", "lan_url": lan_url(settings)}


@peers_router.get("/prova")
def get_prova(desafio: str) -> dict:
    """Sem credencial de propósito: o cliente confere quem responde no endereço local ANTES de
    mandar o token. Só quem tem o token calcula o HMAC do desafio."""
    if not 16 <= len(desafio) <= 128:
        raise HTTPException(400, detail=erro("peers_desafio_invalido", "desafio inválido"))
    ident = settings.server_id or ""
    mac = hmac.new(settings.auth_token.encode(), f"{desafio}|{ident}".encode(), hashlib.sha256)
    return {"identificador": ident, "prova": mac.hexdigest()}


def lan_url(s) -> str:
    """Endereço direto na rede local, que o cliente prefere ao Tailscale quando alcança.
    Vazio com bind só local: a porta não aceita conexão de fora desta máquina."""
    bind = resolve_bind_ip(s)
    if bind in ("127.0.0.1", "localhost", "::1"):
        return ""
    host = detect_lan_ip() if bind in ("0.0.0.0", "::") else bind
    return "" if host == "127.0.0.1" else f"http://{host}:{s.port}"


@peers_router.put("/identificador", dependencies=[Depends(require_auth)])
def put_identificador(body: dict) -> dict:
    """Grava o identificador desta máquina. Antes era somente-leitura (CP_SERVER_ID no .env);
    agora a tela escreve — o valor vai pro .env (fonte do hangar-send) E pro processo em memória,
    pra o pareamento cruzar a porta sem reiniciar o serviço. Vazio remove (pareamento desligado)."""
    valor = body.get("identificador") or ""
    if valor:
        try:
            peers.validar_id(valor)
        except ValueError as e:
            raise HTTPException(400, detail=erro("peers_id_invalido", str(e))) from e
    antigo = settings.server_id or ""
    _escrever_id_env(valor)
    settings.server_id = valor
    avisos = anunciar(antigo) if valor and valor != antigo else []
    return {"identificador": valor, "avisos": avisos}


@peers_router.post("/anunciar", dependencies=[Depends(require_auth)])
def anunciar_rota() -> list[dict]:
    if not settings.server_id:
        raise HTTPException(400, detail=erro("peers_sem_identificador", "esta máquina não tem identificador"))
    return anunciar("")


def _meu_endereco() -> str:
    """Tailscale primeiro: alcança de qualquer rede; a rede local só vale quando não há Tailscale."""
    nome = alcance._nome_tailscale()
    return f"https://{nome}" if nome else lan_url(settings)


def anunciar(antigo: str) -> list[dict]:
    """Grava esta máquina (nome, endereço, token) em cada peer e apaga lá o nome `antigo`.
    Sem isso a outra ponta segue conhecendo o nome velho e o recado de volta não tem destino."""
    alvos = [pid for pid, cfg in peers.listar_peers().items() if isinstance(cfg, dict) and cfg.get("enabled") is not False]
    if not alvos:
        return []
    base = _meu_endereco()
    if not base:
        return [{"id": "", "estado": "falhou", "motivo": "sem_endereco"}]
    corpo = {"id": settings.server_id, "base_url": base, "token": settings.auth_token}
    resultado: list[dict] = []
    # ponytail: um peer por vez; paralelo se a lista crescer e o teto de 8s por peer pesar.
    for pid in alvos:
        # Qualquer erro vira aviso do peer: o nome desta máquina já foi trocado, a rota não pode dar 500.
        try:
            peers.call(pid, "POST", "/api/peers", corpo)
        except Exception as e:
            _log.warning("anunciar em %s falhou: %s", pid, e)
            resultado.append({"id": pid, "estado": "falhou", "motivo": str(e)})
            continue
        # O nome novo já está lá: falha aqui é só o velho que sobrou, e o motivo precisa dizer isso.
        if antigo and antigo != settings.server_id:
            try:
                peers.call(pid, "DELETE", f"/api/peers/{antigo}")
            except Exception as e:
                if getattr(e, "status", None) != 404:
                    _log.warning("apagar %s em %s falhou: %s", antigo, pid, e)
                    resultado.append({"id": pid, "estado": "falhou", "motivo": f"nome antigo {antigo!r} ficou lá: {e}"})
                    continue
        resultado.append({"id": pid, "estado": "ok", "motivo": ""})
    return resultado


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