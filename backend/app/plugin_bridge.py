"""Plugin `hangar` do Claude: instalação, token por sessão e a faixa/avisos dos mods.

Entrega de texto, estado, pergunta e sugestão do plugin são do Rust (`plugin::routes`); aqui
ficam a faixa (`/ui`), os avisos (`/toast`) e as confirmações de clique que o repasse dos mods
ainda manda ao Python.

A identidade é o `CP_SESSION_NAME` que o pane já carrega, e o segredo é um
token por sessão cunhado no nascimento — o bearer do app NUNCA entra no
ambiente do pane.
"""
import asyncio
import hashlib
import hmac
import json
import logging
import re
import secrets
import shutil
import subprocess
import threading
import time
from pathlib import Path

from fastapi import APIRouter, HTTPException, Request
from pydantic import BaseModel, Field

from app import atomico, install_root, state_facts

_log = logging.getLogger("hangar.plugin_bridge")

plugin_router = APIRouter(prefix="/api/plugin")

_lock = threading.Lock()


# Capacidade do CLI, não versão: o número seria um palpite sobre qual release ganhou a flag, e
# quem derruba a sessão é a flag desconhecida — `claude --plugin-dir` inexistente sai com erro e o
# pane morre no nascimento, com o `tmux new-session` ainda devolvendo 0 (o app reportaria sucesso).
# `--help` custa 0,20 s (medido) e responde direto; o cache evita pagar isso a cada sessão.
_TTL_CAPACIDADE_S = 600.0
_capacidade: tuple[float, bool] | None = None
# Mods ligados por padrão no CLI daqui em diante; a variável do acesso antecipado é ignorada.
MODS_BY_DEFAULT = (2, 1, 287)
# Pela raiz da instalação: o `~/.hangar/plugin-dir` e o `--plugin-dir` das sessões apontam para o
# `current`, e a sessão aberta antes da troca de versão segue com os hooks depois da poda da antiga.
PLUGINS_ROOT = install_root.root() / "plugins"
PLUGIN_SRC = PLUGINS_ROOT / "hangar"
_versao: tuple[float, tuple[int, ...] | None] | None = None


def cli_version() -> tuple[int, ...] | None:
    """`claude --version` como (2, 1, 287); None quando não dá para ler. Em cache, com prazo."""
    global _versao
    if _versao is not None and time.monotonic() - _versao[0] < _TTL_CAPACIDADE_S:
        return _versao[1]
    exe = shutil.which("claude")
    versao = None
    if exe:
        try:
            r = subprocess.run([exe, "--version"], capture_output=True, text=True, timeout=10,
                               encoding="utf-8", errors="replace")
            m = re.match(r"\s*(\d+)\.(\d+)\.(\d+)", r.stdout or "")
            versao = tuple(int(x) for x in m.groups()) if m else None
        except (OSError, subprocess.SubprocessError) as e:
            _log.warning("plugin: `claude --version` falhou: %r", e)
    _versao = (time.monotonic(), versao)
    return versao


def mods_by_default() -> bool:
    versao = cli_version()
    return versao is not None and versao >= MODS_BY_DEFAULT


def aceita_plugin_dir() -> bool:
    """O `claude` desta máquina conhece `--plugin-dir`? Em cache, com prazo."""
    global _capacidade
    if _capacidade is not None and time.monotonic() - _capacidade[0] < _TTL_CAPACIDADE_S:
        return _capacidade[1]
    exe = shutil.which("claude")
    ok = False
    if exe:
        try:
            r = subprocess.run([exe, "--help"], capture_output=True, text=True, timeout=10,
                               encoding="utf-8", errors="replace")
            ok = "--plugin-dir" in (r.stdout or "")
        except (OSError, subprocess.SubprocessError) as e:
            # Sonda que não respondeu vira NÃO: o preço de errar para o não é o caminho de sempre,
            # e o de errar para o sim é sessão que não nasce.
            _log.warning("plugin: `claude --help` falhou: %r", e)
    _capacidade = (time.monotonic(), ok)
    return ok


def esquecer_capacidade() -> None:
    """Descarta as sondas do CLI. Quem acabou de atualizar o `claude` precisa disto."""
    global _capacidade, _versao
    _capacidade = None
    _versao = None


def _ligado_de_verdade() -> bool:
    from app import runtime_config
    if not runtime_config.get("claude_function_hooks"):
        return False
    return mods_by_default() or aceita_plugin_dir()


def ligado() -> bool:
    """O caminho do plugin vale nas sessões Claude desta máquina?

    `claude_function_hooks` é o liga/desliga (nasce ligado). Com ele ligado, vale no CLI com mods por
    padrão (2.1.287+) ou no anterior que aceita `--plugin-dir` com a variável do acesso antecipado."""
    return _ligado_de_verdade()


def raizes_dos_plugins() -> list[str]:
    """Um `--plugin-dir` por mod de `plugins/`, com `plugins/hangar` sempre primeiro.

    Só o plugin de `--plugin-dir` fica POR FORA dos instalados pelo marketplace na cadeia de hooks,
    e a faixa dos mods (`ui.ts`) só enxerga o que os plugins depois dele desenham: por isso o do
    Hangar abre a lista."""
    if not ligado():
        return []
    try:
        outros = sorted(p for p in PLUGINS_ROOT.iterdir()
                        if p != PLUGIN_SRC and (p / ".claude-plugin" / "plugin.json").is_file())
    except OSError as e:
        # Mod que não deu para listar fica de fora; a sessão nasce com o do Hangar.
        _log.warning("plugin: não deu para listar %s: %r", PLUGINS_ROOT, e)
        outros = []
    return [str(PLUGIN_SRC), *map(str, outros)]


def env_da_sessao(name: str, key: str) -> dict[str, str]:
    """O que o processo precisa para achar a ponte: endereço e o token DESTE processo.

    `key` distingue o processo dos outros que nasceram com o mesmo nome; o servidor Rust acha a
    sessão por ela, também depois de um renomear. O bearer do app não entra aqui — quem roda
    dentro do pane não é o app."""
    if not ligado():
        return {}
    from app.config import settings
    return {
        "HANGAR_PLUGIN_URL": f"http://127.0.0.1:{settings.port}/api/plugin",
        "HANGAR_PLUGIN_TOKEN": mint(name, key),
    }


def mint(name: str, key: str | None = None) -> str:
    """Token da ponte: `<key>.<HMAC(nome, key)>`, ou só o HMAC do nome sem `key`.

    DERIVADO, não sorteado: sorteado ele viveria só na memória do backend, e todo restart deixava
    a sessão viva batendo 403 para sempre. Com a chave dentro do token, a conferência se refaz
    igual depois do restart sem guardar nada. Não é o bearer do app: é um HMAC dele, de mão única.

    O formato só com o nome é o dos processos lançados antes da chave; dois processos com o mesmo
    nome têm o mesmo token nele.
    """
    from app.config import settings
    segredo = (settings.auth_token or "hangar").encode()
    if key is None:
        return hmac.new(segredo, f"plugin:{name}".encode(), hashlib.sha256).hexdigest()[:32]
    return key + "." + hmac.new(segredo, f"plugin:{name}:{key}".encode(), hashlib.sha256).hexdigest()[:32]


def machine_key() -> str:
    """Chave desta máquina para o `/whoami`: derivada como o `mint`, e só abre aquela rota."""
    from app.config import settings
    segredo = (settings.auth_token or "hangar").encode()
    return hmac.new(segredo, b"plugin:machine", hashlib.sha256).hexdigest()[:32]


def machine_file(home: Path | None = None) -> Path:
    return (home or Path.home()) / ".hangar" / "plugin.json"


def _publish_address(home: Path | None = None) -> None:
    """Onde o plugin acha a ponte: sessão aberta fora do Hangar não recebe `HANGAR_PLUGIN_*`."""
    from app.config import settings
    alvo = machine_file(home)
    alvo.parent.mkdir(parents=True, exist_ok=True)
    tmp = alvo.with_name(alvo.name + ".tmp")
    tmp.write_text(json.dumps({"url": f"http://127.0.0.1:{settings.port}/api/plugin",
                               "chave": machine_key()}), encoding="utf-8")
    try:
        tmp.chmod(0o600)
    except OSError:
        pass
    atomico.substituir(tmp, alvo)


def plugin_dir_file(home: Path | None = None) -> Path:
    return (home or Path.home()) / ".hangar" / "plugin-dir"


def _publish_plugin_dir(home: Path | None = None) -> None:
    """Caminhos dos plugins para o wrapper do shell, que não sabe onde o repositório mora.

    Sessão aberta no terminal precisa do mesmo `--plugin-dir` das que o backend abre: é o único
    caminho do plugin, e o que o põe por fora do marketplace para enxergar a faixa dos mods. Sem
    `raizes_dos_plugins()` o arquivo sai, porque flag que o CLI não conhece mata a sessão ao nascer."""
    alvo = plugin_dir_file(home)
    raizes = raizes_dos_plugins()
    if not raizes:
        alvo.unlink(missing_ok=True)
        return
    alvo.parent.mkdir(parents=True, exist_ok=True)
    tmp = alvo.with_name(alvo.name + ".tmp")
    # Uma pasta por linha, texto puro: quem lê é shell (bash, zsh, fish, PowerShell), sem JSON.
    tmp.write_text("".join(r + "\n" for r in raizes), encoding="utf-8", newline="\n")
    atomico.substituir(tmp, alvo)


def _publish_for_machine(home: Path | None = None) -> None:
    # Um não segura o outro: endereço que falhou não pode deixar o wrapper com o plugin-dir velho.
    try:
        _publish_address(home)
    finally:
        _publish_plugin_dir(home)


# O conftest troca `publish_address`; o teste chega na implementação por `_publish_address` e
# `_publish_plugin_dir`.
publish_address = _publish_for_machine


def esquecer(name: str) -> None:
    with _lock:
        _bands.pop(name, None)
        _toasts.pop(name, None)
    _band_wakers.pop(name, None)
    _toast_wakers.pop(name, None)
    state_facts.notify(name)


def _confere(name: str, token: str) -> None:
    key, dot, _ = token.partition(".")
    if not secrets.compare_digest(mint(name, key if dot else None), token):
        raise HTTPException(403, detail="token do plugin invalido")


def _entra(body) -> None:
    """Confere o token e põe em `body.sessao` o nome atual da sessão do processo.

    O plugin manda o nome com que o processo nasceu; renomeada a sessão sem relançar, é a chave do
    token que diz qual é ela agora. Token só do nome, ou chave que o runtime não conhece, fica no nome."""
    _confere(body.sessao, body.token)
    key, dot, _ = body.token.partition(".")
    if not dot:
        return
    nome = _nome_da_chave(key)
    if nome:
        body.sessao = nome
    elif key not in _chaves_sem_sessao:
        # Uma vez por chave: a faixa e os avisos voltam a cada mudança. Sessão fora do runtime cai aqui também.
        _chaves_sem_sessao.add(key)     # ponytail: cresce uma entrada por lançamento sem runtime
        _log.info("plugin: chave sem sessão no runtime; segue pelo nome sessao=%s", body.sessao)


_chaves_sem_sessao: set[str] = set()


def _nome_da_chave(key: str) -> str | None:
    """A sessão do runtime com essa chave: a dela (sem terminal e wrapper do shell) ou a do token do
    lançamento (`plugin_key`, sessão com terminal aberta pelo Hangar)."""
    from app import runtime_coordinator
    coordinator = runtime_coordinator.current()
    if coordinator is None:
        return None
    for name, slot_key in list(coordinator.names.items()):
        slot = coordinator.slots.get(slot_key)
        if slot_key == key or slot is not None and slot.binding.meta.get("plugin_key") == key:
            return name
    return None


# Faixa e painéis que os mods desenham: (versão, {"above", "columns", "panes"}, JSON do app). A
# versão é de todas as sessões juntas: o SSE só compara se mudou desde o que já mandou. O JSON é
# feito uma vez por mudança, não uma vez por conexão aberta.
_bands: dict[str, tuple[int, dict, str]] = {}
_band_seq = 0
_VAZIA: dict = {"above": None, "columns": None, "panes": []}
_VAZIA_JSON = json.dumps({"above": None, "panes": []})
# Quem espera algo da sessão: um Event por sessão, trocado a cada aviso, acorda todos de uma vez.
# Guardado com o loop que o criou: Event usado em outro loop levanta RuntimeError.
_band_wakers: dict[str, tuple[asyncio.AbstractEventLoop, asyncio.Event]] = {}


def _waker(tabela: dict, name: str) -> asyncio.Event:
    loop = asyncio.get_running_loop()
    guardado = tabela.get(name)
    if guardado is None or guardado[0] is not loop:
        guardado = tabela[name] = (loop, asyncio.Event())
    return guardado[1]


def _acordar_todos(tabela: dict, name: str) -> None:
    guardado = tabela.pop(name, None)
    if guardado is not None:
        guardado[1].set()


async def _esperar_ate(tabela: dict, name: str, achou, timeout: float):
    """Espera `achou()` dar verdadeiro, acordando a cada aviso da tabela, até `timeout`."""
    fim = time.monotonic() + timeout
    while True:
        valor = achou()
        if valor:
            return valor
        resta = fim - time.monotonic()
        if resta <= 0:
            return None
        try:
            await asyncio.wait_for(_waker(tabela, name).wait(), resta)
        except asyncio.TimeoutError:
            return achou() or None


def _guardar_faixa(name: str, above: dict | None, columns: int | None, panes: list[dict]) -> None:
    global _band_seq
    dados = {"above": above, "columns": columns, "panes": panes}
    with _lock:
        # Reenvio igual (o plugin reenvia de tempos em tempos) não vira evento para todo app aberto.
        if name in _bands and _bands[name][1] == dados:
            return
        _band_seq += 1
        _bands[name] = (_band_seq, dados, json.dumps({"above": above, "panes": panes}, ensure_ascii=False))
    _acordar_todos(_band_wakers, name)
    # Largura e âncora da prévia saem da faixa.
    state_facts.notify(name)


def band(name: str) -> tuple[int, dict]:
    """A faixa e os painéis da sessão, como o app recebe, e a versão; versão 0 quando nada foi desenhado."""
    with _lock:
        versao, dados, _ = _bands.get(name, (0, _VAZIA, ""))
    return versao, {"above": dados["above"], "panes": dados["panes"]}


def band_json(name: str) -> str:
    """O mesmo de `band`, já em JSON, para o evento `plugin_ui`."""
    with _lock:
        return _bands[name][2] if name in _bands else _VAZIA_JSON


def band_columns(name: str) -> int | None:
    """Largura da coluna da conversa no pane, em células, como o plugin mediu."""
    with _lock:
        return _bands.get(name, (0, _VAZIA, ""))[1]["columns"]


def transcript_columns(name: str) -> int | None:
    """Onde cortar as linhas do pane para a prévia: só com painel ancorado ao lado da conversa."""
    with _lock:
        dados = _bands.get(name, (0, _VAZIA, ""))[1]
    return dados["columns"] if any(p.get("placement") == "dock" for p in dados["panes"]) else None


async def esperar_faixa(name: str, vista: int, timeout: float) -> int:
    """Dorme até a faixa sair da versão `vista`, ou até `timeout`; devolve a versão atual."""
    await _esperar_ate(_band_wakers, name, lambda: band(name)[0] != vista, timeout)
    return band(name)[0]


# Avisos (`$.ui.toast`) dos mods, por sessão: (número, quando vence, dado). O terminal os desenha por
# alguns segundos e eles não entram no transcript; ficam aqui até vencer, para o app que abre ou
# reconecta nesse intervalo ainda mostrar o que está na tela do terminal.
_toasts: dict[str, list[tuple[int, float, dict]]] = {}
_toast_seq = 0
_toast_wakers: dict[str, tuple[asyncio.AbstractEventLoop, asyncio.Event]] = {}
# O número recomeça com o backend: o prefixo impede o app de tomar um aviso novo por um já mostrado.
_TOAST_BOOT = secrets.token_hex(4)
TOAST_DEFAULT_MS = 4000
TOAST_MAX_MS = 5 * 60 * 1000
TOAST_MAX_CHARS = 2000
TOASTS_KEPT = 20


def _store_toast(name: str, text: str, timeout_ms: float | None, plugin: str | None) -> None:
    global _toast_seq
    ms = min(max(timeout_ms or TOAST_DEFAULT_MS, 1000), TOAST_MAX_MS)
    now = time.monotonic()
    with _lock:
        _toast_seq += 1
        toast = {"id": f"{_TOAST_BOOT}-{_toast_seq}", "text": text[:TOAST_MAX_CHARS], "plugin": (plugin or "")[:64]}
        alive = [t for t in _toasts.get(name, []) if t[1] > now]
        _toasts[name] = [*alive, (_toast_seq, now + ms / 1000, toast)][-TOASTS_KEPT:]
    _acordar_todos(_toast_wakers, name)


def toasts_after(name: str, seen: int) -> tuple[int, list[dict]]:
    """Os avisos ainda não vencidos de número maior que `seen`, com o tempo que resta a cada um, e
    o número do último aviso da sessão."""
    now = time.monotonic()
    with _lock:
        stored = _toasts.get(name, [])
        # O 0 seria "sem prazo" para o app: quem está no último milissegundo ainda leva 1.
        fresh = [{**toast, "timeoutMs": max(1, int((expires - now) * 1000))}
                 for seq, expires, toast in stored if seq > seen and expires > now]
    return (stored[-1][0] if stored else seen), fresh


async def wait_toasts(name: str, seen: int, timeout: float) -> tuple[int, list[dict]]:
    """Dorme até a sessão ter aviso vivo de número maior que `seen`, ou até `timeout`."""
    await _esperar_ate(_toast_wakers, name, lambda: toasts_after(name, seen)[1], timeout)
    return toasts_after(name, seen)


# Trecho da âncora: o terminal corta a linha da faixa com reticências quando o pane é estreito.
_ANCHOR_CHARS = 16


def band_anchor(name: str) -> str | None:
    """O começo do primeiro texto legível da faixa, como ele aparece no pane.

    É por ele que a prévia sabe onde a conversa acaba: a faixa fica entre a última resposta e a
    caixa de digitar, e um mod que começa a linha com ● seria lido como prosa em andamento."""
    tree = band(name)[1]["above"]
    pilha: list = [tree]
    while pilha:
        no = pilha.pop()
        if isinstance(no, str):
            texto = no.strip()
            if sum(c.isalnum() for c in texto) >= 3:
                return texto[:_ANCHOR_CHARS]
        elif isinstance(no, dict):
            pilha.extend(reversed(no.get("children") or []))
            # Botão e link desenham o `label` antes dos filhos: uma faixa que começa por botão
            # começa por ele.
            rotulo = (no.get("props") or {}).get("label")
            if isinstance(rotulo, str):
                pilha.append(rotulo)
    return None


def plugin_facts(name: str) -> dict:
    """Os fatos desta sessão para o `state_facts`: o Python não recebe mais estado, pergunta nem
    sugestão do plugin (são do Rust); só a faixa dos mods ainda passa por aqui."""
    return {"plugin_state": None, "waiter_open": False, "heartbeat_age_ms": None, "question": None,
            "suggestion": "", "body_columns": transcript_columns(name), "band_anchor": band_anchor(name)}


def tracked_session_id(name: str) -> str | None:
    """O uuid da conversa que o Hangar acompanha nesta sessão; None quando o vínculo é só palpite."""
    transcript = _tracked_transcript(name)
    return transcript.stem if transcript else None


def _tracked_transcript(name: str) -> Path | None:
    """Caminho confirmado pelo registro, nunca fornecido pelo plugin."""
    from app import tmux
    from app.api import registry
    from app.procinfo import _proc_children_map
    from app.registry import SessionRegistry
    # cwd do pane do agente, como no `list()`: no Windows a pasta do projeto sai dele, e um split
    # ativo noutra pasta apontaria para outro transcript.
    panes = tmux.list_panes_all().get(name)
    cwd = SessionRegistry._agent_pane(panes, _proc_children_map())["cwd"] if panes else ""
    jsonl, tracked = registry.resolve_tracked(name, cwd)
    return Path(jsonl) if jsonl and tracked else None


# O plugin já corta árvore acima de 256 KB; a folga cobre sessão e token no mesmo corpo.
MAX_BAND_BYTES = 300 * 1024


class BandPane(BaseModel):
    id: str = Field(min_length=1, max_length=64)
    title: str = ""
    placement: str = "inline"
    columns: int | None = None
    tree: dict | None = None


class BandBody(BaseModel):
    sessao: str
    token: str
    above: dict | None = None
    columns: int | None = None
    panes: list[BandPane] = Field(default_factory=list)


@plugin_router.post("/ui")
async def ui(body: BandBody, request: Request):
    """A faixa acima do prompt e os painéis, como o plugin os recebeu de todos os mods, só quando mudam.

    O Hangar não interpreta as árvores: elas vão como vieram para o app, que desenha os elementos do
    Claude Code sem saber de que mod vieram."""
    if int(request.headers.get("content-length") or 0) > MAX_BAND_BYTES:
        raise HTTPException(413, detail="faixa grande demais")
    _entra(body)
    _guardar_faixa(body.sessao, body.above, body.columns, [p.model_dump() for p in body.panes])
    return {"ok": True}


class ToastBody(BaseModel):
    sessao: str
    token: str
    text: str
    timeoutMs: float | None = None
    plugin: str | None = None


@plugin_router.post("/toast")
async def toast(body: ToastBody):
    """Um mod mostrou um aviso (`$.ui.toast`) no terminal: vai ao app, que o mostra pelo mesmo tempo.

    Texto e nome longos são cortados em vez de recusados: recusar faria o aviso sumir do app."""
    _entra(body)
    if body.text.strip():
        _store_toast(body.sessao, body.text, body.timeoutMs, body.plugin)
    return {"ok": True}


class PressBody(BaseModel):
    sessao: str
    token: str
    requestId: str = Field(min_length=1, max_length=64)
    element: str = Field(min_length=1, max_length=256)


@plugin_router.post("/pressed")
async def pressed(body: PressBody):
    """Um botão de mod foi pressionado no terminal. O clique do app é do Rust: aqui não há quem espere."""
    _entra(body)
    return {"ok": True}


class CopiedBody(BaseModel):
    sessao: str
    token: str
    attempt: str = Field(min_length=1, max_length=64)
    text: str = Field(max_length=65536)


@plugin_router.post("/copied")
async def copied(body: CopiedBody):
    """Um mod copiou um texto num clique do app. O clique do app é do Rust: o 409 faz a cópia acontecer no
    terminal, em vez de sumir."""
    _entra(body)
    raise HTTPException(409, detail="clique do app já respondido")


@plugin_router.post("/press-start")
async def press_start(body: PressBody):
    """O press que começou no terminal é o clique que o app pediu? Aqui nunca: o clique do app é do Rust."""
    _entra(body)
    return {"fromApp": False, "attempt": None}


class OpenedBody(BaseModel):
    sessao: str
    token: str
    attempt: str = Field(min_length=1, max_length=64)
    url: str = Field(max_length=8192)


@plugin_router.post("/opened")
async def opened(body: OpenedBody):
    """Um mod mandou abrir uma URL num clique do app: o app abre no aparelho de quem clicou."""
    _entra(body)
    if not re.match(r"^https?://", body.url, re.IGNORECASE):
        raise HTTPException(400, detail="só http(s)")
    raise HTTPException(409, detail="clique do app já respondido")
