"""Caminho NATIVO de entrada nas sessões Claude: o function-hook do plugin `hangar`.

O que muda em relação ao pane: o texto não é digitado. O plugin mantém um
long-poll aberto aqui e entrega por `$.prompt.submit`, a mesma chamada que o
engine faz para um prompt digitado. A fila durável, a poda e o claim continuam
sendo do `pqueue`/`terminal_input.drain` — aqui é só o transporte.

Fallback é por AUSÊNCIA, não por erro: sem um long-poll vivo para a sessão,
`aguardando()` é falso e o `drain` segue pelo caminho de tecla de sempre. Vale
para toda sessão que não é Claude, para a que nasceu antes do flag e para a
que carregou o plugin e ele morreu.

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

# Quanto o backend segura o long-poll antes de responder 204. Curto o bastante
# para a morte da sessão aparecer, longo o bastante para a espera não virar poll.
ESPERA_S = 25.0
# Entre dois long-polls o hook volta em milissegundos (2 s se o backend falhar). Sem long-poll aberto
# por mais que isto, o hook morreu: o Esc no terminal interrompe a ferramenta sem o `/ask-fim`.
SEM_POLL_S = 5.0
# O hook que ainda faz long-poll depois disso sobreviveu ao Esc do app (o diálogo ficou).
HOOK_VIVO_S = 2.0

_lock = threading.Lock()
_waiters: dict[str, asyncio.Queue] = {}
_loop: asyncio.AbstractEventLoop | None = None
# Backend parando: o uvicorn espera cada pedido aberto antes do lifespan, e uma espera de 25 s
# passava do teto do systemd (SIGKILL). As esperas respondem vazio, como na janela que fecha.
_stopping = False
_STOP = object()


def stop_waits() -> None:
    """Chamado do tratador de sinal: só agenda, porque o sinal pode chegar com `_lock` tomado."""
    global _stopping
    _stopping = True        # já aqui: entrega nova cai no pane em vez de numa espera que vai fechar
    loop = _loop
    if loop is not None:
        try:
            loop.call_soon_threadsafe(_release_waits)
        except RuntimeError:
            pass        # loop já fechado: não há espera viva


def _release_waits() -> None:
    with _lock:
        queues = [*_waiters.values(), *(p["fila"] for p in _perguntas.values() if p.get("fila"))]
    for queue in queues:
        queue.put_nowait(_STOP)

# Dono do long-poll por sessão: (instância, modos declarados, última batida). Um segundo `claude` com
# o mesmo nome ou pane não pode tomar a fila do primeiro.
_donos: dict[str, tuple[str, set[str], float]] = {}
_publications: dict[str, dict] = {}


def publish_terminal(name, conversation, generation, publication, validate):
    """Publica uma vez; aviso perdido conserva a possibilidade de escrita."""
    if publication.get("mode") not in {"fill", "user", "focus"} or not isinstance(publication.get("text"), str):
        raise ValueError("publicação inválida")
    validate()
    if tracked_session_id(name) != conversation:
        return "not_written"
    with _lock:
        queue, loop = _waiters.get(name), _loop
        modes = (_donos.get(name) or (None, set(), 0))[1]
        if queue is not None and ("receipt_v2" not in modes or publication["mode"] == "focus" and "focus" not in modes):
            return "not_written"
        if queue is None or loop is None:
            return "unavailable"
        if name in _publications:
            if _publications[name]['conversation'] == conversation:
                return "unknown"
            del _publications[name]
        pending = {"id":publication["id"], "conversation":conversation, "generation":generation,
            "mode":publication["mode"], "event":threading.Event(), "result":"unknown"}
        _publications[name] = pending
    def enqueue():
        try:
            validate()
            if tracked_session_id(name) != conversation:
                raise RuntimeError("conversa mudou antes da publicação")
            with _lock:
                if _waiters.get(name) is not queue:
                    # A espera saiu (queda de quem a repassava, ou o prazo) entre a escolha e agora.
                    raise RuntimeError("espera do plugin encerrada antes da publicação")
            queue.put_nowait({"text":publication["text"], "modo":publication["mode"],
                "publication_id":publication["id"], "generation":generation, "session_id":conversation})
        except Exception:
            pending["result"] = "not_written"
            pending["event"].set()
    try:
        try:
            loop.call_soon_threadsafe(enqueue)
        except RuntimeError:
            return "not_written"
        pending["event"].wait(PUBLICA_FOCO_S if publication["mode"] == "focus" else PUBLICA_S)
        return pending["result"]
    finally:
        with _lock:
            if _publications.get(name) is pending:
                # Foco devolvido tarde não deixa entrada incerta: não segura a publicação seguinte.
                if pending["result"] != "unknown" or pending["mode"] == "focus":
                    del _publications[name]
                else:
                    pending["returned"] = True


def declared_modes(name: str) -> set[str]:
    with _lock:
        dono = _donos.get(name)
    return set(dono[1]) if dono else set()


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
        _waiters.pop(name, None)
        _donos.pop(name, None)
        _estados.pop(name, None)
        _perguntas.pop(name, None)
        _batidas.pop(name, None)
        _fechadas.pop(name, None)
        _sugestoes.pop(name, None)
        _bands.pop(name, None)
        _toasts.pop(name, None)
        _confirmacoes.pop(name, None)
        _preenchido.pop(name, None)
    _eventos.pop(name, None)
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
        # Uma vez por chave: o `/pull` volta a cada 25 s. Sessão fora do runtime cai aqui também.
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


def aguardando(name: str) -> bool:
    """Há um long-poll ABERTO para esta sessão agora?

    É o gate do caminho feliz. Falso enquanto o plugin processa uma entrega
    anterior — e isso é de propósito: nesse instante ele não pode receber outra,
    e a fila espera o próximo poll em vez de cair para a tecla no meio do caminho.
    """
    with _lock:
        return name in _waiters


# Como o texto entra na sessão:
#  - `submit`: `$.prompt.submit`, sem tecla nenhuma. O engine carimba a origem
#    `plugin` e ENVOLVE a mensagem numa moldura ("The hangar plugin sent a
#    message: …") que nenhum hook consegue tirar — medido, o engine recusa.
#  - `fill`: `$.prompt.fill` põe o rascunho no composer e o Hangar manda SÓ o
#    Enter pelo tmux. A origem vira a do usuário, sem moldura, e o `send-keys`
#    deixa de digitar o texto — a parte que hoje fatia, espera e às vezes corta.
MODO_PADRAO = "fill"

# `user`: `$.prompt.submit({text, asUser: true})` entra como fala da pessoa, sem tecla nenhuma. Só com
# a sessão parada (no turno a promessa espera o fim), só se o plugin dono declarou o modo (plugin
# velho trataria como `submit` com moldura) e só com texto que o modo não estraga: `@arquivo` não é
# expandido, `!` é modo bash digitado e `/` é menu da TUI.
_MENCAO = re.compile(r"(^|\s)@\S")
PROVA_TRANSCRIPT_S = 10.0
# Sem confirmação nem prova (transcript ilegível ou texto repetido): nem entregue, nem livre p/ tecla.
INCERTO = "incerto"


def choose_mode(name: str, text: str) -> str:
    recente = estado_recente(name)
    if (mods_by_default() and "user" in declared_modes(name) and recente is not None
            and recente[0] == "idle" and not _MENCAO.search(text)
            and not text.lstrip().startswith(("!", "/"))):
        return "user"
    return MODO_PADRAO


def _linhas_do_usuario(jsonl: str | None) -> set[str] | None:
    if not jsonl:
        return None
    from app import pqueue
    return pqueue.committed_user_lines(jsonl)


def _prova_user(aviso: threading.Event, texto: str, jsonl: str | None, antes: set[str] | None):
    """Entrega `user`: vale o que vier primeiro, o aviso do plugin (que só chega depois dos hooks
    do UserPromptSubmit) ou o texto no transcript. Devolve "aviso", True (transcript), False ou
    `INCERTO`."""
    alvo = texto.strip()
    inicio = time.monotonic()
    limite = inicio + CONFIRMA_S + PROVA_TRANSCRIPT_S
    # A conferência é por conjunto: texto que já estava lá não prova esta entrega. Sem a foto de
    # antes, o transcript só vale depois do prazo do aviso, como era.
    repetido = antes is not None and alvo in antes
    transcript_desde = inicio + (CONFIRMA_S if antes is None else 0.0)
    lido: set[str] | None = None
    while True:
        agora = time.monotonic()
        if not repetido and agora >= transcript_desde:
            lido = _linhas_do_usuario(jsonl)
            if lido is not None and alvo in lido:
                return True
        if aviso.wait(max(0.0, min(0.15, limite - agora))):
            return "aviso"
        if time.monotonic() >= limite:
            # Repetido e legível também é incerto: o texto está lá, mas pode ser o de antes.
            if repetido or lido is None:
                return INCERTO
            # O texto pode ter chegado durante a última espera: sem reler, seria digitado de novo.
            lido = _linhas_do_usuario(jsonl)
            if lido is None:
                return INCERTO
            return alvo in lido

# Teto da espera pelo aviso de que o rascunho entrou. Passou disso, o Enter NÃO
# é enviado: apertar Enter num composer que não recebeu o texto submete o que
# estiver lá — ou nada.
CONFIRMA_S = 5.0
# Espera do aviso de uma publicação do Rust no terminal. No modo `user` o aviso só sai depois dos
# hooks do UserPromptSubmit, que com a máquina ocupada passam de 5 s. Fica abaixo do teto da
# política `terminal_publish` no Rust (`PUBLISH_POLICY_TIMEOUT`).
PUBLICA_S = 30.0
# A devolução do foco é um `fill` do plugin, de milissegundos; acima disso o clique e a fila seguem pelo
# `ctrl+x tab`, e a limpeza do clique tem 2 s ao todo.
PUBLICA_FOCO_S = 1.0

_confirmacoes: dict[str, threading.Event] = {}
_preenchido: dict[str, bool] = {}

# Último estado que o plugin anunciou, por sessão: (momento, estado, motivo).
_estados: dict[str, tuple[float, str, str | None]] = {}

# A frase que a TUI propõe depois do turno. Não há evento de DESCARTE — se a pessoa digita por
# cima, nada avisa —, então quem a apaga é o começo do turno seguinte.
_sugestoes: dict[str, str] = {}


def sugestao(name: str) -> str:
    """A sugestão viva desta sessão, ou string vazia."""
    with _lock:
        return _sugestoes.get(name, "")


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

# Depois disso a leitura do pane volta a mandar sozinha. O plugin não repete estado — ele avisa
# transição —, então o prazo cobre uma sessão parada em `idle` por horas: o que expira aqui é a
# CONFIANÇA de que o plugin ainda está vivo, e quem a renova é a batida do long-poll.
VALIDADE_ESTADO_S = 90.0


# Quantos SSE do app (lista ou conversa) estão abertos agora. Pedido de permissão só é segurado
# pelo plugin com alguém no app para responder.
_apps_abertos = 0


def app_entrou() -> None:
    global _apps_abertos
    with _lock:
        _apps_abertos += 1


def app_saiu() -> None:
    global _apps_abertos
    with _lock:
        _apps_abertos = max(0, _apps_abertos - 1)


def app_presente() -> bool:
    with _lock:
        return _apps_abertos > 0


# Última batida do long-poll de entrada, por sessão: é o pulso que diz que o plugin está vivo.
_batidas: dict[str, float] = {}
# Um por sessão, criado por quem espera (o monitor de estado). Aviso do plugin acorda o monitor na
# hora, em vez de a transição esperar o próximo tique do pane.
_eventos: dict[str, asyncio.Event] = {}


def vivo(name: str) -> bool:
    """O plugin desta sessão bateu aqui há pouco?"""
    with _lock:
        if name in _waiters:
            return True
        quando = _batidas.get(name)
    return quando is not None and time.monotonic() - quando < ESPERA_S + 10


async def esperar_evento(name: str, timeout: float) -> None:
    """Dorme até o plugin avisar algo desta sessão, ou até `timeout`. Só no loop do servidor."""
    ev = _eventos.setdefault(name, asyncio.Event())
    try:
        await asyncio.wait_for(ev.wait(), timeout)
    except asyncio.TimeoutError:
        pass
    ev.clear()


def _acordar(name: str) -> None:
    ev = _eventos.get(name)
    if ev is not None:
        ev.set()


def plugin_facts(name: str) -> dict:
    """O que o plugin disse desta sessão, para o `Monitor` do Rust, sem as validades: elas vão como
    idade em ms e o Rust as aplica com o relógio dele (`state_facts`)."""
    agora = time.monotonic()

    def idade(quando: float) -> int:
        return max(0, round((agora - quando) * 1000))

    with _lock:
        hit = _estados.get(name)
        batida = _batidas.get(name)
        p = _perguntas.get(name)
        out = {
            "plugin_state": None if hit is None else {"state": hit[1], "reason": hit[2], "age_ms": idade(hit[0])},
            "waiter_open": name in _waiters,
            "heartbeat_age_ms": None if batida is None else idade(batida),
            "question": None if p is None else {"id": p["id"], "questions": p["questions"], "tool": p.get("tool"),
                                                "resumo": p.get("resumo"), "seen_age_ms": idade(p["visto"])},
            "suggestion": _sugestoes.get(name, ""),
        }
    out["body_columns"] = transcript_columns(name)
    out["band_anchor"] = band_anchor(name)
    return out


def estado_recente(name: str) -> tuple[str, str | None] | None:
    """O estado anunciado pelo plugin, se ainda válido. None = o pane que decida."""
    with _lock:
        hit = _estados.get(name)
        vivo = name in _waiters
    if hit is None:
        return None
    quando, estado, motivo = hit
    if not vivo and time.monotonic() - quando > VALIDADE_ESTADO_S:
        return None
    return estado, motivo


def entregar(name: str, texto: str, modo: str = MODO_PADRAO, jsonl: str | None = None):
    """Passa o texto ao long-poll da sessão. True = entregue; False = ninguém entregou (use o pane);
    `INCERTO` = pode ter entrado e não dá para provar (não digite; a reconciliação decide).

    Chamado de dentro do `drain`/`_send_one`, que rodam em thread: o Queue é do
    loop do FastAPI, então a entrega atravessa por `call_soon_threadsafe`.

    Roda inteiro sob o `_send_lock` da sessão: o Enter, a conferência e a limpeza tocam o mesmo
    composer que o `send_prompt`, e a confirmação do rascunho é UMA por sessão — duas entregas
    juntas roubariam o aviso uma da outra.
    """
    from app import terminal_input
    from app.conversation_transfer import transfer_active
    if transfer_active(name):
        return False
    from app.runtime_terminal import assert_writer
    assert_writer(name)
    with terminal_input._send_lock(name):
        if transfer_active(name):
            return False
        return _entregar(name, texto, modo, jsonl)


def _entregar(name: str, texto: str, modo: str, jsonl: str | None = None):
    with _lock:
        fila = _waiters.get(name)
        loop = _loop
    if fila is None or loop is None or _stopping:
        return False
    aviso = threading.Event()
    if modo in ("fill", "user"):
        with _lock:
            _confirmacoes[name] = aviso
            _preenchido.pop(name, None)
    # A foto sai ANTES de o plugin receber o texto: depois, não dá para separar o novo do repetido.
    antes = _linhas_do_usuario(jsonl) if modo == "user" else None
    try:
        loop.call_soon_threadsafe(fila.put_nowait, {"text": texto, "modo": modo})
    except RuntimeError:
        # Loop morrendo (shutdown): não é entrega, e o caller tem o pane.
        return False
    if modo not in ("fill", "user"):
        return True
    if modo == "user":
        prova = _prova_user(aviso, texto, jsonl, antes)
        with _lock:
            ok = _preenchido.pop(name, False)
            _confirmacoes.pop(name, None)
        if prova == "aviso":
            return ok
        if prova is not True:
            _log.warning("plugin %s: envio sem confirmação nem prova no transcript — resultado=%s",
                         name, prova)
        return prova
    confirmou = aviso.wait(CONFIRMA_S)
    with _lock:
        ok = _preenchido.pop(name, False)
        _confirmacoes.pop(name, None)
    if not confirmou:
        _log.warning("plugin %s: rascunho sem confirmação em %.0fs — sem Enter", name, CONFIRMA_S)
    from app import terminal_input, tmux
    if not confirmou or not ok:
        # O rascunho pode ter entrado mesmo sem o aviso chegar. Quem assume daqui é o caminho de
        # tecla, que digitaria EM CIMA do resíduo e concatenaria as duas coisas — o mesmo estrago
        # que o tratamento de envio parcial existe pra evitar. Limpa antes de devolver.
        terminal_input._limpar_composer(name, texto, None)
        return False
    if not tmux.send_keys(name, "Enter"):
        terminal_input._limpar_composer(name, texto, None)
        return False
    # O Enter SAIR não é o mesmo que a mensagem ENTRAR: o composer pode ter perdido o foco, ou um
    # overlay pode ter subido entre o rascunho e a tecla. Sem esta conferência a entrada era marcada
    # como entregue com o texto parado no campo — mensagem perdida do ponto de vista da fila, que é
    # o pior defeito possível aqui. Mesma prova que o caminho de tecla já usava.
    if not terminal_input._submeteu(name, texto):
        _log.warning("plugin %s: Enter não submeteu — devolvendo pro caminho de tecla", name)
        terminal_input._limpar_composer(name, texto, None)
        return False
    return True


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


# Pergunta de múltipla escolha que o hook de `tool.call` segura: o diálogo do terminal e o app
# correm juntos, e a entrada sobrevive ao intervalo entre dois long-polls (só a fila troca).
_perguntas: dict[str, dict] = {}


def interrompeu(name: str, id: str | None) -> None:
    """O Esc do app fecha o diálogo `id` (lido antes do Esc) no terminal, e o hook morre sem
    `/ask-fim` deixando o long-poll aberto até a janela fechar. A pergunta interrompida deixa de
    contar na hora e o long-poll é acordado para terminar; hook que ainda pergunta depois de
    `HOOK_VIVO_S` sobreviveu ao Esc e a pergunta volta a contar."""
    with _lock:
        p = _perguntas.get(name)
        if p is None or id is None or p["id"] != id:
            return
        p["interrompida"] = time.monotonic()
        fila = p.get("fila")
    if fila is not None:
        fila.put_nowait({"answers": None})


def pergunta_pendente(name: str) -> dict | None:
    """A pergunta que o plugin segura agora (`id`, `questions`), ou None.

    Só vale com o long-poll batendo: hook que morreu não pode segurar a resposta do app."""
    with _lock:
        p = _perguntas.get(name)
        idade = time.monotonic() - p["visto"] if p is not None else 0
        if p is None or p.get("interrompida") or idade > ESPERA_S + 10 or not p.get("fila") and idade > SEM_POLL_S:
            return None
        return {"id": p["id"], "questions": p["questions"], "tool": p.get("tool"),
                "resumo": p.get("resumo")}


# Última pergunta fechada por sessão: (id, quem fechou). Resposta repetida (toque duplo, retry do
# cliente) para uma pergunta que o APP já fechou é entrega feita — cair na tecla a mandaria de novo.
_fechadas: dict[str, tuple[str, str]] = {}


def responder_pergunta(name: str, corpo: dict, id: str | None = None, receipt: dict | None = None) -> bool:
    """Entrega a resposta do app ao hook. False = ele não pegou; quem chama cai na tecla.

    `id` é a pergunta que quem chama leu em `pergunta_pendente`: outra pergunta no lugar não recebe
    esta resposta."""
    with _lock:
        p = _perguntas.get(name)
        loop = _loop
        if p is None or loop is None or (id is not None and p["id"] != id):
            return receipt is None and id is not None and _fechadas.get(name) == (id, "app")
        if receipt is not None:
            if p.get("publication") is not None and p["publication"] != receipt:
                return False
            p["publication"] = dict(receipt)
            corpo = {**corpo, **receipt}
        aviso = p.get("aviso")
        primeiro = aviso is None
        fila = None
        if primeiro:
            aviso = p["aviso"] = threading.Event()
            fila = p.get("fila")
            if fila is None:
                p["resposta"] = corpo
    if fila is not None:
        try:
            loop.call_soon_threadsafe(fila.put_nowait, corpo)
        except RuntimeError:
            return False
    # Segunda resposta com a primeira em voo espera o MESMO aviso: uma pergunta, um veredito.
    pegou = aviso.wait(CONFIRMA_S)
    if primeiro:
        with _lock:
            p = _perguntas.get(name)
            if p is not None and p.get("aviso") is aviso:
                p.pop("aviso", None)
                p.pop("resposta", None)
    return pegou
