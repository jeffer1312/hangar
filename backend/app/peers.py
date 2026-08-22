"""Resolução de servidores peer + chamadas outbound pro backend do outro server. peers.json
(backend/peers.json, gitignored — MESMO arquivo que o cp-send lê) mapeia server_id ->
{base_url, token}. Só o pareamento cross-server usa isto; recado 1:1 e --list seguem no cp-send
(bash). Sem httpx no hot path: urllib da stdlib (mesmo padrão de transcribe.py), chamado numa thread
pelo caller async — os POSTs de pareamento são ação de usuário, não hot path.

ponytail: só o que o pareamento precisa — resolver base/token e um POST/DELETE com erro claro.
Grupo cross-server de N não existe (o pareamento cross-server é 1:1); quando existir, isto não muda."""
import http.client
import json
import logging
import os
import re
import tempfile
import urllib.error
import urllib.request
from pathlib import Path

try:
    import fcntl
except ImportError:      # Windows: não existe flock; a trava vai por msvcrt.locking (ver _mutar)
    fcntl = None

try:
    import msvcrt
except ImportError:      # Linux/macOS: só o ramo Windows da trava usa
    msvcrt = None

_log = logging.getLogger("claude_pocket")

# Ao lado de backend/.env e backend/peers.json (app/ -> backend/). Robusto a qual é o cwd do backend.
_PEERS_FILE = Path(__file__).resolve().parent.parent / "peers.json"


class PeerError(Exception):
    """Falha de transporte/HTTP falando com um peer — mensagem já legível pro usuário/Claude.

    transport=True: falha de REDE (URLError/timeout/corpo ilegível) — a requisição PODE ter chegado
    e sido processada no peer, só a resposta se perdeu; o estado remoto fica INCERTO (o caller
    compensa). transport=False: o peer respondeu !2xx (rejeitou limpo, não comitou)."""

    def __init__(self, msg: str, transport: bool = False):
        super().__init__(msg)
        self.transport = transport


def is_remote(name: str) -> bool:
    """Nome qualificado 'srv::sessao' (peer em OUTRA máquina) vs sessão local (nome cru)."""
    return "::" in name


def split_addr(qualified: str) -> tuple[str, str]:
    """'srv::sessao' -> ('srv', 'sessao'). Split no PRIMEIRO '::' (nome de sessão não tem '::')."""
    srv, _, sess = qualified.partition("::")
    return srv, sess


def _load() -> dict:
    try:
        data = json.loads(_PEERS_FILE.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}   # esperado: sem peers.json = cross-server desligado, não é erro
    except (OSError, json.JSONDecodeError, ValueError) as e:
        # Arquivo existe mas está ilegível/malformado: BUG de config, não "desligado". Sem este log
        # (journalctl --user -u hangar-backend) uma vírgula sobrando no peers.json fazia
        # TODOS os peers falharem com "não cadastrado" e o usuário caçava o problema errado.
        _log.warning("peers.json ilegível/malformado (%s): %r", _PEERS_FILE, e)
        return {}
    if not isinstance(data, dict):
        _log.warning("peers.json não é um objeto JSON (%s)", _PEERS_FILE)
        return {}
    return data


_ID_OK = re.compile(r"^[a-z0-9][a-z0-9_-]{0,31}$")


def validar_id(nome: str) -> None:
    """Identificador de máquina (peer OU desta máquina): mesmo domínio do nome de conta.

    fullmatch e não match+$: com `$`, um nome terminado em quebra de linha final passava e o
    arquivo nascia com controle de linha no nome (precedente contas.py:79). O nome vira chave do
    peers.json e prefixo de endereço `srv::sessao` — os dois não aceitam espaço nem maiúscula.
    """
    if not isinstance(nome, str) or not _ID_OK.fullmatch(nome):
        raise ValueError("identificador: use minúsculas, números, '-' ou '_' (até 32 caracteres)")


def _ler_estrito() -> dict:
    """Leitura pra ESCRITA. O _load tolera corrompido (quem só lê não pode derrubar a malha);
    aqui corrompido é RECUSA: gravar por cima apagaria os tokens que o operador ainda podia
    recuperar à mão (mesmo racional de cp_panel_common). Ausente continua sendo {}, sem peers.
    """
    try:
        data = json.loads(_PEERS_FILE.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return {}   # esperado: sem peers.json = cross-server desligado, não é erro
    except (OSError, json.JSONDecodeError, ValueError) as e:
        raise ValueError(f"peers.json ilegível ({e}) — corrija o arquivo antes de gravar") from e
    if not isinstance(data, dict):
        raise ValueError("peers.json não é um objeto JSON — corrija o arquivo antes de gravar")
    return data


def _mutar(fn):
    """Read-MODIFY-WRITE do arquivo sob UM lock exclusivo — a LEITURA também fica dentro.

    Sem isto duas gravações concorrentes liam o mesmo estado ANTES do lock e a última a escrever
    apagava a mudança da outra, calado (test_gravacao_concorrente pega exatamente isso). O lock é
    sidecar (.lock) e não o próprio arquivo porque o os.replace troca o INODE — travar o arquivo
    antigo seguraria um inode órfão (mesmo desenho de cp_panel_common; no Windows sem fcntl a
    trava vira no-op, como em contas.py).

    Escrita atômica (tmp + os.replace) com o PID no nome do temporário: com nome fixo, duas
    escritas concorrentes abriam o MESMO caminho em modo truncate e o replace promovia bytes
    entrelaçados das duas — o acidente que este formato existe pra impedir.
    """
    lock_path = _PEERS_FILE.with_name(_PEERS_FILE.name + ".lock")
    # "a+" e não "w": o `w` trunca, e no Windows a trava é de REGIÃO do arquivo (msvcrt.locking
    # tranca N bytes a partir da posição atual) — truncar o arquivo de lock debaixo de outro
    # processo que já o tem travado é pedir confusão. No POSIX o flock é do descritor e o modo não
    # muda nada; "a+" também cria o arquivo se faltar, que era o motivo do `w`.
    with open(lock_path, "a+", encoding="utf-8") as lock:
        _travar(lock)
        try:
            return _mutar_travado(fn)
        finally:
            _destravar(lock)


def _travar(lock) -> None:
    """Trava exclusiva no sidecar. No Windows ela era NO-OP — e o arquivo travado é o peers.json,
    que guarda os TOKENS da malha inteira. Sem trava, `_mutar` é read-modify-write do arquivo
    todo: duas gravações concorrentes (app e CLI/painel) leem o mesmo estado e a última apaga a
    mudança da outra, calada. Medido nesta VM: o caso de concorrência falhava com KeyError no peer
    que sumiu. O `msvcrt.locking` é o mesmo mecanismo que `contas.py` já usa desde sempre — só o
    peers.py tinha ficado pra trás.

    Sem nenhum dos dois (plataforma exótica), degrada pro comportamento antigo em vez de estourar.
    """
    if fcntl is not None:
        fcntl.flock(lock, fcntl.LOCK_EX)
    elif msvcrt is not None:
        lock.seek(0)
        # LK_LOCK: tenta por ~10s antes de desistir (o LK_NBLCK falharia na hora). A alternativa
        # seria propagar OSError pro caller e perder a gravação por causa de meio segundo de
        # disputa.
        msvcrt.locking(lock.fileno(), msvcrt.LK_LOCK, 1)


def _destravar(lock) -> None:
    if fcntl is not None:
        fcntl.flock(lock, fcntl.LOCK_UN)
    elif msvcrt is not None:
        lock.seek(0)
        msvcrt.locking(lock.fileno(), msvcrt.LK_UNLCK, 1)


def _mutar_travado(fn):
    """O corpo do `_mutar`, já sob a trava. Separado só para o unlock caber num `finally` sem
    aninhar o resto do bloco."""
    dados = _ler_estrito()
    resultado = fn(dados)
    # Órfão de processo morto entre o write e o replace (kill -9/OOM não roda except): contém
    # a malha inteira de tokens — não pode apodrecer no disco esperando a próxima gravação.
    for stale in _PEERS_FILE.parent.glob(_PEERS_FILE.name + ".*.tmp"):
        stale.unlink(missing_ok=True)
    fd, tmp_name = tempfile.mkstemp(
        dir=str(_PEERS_FILE.parent),
        prefix=f"{_PEERS_FILE.name}.{os.getpid()}.",
        suffix=".tmp")
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as fh:
            json.dump(dados, fh, ensure_ascii=False, indent=2)
            fh.write("\n")
        os.chmod(tmp, 0o600)          # mkstemp já nasce 0600; explícito porque é contrato
        os.replace(tmp, _PEERS_FILE)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    # Arquivo que JÁ existia com modo frouxo volta a 0600 na gravação (o replace carrega o modo
    # do tmp, que é 0600; este chmod é a rede se algo alargar o modo entre replace e aqui).
    try:
        os.chmod(_PEERS_FILE, 0o600)
    except OSError as e:
        _log.warning("não consegui garantir 0600 em %s: %r", _PEERS_FILE, e)
    return resultado


def listar_peers() -> dict:
    """Mapa id -> cfg do peers.json. Arquivo ausente = sem peers, não erro."""
    return _load()


def gravar_peer(server_id: str, base_url: str, token: str, web_url: str | None = None) -> dict:
    """Upsert de um peer: regravar o mesmo identificador substitui, não duplica. Valida ANTES de
    escrever (recusa é recusa — nada de gravação parcial). Preserva `enabled`, que é o toggle do
    painel e não pertence a este formulário."""
    validar_id(server_id)
    if not isinstance(base_url, str) or not isinstance(token, str):
        raise ValueError("endereço e token precisam ser texto")
    if web_url is not None and not isinstance(web_url, str):
        raise ValueError("web_url precisa ser texto")
    base = base_url.strip()
    tok = token.strip()
    if not (base.startswith("http://") or base.startswith("https://")) or not tok:
        raise ValueError("endereço precisa ser http(s):// e o token não pode ficar vazio")

    def _gravar(dados: dict) -> dict:
        antigo = dados.get(server_id)
        dados[server_id] = {"base_url": base.rstrip("/"), "token": tok}
        if isinstance(antigo, dict) and antigo.get("enabled") is not None:
            dados[server_id]["enabled"] = antigo["enabled"]
        # web_url é do painel (cp_panel_common/cp-panel-data), não deste formulário: quem regrava
        # sem informá-lo não está pedindo pra apagá-lo. Mesmo racional do enabled.
        if isinstance(antigo, dict) and antigo.get("web_url"):
            dados[server_id]["web_url"] = antigo["web_url"]
        if web_url:
            w = web_url.strip()
            if w:
                dados[server_id]["web_url"] = w
        return dados[server_id]

    return _mutar(_gravar)


def remover_peer(server_id: str) -> None:
    """Remove um peer do arquivo. Desconhecido é recusa (ValueError), não no-op: apagar um peer
    que não existe esconderia o typo de quem pediu."""
    validar_id(server_id)

    def _remover(dados: dict) -> None:
        if server_id not in dados:
            raise ValueError(f"servidor '{server_id}' não está no peers.json")
        del dados[server_id]

    _mutar(_remover)


def peer_cfg(server_id: str) -> tuple[str, str] | None:
    """(base_url sem barra final, token) do peer, ou None se ausente/inválido no peers.json."""
    p = _load().get(server_id)
    if not isinstance(p, dict) or not p.get("base_url") or not p.get("token"):
        return None
    return p["base_url"].rstrip("/"), p["token"]


def call(server_id: str, method: str, path: str, body: dict | None = None, timeout: int = 8):
    """POST/DELETE num backend peer. Devolve (status, json|None). Levanta PeerError em qualquer
    falha (peer desconhecido, inacessível, ou !2xx) com o detail real do backend remoto — sem isto,
    quem chama (o iniciador) não teria como reportar por que o pareamento não fechou."""
    cfg = peer_cfg(server_id)
    if not cfg:
        raise PeerError(f"servidor '{server_id}' não está em peers.json (ou sem base_url/token)")
    base, token = cfg
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(
        base + path, data=data, method=method,
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            status = r.status
            raw = r.read().decode()
    except urllib.error.HTTPError as e:
        # Peer respondeu !2xx — rejeitou de forma limpa, NÃO comitou. transport=False.
        raw = e.read().decode(errors="replace")
        try:
            detail = json.loads(raw).get("detail", raw)
        except (json.JSONDecodeError, ValueError, AttributeError):
            detail = raw
        raise PeerError(f"{server_id} respondeu HTTP {e.code}: {detail}", transport=False)
    except (urllib.error.URLError, http.client.IncompleteRead, OSError, TimeoutError) as e:
        # Falha de rede/leitura truncada — pode ter chegado no peer. Estado remoto INCERTO.
        raise PeerError(f"{server_id} inacessível: {e}", transport=True)
    try:
        return status, (json.loads(raw) if raw.strip() else None)
    except (json.JSONDecodeError, ValueError) as e:
        # 2xx com corpo ilegível (proxy retornando HTML em 200, leitura truncada): sem este catch a
        # exceção escapava crua, não virava PeerError, e o rollback do caller (que só pega PeerError)
        # NUNCA rodava — sidecar local comitava com estado remoto incerto. transport=True.
        raise PeerError(f"{server_id} respondeu corpo ilegível: {e}", transport=True)
