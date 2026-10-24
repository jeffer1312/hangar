"""Resolução de servidores peer + chamadas outbound pro backend do outro server. peers.json
(backend/peers.json, gitignored — MESMO arquivo que o hangar-send lê) mapeia server_id ->
{base_url, token}. Grupos entre máquinas são do servidor Rust; aqui o peers.json segue lido para
recados, lista e criação noutra máquina. Sem httpx no hot path: urllib da stdlib (mesmo padrão de
transcribe.py), chamado numa thread pelo caller async — os POSTs são ação de usuário, não hot path.

ponytail: só resolver base/token e um POST/DELETE com erro claro."""
import hashlib
import http.client
import json
import logging
import os
import re
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from app import atomico, diag, user_data

try:
    import fcntl
except ImportError:      # Windows: não existe flock; a trava vai por msvcrt.locking (ver _mutar)
    fcntl = None

try:
    import msvcrt
except ImportError:      # Linux/macOS: só o ramo Windows da trava usa
    msvcrt = None

_log = logging.getLogger("hangar")

_PEERS_FILE = user_data.path("peers.json")


class PeerError(Exception):
    """Falha de transporte/HTTP falando com um peer — mensagem já legível pro usuário/Claude.

    transport=True: falha de REDE (URLError/timeout/corpo ilegível) — a requisição PODE ter chegado
    e sido processada no peer, só a resposta se perdeu; o estado remoto fica INCERTO (o caller
    compensa). transport=False: o peer respondeu !2xx (rejeitou limpo, não comitou)."""

    def __init__(self, msg: str, transport: bool = False, status: int | None = None, detail=None):
        super().__init__(msg)
        self.transport = transport
        self.status = status
        # `detail` do corpo da resposta (dict do envelope ou string), sem o prefixo da mensagem.
        self.detail = detail


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
    recuperar à mão (mesmo racional de hangar_panel_common). Ausente continua sendo {}, sem peers.
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
    antigo seguraria um inode órfão (mesmo desenho de hangar_panel_common; no Windows sem fcntl a
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
        atomico.substituir(tmp, _PEERS_FILE)
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
        # O resto da entrada (enabled e web_url do painel, a parte `app` da lista das máquinas) não é
        # deste formulário: quem regrava endereço e token não está pedindo pra apagá-lo.
        dados[server_id] = {**(antigo if isinstance(antigo, dict) else {}), "base_url": base.rstrip("/"), "token": tok}
        if web_url:
            w = web_url.strip()
            if w:
                dados[server_id]["web_url"] = w
        return dados[server_id]

    return _mutar(_gravar)


def set_peer_enabled(server_id: str, enabled: bool) -> dict:
    """Liga ou desliga o peer na varredura (painel, `--list`, testes). Desconhecido é recusa."""
    validar_id(server_id)

    def _definir(dados: dict) -> dict:
        cfg = dados.get(server_id)
        if not _eh_peer(cfg):
            raise ValueError(f"servidor '{server_id}' não está no peers.json")
        cfg["enabled"] = enabled
        return cfg

    return _mutar(_definir)


def remover_peer(server_id: str) -> None:
    """Remove um peer do arquivo. Desconhecido é recusa (ValueError), não no-op: apagar um peer
    que não existe esconderia o typo de quem pediu."""
    validar_id(server_id)

    def _remover(dados: dict) -> None:
        cfg = dados.get(server_id)
        if not _eh_peer(cfg):
            raise ValueError(f"servidor '{server_id}' não está no peers.json")
        # Tirar os recados não tira a máquina da lista do app: a parte `app` fica.
        if isinstance(cfg.get("app"), dict):
            dados[server_id] = {"app": cfg["app"]}
        else:
            del dados[server_id]

    _mutar(_remover)


def _eh_peer(cfg: object) -> bool:
    """Entrada que é peer de recados. Só com a parte `app` é máquina da lista do app, não peer."""
    return isinstance(cfg, dict) and ("base_url" in cfg or "app" not in cfg)


# ── Lista de máquinas dos apps ──────────────────────────────────────────────────────────────
# A lista que o app nativo mostra mora aqui, na parte `app` de cada entrada: uma máquina, uma
# entrada. Entrada só com `app` (sem base_url) não é peer: nenhum leitor de recados a enxerga.

_APP_CAMPOS_BOOL = ("disabled", "invite")


class ListaMudou(Exception):
    """A lista mudou desde a leitura de quem grava (outro app gravou antes)."""


def _app_entrada(bruta: object) -> dict:
    if not isinstance(bruta, dict):
        raise ValueError("máquina precisa ser um objeto")
    e = {}
    for campo in ("id", "label", "address", "token"):
        v = bruta.get(campo, "")
        if not isinstance(v, str):
            raise ValueError(f"{campo} precisa ser texto")
        e[campo] = v
    if not e["id"] or len(e["id"]) > 64:
        raise ValueError("id da máquina vazio ou longo demais")
    if not e["address"].startswith(("http://", "https://")):
        raise ValueError("endereço precisa ser http(s)://")
    for campo in _APP_CAMPOS_BOOL:
        v = bruta.get(campo, False)
        if not isinstance(v, bool):
            raise ValueError(f"{campo} precisa ser true ou false")
        e[campo] = v
    lan = bruta.get("lan")
    if lan is not None:
        if not isinstance(lan, dict) or not all(isinstance(lan.get(k), str) for k in ("url", "id")):
            raise ValueError("lan precisa ser {url, id}")
        e["lan"] = {"url": lan["url"], "id": lan["id"]}
    return e


def _app_lista(dados: dict) -> list[dict]:
    partes = [cfg["app"] for cfg in dados.values() if isinstance(cfg, dict) and isinstance(cfg.get("app"), dict)]
    partes.sort(key=lambda a: a.get("pos", 0) if isinstance(a.get("pos"), int) else 0)
    return [{k: v for k, v in a.items() if k != "pos"} for a in partes]


def _revisao(lista: list[dict]) -> str:
    bruto = json.dumps(lista, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(bruto.encode()).hexdigest()[:16]


def ler_lista_app() -> tuple[str, list[dict]]:
    """(revisão, máquinas na ordem do app). Arquivo corrompido é erro: lista vazia faria o app
    achar que não tem máquina nenhuma."""
    lista = _app_lista(_ler_estrito())
    return _revisao(lista), lista


def _host(url: str) -> str:
    return (urllib.parse.urlsplit(url).hostname or "").lower()


def _loopback(host: str) -> bool:
    return host in ("localhost", "::1") or host.startswith("127.")


def _chave_livre(app_id: str, dados: dict) -> str:
    base = re.sub(r"[^a-z0-9_-]", "-", app_id.lower()).strip("-_")[:28] or "maquina"
    if not base[0].isalnum():
        base = "m" + base[:27]
    chave, n = base, 2
    while chave in dados:
        chave, n = f"{base}-{n}", n + 1
    return chave


def gravar_lista_app(revisao: str, maquinas: list) -> tuple[str, list[dict]]:
    """Troca a lista inteira do app, tudo ou nada. `revisao` é a da leitura de quem grava: diferente
    da atual, outro app gravou no meio e nada muda (ListaMudou).

    Cada máquina volta à entrada que já era dela (pelo id do app); nova casa com o peer de mesmo
    host, senão ganha entrada própria. Convite e endereço local nunca casam com peer: o token de
    convidado e o 127.0.0.1 deste app não podem virar credencial de recado."""
    if not isinstance(maquinas, list):
        raise ValueError("servers precisa ser uma lista")
    novas = [_app_entrada(m) for m in maquinas]
    if len({m["id"] for m in novas}) != len(novas):
        raise ValueError("id de máquina repetido")

    def _trocar(dados: dict) -> tuple[str, list[dict]]:
        if _revisao(_app_lista(dados)) != revisao:
            raise ListaMudou()
        dona = {}
        for chave, cfg in list(dados.items()):
            if isinstance(cfg, dict) and isinstance(cfg.get("app"), dict):
                dona[cfg["app"].get("id")] = chave
                del cfg["app"]
                if not cfg:
                    del dados[chave]
        usadas: set[str] = set()
        for pos, m in enumerate(novas):
            chave = dona.get(m["id"])
            if chave in usadas:
                chave = None
            host = _host(m["address"])
            if chave is None and not m["invite"] and not _loopback(host):
                chave = next((k for k, cfg in dados.items() if k not in usadas and isinstance(cfg, dict)
                              and "app" not in cfg and host in (_host(str(cfg.get("base_url", ""))), _host(str(cfg.get("web_url", ""))))), None)
            if chave is None:
                chave = _chave_livre(m["id"], dados)
            dados.setdefault(chave, {})["app"] = {**m, "pos": pos}
            usadas.add(chave)
        lista = _app_lista(dados)
        return _revisao(lista), lista

    return _mutar(_trocar)


def peer_cfg(server_id: str) -> tuple[str, str] | None:
    """(base_url sem barra final, token) do peer, ou None se ausente/inválido no peers.json."""
    p = _load().get(server_id)
    if not isinstance(p, dict) or not p.get("base_url") or not p.get("token"):
        return None
    return p["base_url"].rstrip("/"), p["token"]


class _SemRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None   # o urllib repassaria o Authorization ao destino do 3xx


_opener_sem_redirect = urllib.request.build_opener(_SemRedirect)


@diag.rastrear("peer.chamar")
def call(server_id: str, method: str, path: str, body: dict | None = None, timeout: int = 8):
    """POST/DELETE num backend peer. Devolve (status, json|None). Levanta PeerError em qualquer
    falha (peer desconhecido, inacessível, ou !2xx) com o detail real do backend remoto — sem isto,
    quem chama (o iniciador) não teria como reportar por que o pareamento não fechou."""
    cfg = peer_cfg(server_id)
    if not cfg:
        etapa = method if method in {"GET", "POST", "PUT", "PATCH", "DELETE"} else "outro"
        diag.registrar("peer.recusado", "erro", etapa=etapa, detalhe="configuracao_ausente_ou_incompleta")
        raise PeerError(f"servidor '{server_id}' não está em peers.json (ou sem base_url/token)")
    base, token = cfg
    return call_url(base, token, method, path, body, timeout, label=server_id, follow_redirects=True)


_MAX_CORPO = 1 << 20


def _ler_corpo(r, prazo: float, truncar: bool = False) -> bytes:
    """Lê até 1 MiB dentro do prazo TOTAL: o timeout do socket só vale por leitura, e um servidor
    que pinga um byte por vez seguraria a thread pra sempre."""
    partes, total = [], 0
    while True:
        if time.monotonic() > prazo:
            raise TimeoutError("prazo total da chamada estourou")
        parte = r.read1(65536)
        if not parte:
            return b"".join(partes)
        total += len(parte)
        if total > _MAX_CORPO:
            if truncar:
                return b"".join(partes + [parte])[:_MAX_CORPO]
            raise PeerError("resposta maior que 1 MiB", transport=True)
        partes.append(parte)


def call_url(base: str, token: str | None, method: str, path: str, body: dict | None = None,
             timeout: int = 8, label: str = "", follow_redirects: bool = False):
    """Chamada HTTP a um backend por endereço. Sem `token`, não manda Authorization. Endereço que
    outra pessoa entregou não segue redirect (o 3xx vira PeerError com `status`)."""
    inicio = time.monotonic()
    etapa = method if method in {"GET", "POST", "PUT", "PATCH", "DELETE"} else "outro"
    label = label or base
    data = json.dumps(body).encode() if body is not None else None
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if correlation := diag.req_atual.get():
        headers["X-Hangar-Req"] = correlation
    req = urllib.request.Request(base + path, data=data, method=method, headers=headers)
    prazo = inicio + timeout * 2
    try:
        abrir = urllib.request.urlopen if follow_redirects else _opener_sem_redirect.open
        with abrir(req, timeout=timeout) as r:
            status = r.status
            raw = _ler_corpo(r, prazo).decode(errors="replace")
    except urllib.error.HTTPError as e:
        diag.registrar("peer.falhou", "erro", etapa=etapa, codigo=str(e.code),
                       detalhe="http_recusado", erro_tipo=type(e).__name__,
                       ms=int((time.monotonic() - inicio) * 1000))
        # Peer respondeu !2xx — rejeitou de forma limpa, NÃO comitou. transport=False.
        try:
            raw = _ler_corpo(e, prazo, truncar=True).decode(errors="replace")
        except (OSError, TimeoutError):
            raw = ""
        try:
            detail = json.loads(raw).get("detail", raw)
        except (ValueError, RecursionError, AttributeError):
            detail = raw
        raise PeerError(f"{label} respondeu HTTP {e.code}: {detail}", transport=False, status=e.code,
                        detail=detail)
    except (urllib.error.URLError, http.client.IncompleteRead, OSError, TimeoutError) as e:
        causa = e.reason if isinstance(e, urllib.error.URLError) and isinstance(e.reason, BaseException) else e
        diag.registrar("peer.falhou", "erro", etapa=etapa, detalhe="transporte_resultado_incerto",
                       erro_tipo=type(causa).__name__, errno=getattr(causa, "errno", None),
                       winerror=getattr(causa, "winerror", None),
                       ms=int((time.monotonic() - inicio) * 1000))
        # Falha de rede/leitura truncada — pode ter chegado no peer. Estado remoto INCERTO.
        raise PeerError(f"{label} inacessível: {e}", transport=True)
    try:
        resultado = json.loads(raw) if raw.strip() else None
    except (ValueError, RecursionError, UnicodeError) as e:
        diag.registrar("peer.falhou", "erro", etapa=etapa, codigo=str(status),
                       detalhe="resposta_json_ilegivel", erro_tipo=type(e).__name__,
                       ms=int((time.monotonic() - inicio) * 1000))
        # 2xx com corpo ilegível (proxy retornando HTML em 200, leitura truncada): sem este catch a
        # exceção escapava crua, não virava PeerError, e o rollback do caller (que só pega PeerError)
        # NUNCA rodava — sidecar local comitava com estado remoto incerto. transport=True.
        raise PeerError(f"{label} respondeu corpo ilegível: {e}", transport=True)
    diag.registrar("peer.respondeu", etapa=etapa, codigo=str(status),
                   ms=int((time.monotonic() - inicio) * 1000))
    return status, resultado
