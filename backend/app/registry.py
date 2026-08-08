import asyncio
import json
import logging
import os
import re
import threading
import time
import uuid
from pathlib import Path
from typing import Optional
from app import tmux
from app import agentpane
from app.config import settings
from app import runtime_config
from app.names import sanitize_session_name
from app.git_ops import git_summary, branch_of
from app.models import SessionInfo
from app.pqueue import PromptQueue
from app.chain import ThenLink
from app.pair import PairLink, rename_pair, leave as pair_leave
from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex import adapter as codex_adapter
from app.adapters.codex.appserver import AppServerClient
from app.askquestion import clear_pending_askq
from app.state import classify, _live_spinner, rate_limit_reset, status_line as _pane_status
from app.statusline import read as _sidecar_status
from app.hook_state import hook_state
from app.planprog import plan_progress, plano_escondido
# As funcoes de /proc vivem no procinfo.py — e o unico ponto do backend preso ao Linux.
# Importadas por NOME (nao `procinfo._cmdline(...)`) de proposito: os testes fazem
# monkeypatch delas neste modulo, e o binding local preserva isso.
from app import procinfo
from app.procinfo import (_proc_children_map, _descendant_pids, _open_jsonl, _cmdline,
                         _config_dir_of, _proc_start_time, _engine_of)
# _proc_environ_path/_proc_stat_path saem de proposito da lista acima: existem SO como ponto de
# injecao de teste e sao chamadas de dois lados (aqui e de dentro do procinfo). Importadas por
# nome, cada lado ganharia um binding proprio e um monkeypatch alcancaria so um deles — o outro
# leria o /proc real e o teste passaria por acidente. Qualificadas pelo modulo, ha UM alvo.

# Sentinela: distingue "pid nao informado" (resolve sozinho via tmux) de "pid=None" (sem pane).
_UNSET = object()

_log = logging.getLogger("claude_pocket.registry")


# Idade minima de um marcador awaiting_input pra que um pane raspado SEM menu o rebaixe pra idle
# (hook_state.demote_awaiting). O grace cobre a janela Notification->menu renderizado: raspar nesse
# vao nao pode matar um awaiting real que ainda nem apareceu na tela.
_AWAITING_DEMOTE_GRACE_S = 10.0

# Teto de pares (done, total) por sessao em plan_tasks. O front so segmenta a barra com <= 8 Tasks
# (PlanBar.svelte), acima disso desenha barra unica e ignora a lista. 9 e nao 8 DE PROPOSITO: cortar
# em 8 exatos faria o front achar que o plano TEM 8 Tasks e segmentar um plano de 30.
_MAX_PLAN_TASK_SEGMENTS = 9


def _decorate_loop(info) -> None:
    """Decora loop_status/iter/max de UMA sessao a partir do sidecar (app.loop). Sem loop -> tudo None
    (sem badge). Module-level (nao closure) pra ser testavel isolado."""
    from app.loop import LoopLink
    d = LoopLink(info.name).get()
    if d is not None:
        info.loop_status = d.get("status")
        info.loop_iter = d.get("iter")
        info.loop_max = d.get("max_iters")


def _decorate_plan(info) -> None:
    """Decora plan_* de UMA sessao a partir do .md do plano (app.planprog). Sem plano -> tudo None.
    Engole a excecao de proposito: roda no tick da lista, e uma falha aqui nao pode derrubar o SSE
    (incidente 2026-07-23). Module-level (nao closure) pra ser testavel isolado, igual _decorate_loop."""
    try:
        p = plan_progress(info.cwd)
    except Exception:
        _log.warning("decorate_plan falhou pra %r", getattr(info, "name", "?"), exc_info=True)
        return
    if p is None:
        # Sem plano tem DOIS motivos: nao existe, ou o usuario escondeu. So o 2o mantem o painel
        # (e o seletor que desfaz) na tela. Custo: um read do pin, so pra sessao sem plano.
        info.plan_hidden = plano_escondido(info.cwd) or None
        return
    info.plan_name = p.name
    info.plan_task = p.task_idx
    info.plan_task_total = p.task_total
    info.plan_done = p.done
    info.plan_total = p.total
    info.plan_complete = p.complete
    # Sem o corte, um plano de 30 Tasks manda 30 pares por sessao em TODO /api/sessions e em toda
    # re-emissao do SSE, de graca. Ver _MAX_PLAN_TASK_SEGMENTS acima pro porque de 9 e nao 8.
    info.plan_tasks = [(t.done, t.total) for t in p.tasks[:_MAX_PLAN_TASK_SEGMENTS]]


def sanitize_cwd(cwd: str) -> str:
    return re.sub(r"[^A-Za-z0-9]", "-", cwd)


_pretrust_lock = threading.Lock()


def _pretrust_cwd(cwd: str, config_dir: str | None) -> None:
    """Marca `hasTrustDialogAccepted=True` pra `cwd` no .claude.json (o do config_dir, senão o
    ~/.claude.json) — pré-aprova o "trust this folder?" que o Claude Code mostra no 1º acesso a uma
    pasta nova. Read-modify-write atômico (tmp+replace) sob _pretrust_lock: dois create()
    concorrentes (rodam em threads via to_thread) fariam read-modify-write no MESMO arquivo e
    last-write-wins perderia uma entrada — mesmo padrão do _append_lock (pqueue) / _LOCK (pair).
    ensure_ascii=False + indent=2: NÃO reserializa chaves com acento pra \\uXXXX nem colapsa o
    arquivo pretty-printed do usuário (reescreve o dict inteiro; preserva o formato).
    Best-effort — qualquer falha só deixa o dialog aparecer, como hoje.
    JANELA RESIDUAL: o _lock é intra-processo; se o PRÓPRIO Claude Code CLI (processo externo)
    reescrever o .claude.json entre nosso read e replace, essa escrita dele é perdida (last-write-
    wins). Janela ~ms, o CLI escreve raro, e o guard 'já confiada -> return' limita a 1x/pasta ->
    colisão rara e aceita; fechar exigiria flock que o CLI teria de respeitar (não verificável)."""
    with _pretrust_lock:
        try:
            cfg = Path(config_dir or os.environ.get("CLAUDE_CONFIG_DIR") or Path.home()) / ".claude.json"
            data = json.loads(cfg.read_text(encoding="utf-8")) if cfg.exists() else {}
            projects = data.setdefault("projects", {})
            entry = projects.setdefault(cwd, {})
            if entry.get("hasTrustDialogAccepted") is True:
                return  # já confiada -> não reescreve o arquivo (evita corrida à toa)
            entry["hasTrustDialogAccepted"] = True
            tmp = cfg.with_suffix(".json.cp-tmp")
            tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
            tmp.replace(cfg)
        except Exception as e:
            _log.warning("pretrust falhou pra %s: %r", cwd, e)



def _newest_after_clear(projdir: Path, sid_jsonl: str, exclude: set[str]) -> str:
    # /clear rola um session-id NOVO (novo .jsonl) sem alterar o --session-id do cmdline -> o jsonl do
    # cmdline congela no transcript de BOOT. Se o projeto tem um .jsonl mais recente (e nao seguro por um
    # subagente/daemon), ele e o transcript pos-clear do mesmo REPL: segue ele. Senao devolve sid_jsonl.
    # ponytail: heuristica por mtime no mesmo cwd. Teto: durante uma Task, se o subagente escrever por
    # ultimo SEM estar com fd aberto no instante (abre/escreve/fecha), pode pegar o jsonl dele num poll
    # -> transitorio, o REPL reassume ao gravar a resposta. Upgrade: o REPL marcar seu transcript ativo
    # explicitamente (ex: hook gravando o path).
    try:
        best_mt = os.path.getmtime(sid_jsonl)
    except OSError:
        # boot-id ainda nao escrito (sessao recem-criada) -> sem /clear possivel ainda; confia no
        # --session-id deterministico (o tailer segue quando o arquivo aparecer). NAO cair pro mtime aqui
        # senao um jsonl antigo do mesmo cwd venceria o transcript novo que ainda nem nasceu.
        return sid_jsonl
    best = sid_jsonl
    try:
        for f in projdir.glob("*.jsonl"):
            if os.path.realpath(str(f)) in exclude:
                continue
            try:
                mt = f.stat().st_mtime
            except OSError:
                continue
            if mt > best_mt:
                best, best_mt = str(f), mt
    except OSError:
        pass
    return best


def _marker_by_pids(config_base: Path, pids: list[int], exclude: set[str]) -> Optional[str]:
    # Marcador do hook casado por PID: o state_hook grava {jsonl, ts, cwd, pid} onde pid = o REPL
    # claude que disparou o evento. Se esse pid e DESCENDENTE deste pane, o marcador e desta sessao
    # — resolve sessao BARE (sem --session-id no cmdline) de forma deterministica, sem chute por
    # mtime. Varios marcadores casando (ex: restart do claude no mesmo pane) -> o mais recente vence.
    d = config_base / ".claude-pocket-active"
    pidset = set(pids)
    best: tuple[float, str] | None = None
    try:
        files = list(d.glob("*.json"))
    except OSError:
        return None
    for f in files:
        try:
            o = json.loads(f.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        j, pid = o.get("jsonl"), o.get("pid")
        if not j or pid not in pidset:
            continue
        if not os.path.exists(j) or os.path.realpath(j) in exclude:
            continue
        ts = float(o.get("ts") or 0.0)
        if best is None or ts > best[0]:
            best = (ts, j)
    return best[1] if best else None


def _active_marker_jsonl(config_base: Path, sid: str, exclude: set[str]) -> Optional[str]:
    # Marcador do hook (state_hook.py): <config>/.claude-pocket-active/<boot_id>.json = {"jsonl": <path>}
    # = o transcript REALMENTE ativo daquele boot_id. Sinal DETERMINISTICO pro caso resume/clear, onde
    # o <boot_id>.jsonl do cmdline NUNCA nasce (o claude escreve no <uuid> resumido) -> sem isto resolvia
    # pro path fantasma = chat vazio. So vale se o arquivo existe e nao e de um auxiliar (subagente/daemon).
    p = config_base / ".claude-pocket-active" / f"{sid}.json"
    try:
        j = json.loads(p.read_text(encoding="utf-8")).get("jsonl")
    except (OSError, ValueError):
        return None
    if not j or not os.path.exists(j) or os.path.realpath(j) in exclude:
        return None
    return j


# session-id (uuid) na linha de comando do claude: `--session-id <uuid>` / `--session-id=<uuid>` /
# `--resume <uuid>`. Este e o sinal AUTORITATIVO e ESTAVEL (vive no /proc/PID/cmdline pela vida do
# processo, inclusive em idle) -> o jsonl da sessao e <uuid>.jsonl. So casa uuid de verdade pra nao
# pescar argumento de outra flag.
_SID_RE = re.compile(
    r"--(?:session-id|resume)[ =]"
    r"([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})"
)


def _session_id_from_cmdline(cmdline: str) -> Optional[str]:
    m = _SID_RE.search(cmdline)
    return m.group(1) if m else None


def _jsonl_mtime(jsonl: Optional[str]) -> Optional[float]:
    # last_activity = mtime do transcript (epoch s). Usado pro desempate da ordenacao na lista.
    if not jsonl:
        return None
    try:
        return os.path.getmtime(jsonl)
    except OSError:
        return None



# Executavel do agente -> provider. Casa o BASENAME do argv[0], nunca a linha inteira: `pip`,
# `pipx`, `mpirun` e um caminho contendo "/pi/" nao sao o agente Pi.
_EXEC_PROVIDER = {"pi": "pi", "claude": "claude"}


def provider_of_pane(pid, children: Optional[dict[int, list[int]]] = None) -> str:
    """Qual agente roda neste pane, lido do /proc dos descendentes.

    NAO ha campo de comando no pane: tmux.list_panes_active() devolve so name/pid/cwd/pane_id, entao
    o caminho e o mesmo do _repl_sid — descer os descendentes e ler o cmdline.

    Default "claude" preserva o comportamento anterior a esta funcao existir: pane nao reconhecido
    segue tratado como Claude, em vez de sumir da lista.
    """
    if pid is None:
        return "claude"
    for p in _descendant_pids(pid, children):
        cmd = _cmdline(p)
        if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
            continue        # mesma exclusao do _repl_sid: subprocesso nao e o REPL dono
        argv0 = cmd.strip().split()[:1]   # o pi reescreve o argv -> "pi" + NUL virado espaco
        if not argv0:
            continue
        prov = _EXEC_PROVIDER.get(os.path.basename(argv0[0]))
        if prov:
            return prov
    return "claude"


# Cache pid -> (instante de inicio do processo, nome da sessao tmux). Um processo nunca muda de
# sessao (nasce dentro do pane), entao a resposta e estavel enquanto ELE vive. A chave carrega o
# start time porque o pid sozinho NAO identifica processo: reusado depois que o dono morreu (churn
# alto, pid_max baixo, container), a entrada velha seria devolvida pro processo NOVO e o recado
# apareceria vindo da sessao errada, calado — o comentario anterior afirmava que isso nao acontecia,
# e o codigo nao fazia o que ele dizia (achado da revisao). Guarda tambem o resultado vazio: sem
# isso, recado de sessao fora do tmux (um `claude -p` solto) pagaria um fork de tmux por linha.
# EXCECAO, e ela e real: onde `/proc/<pid>/stat` nao da pra ler (hidepid=2, backend sob outro uid),
# o start time e None SEMPRE e o cache — inclusive o negativo — nunca vale. Ali o fork por linha
# volta, e um `GET /history` sem limite (que reparseia o jsonl inteiro) paga um por recado. E o
# preco de nao arriscar atribuir recado a sessao errada; se doer, o conserto e cachear por
# (pid, jsonl do remetente), nao afrouxar a chave.
_NOME_POR_PID: dict[int, tuple[Optional[float], Optional[str]]] = {}


def name_of_pid(pid: int) -> Optional[str]:
    """Nome da sessao tmux dona deste pid, ou None.

    Existe pro recado nativo entre sessoes Claude (cross-session messaging): o transcript do destino
    traz `origin.verifiedPeerPid` do REMETENTE, e o app precisa do nome tmux — que e o endereco que
    o cp-send, o pareamento e a UI usam. O `origin.name` que vem junto NAO serve: e o titulo da
    sessao ("Revisar novo modo de envio no backlog"), nao o nome (medido em 07/08/2026).

    Roda no parse do transcript, que vive num `to_thread` — o fork do tmux aqui nao toca o laco de
    eventos. Falha (tmux fora, timeout) devolve None: quem chama tem fallback, e recado sem nome
    resolvido e melhor que transcript que para de ser lido.
    """
    nascimento = _proc_start_time(pid)
    # `nascimento is not None` faz parte da condicao: sem ele, um ambiente onde o /proc/<pid>/stat
    # nao da pra ler (hidepid=2, backend sob outro uid — o mesmo cenario que o inbox_socket_of ja
    # reconhece) devolve None SEMPRE, `None == None` casa, e o cache volta a ser por pid puro — o
    # bug de atribuicao errada que esta chave existe pra fechar, de volta pela porta dos fundos
    # (achado da revisao do proprio conserto). Nao saber a idade do processo = nao confiar no cache.
    if (cache := _NOME_POR_PID.get(pid)) is not None and nascimento is not None \
            and cache[0] == nascimento:
        return cache[1]
    achado: Optional[str] = None
    try:
        children = _proc_children_map()
        for nome, panes in tmux.list_panes_all().items():
            for pane in panes:
                ppid = pane.get("pid")
                if ppid and (ppid == pid or pid in _descendant_pids(ppid, children)):
                    achado = nome
                    break
            if achado:
                break
    except Exception:                                # noqa: BLE001
        _log.warning("name_of_pid(%d) falhou; recado fica com o nome do remetente", pid,
                     exc_info=True)
        return None                                  # NAO cacheia falha: a proxima tentativa retenta
    _NOME_POR_PID[pid] = (nascimento, achado)
    return achado


def inbox_socket_of(name: str) -> Optional[str]:
    """Socket de inbox do cross-session messaging desta sessao, ou None se ela nao tem.

    O Claude Code (2.1.224+) liga um socket por sessao em `$XDG_RUNTIME_DIR/cc-socks/<pid>.sock`
    (medido em 07/08/2026; o pid e o do processo `claude`). Ter o socket e o que torna a sessao
    alcancavel pelo `SendMessage` de outra — e o que o `ListAgents` de la vai listar.

    Serve pro cp-send decidir, com FATO em vez de suposicao, se o caminho nativo existe pra este
    alvo: sessao aberta antes da liberacao, sessao Codex/Pi ou sessao de outra maquina nao tem
    socket nenhum, e mandar o modelo usar `SendMessage` nesses casos seria mandar ele bater numa
    porta que nao existe.
    """
    # Gate de plataforma ANTES de qualquer coisa: `os.getuid()` nao existe no Windows, e e justamente
    # la que XDG_RUNTIME_DIR costuma faltar — o AttributeError subia direto pra rota /peer-address e
    # ela dava 500 em TODA chamada naquela plataforma (achado da revisao). O Claude Code tambem nao
    # oferece a feature no Windows nativo, entao "sem socket" e a resposta certa, nao um erro.
    if os.name != "posix":
        return None
    try:
        # O `is_dir()` tambem fica DENTRO do try: ele engole ENOENT/ENOTDIR mas RELEVANTA EACCES, e
        # um XDG_RUNTIME_DIR sem permissao (backend sob outro uid, escopo do systemd mais apertado,
        # container com namespace proprio) mandava PermissionError direto pra rota /peer-address —
        # 500 em toda chamada naquele ambiente. Mesma classe do gate de Windows acima, um passo antes
        # (achado da revisao).
        run = os.environ.get("XDG_RUNTIME_DIR") or f"/run/user/{os.getuid()}"
        socks = Path(run) / "cc-socks"
        if not socks.is_dir():
            return None                              # feature ausente/desligada nesta maquina
        pane = tmux.pane_pid(name)
        if not pane:
            return None
        for p in _descendant_pids(pane, _proc_children_map()):
            caminho = socks / f"{p}.sock"
            if caminho.exists():
                return str(caminho)
    except Exception:                                # noqa: BLE001
        _log.warning("inbox_socket_of(%r) falhou; tratando como sem socket", name, exc_info=True)
    return None


def _pi_sid_of(pid: int) -> Optional[str]:
    # CP_PI_SESSION: o uuid que o wrapper do pi injetou. Mesmo truque do _engine_of — o env do
    # processo VIVO e o registro autoritativo. Existe porque o `--session-id` some do cmdline: o pi
    # sobrescreve o proprio argv (medido na Task 0).
    if not procinfo._TEM_PROC:
        # Escapou do procinfo quando ele foi extraido: sem /proc isto abria um caminho de arquivo
        # inexistente e caia no `except OSError: return None`, perdendo o fallback CP_PI_SESSION —
        # a sessao Pi entrava na lista sem transcript, em silencio.
        return procinfo._env_psutil(pid).get("CP_PI_SESSION") or None
    try:
        with open(procinfo._proc_environ_path(pid), "rb") as fh:
            for kv in fh.read().split(b"\x00"):
                if kv.startswith(b"CP_PI_SESSION="):
                    return kv.split(b"=", 1)[1].decode("utf-8", "replace") or None
    except OSError:
        return None
    return None


def _pi_transcript_of_id(cwd: str, sid: str) -> Optional[str]:
    # Indireção pro adapter (Task 1), que sabe o slug e o glob <timestamp>_<uuid>.jsonl. Import local
    # pelo mesmo motivo do get_adapter em create(): evita qualquer ciclo se um adapter futuro vier a
    # importar daqui.
    from app.adapters import get_adapter
    return get_adapter("pi").transcript_path(cwd, sid) or None


def _pi_is_subagent(path: str) -> bool:
    # Import local pelo mesmo motivo do _pi_transcript_of_id. Quem sabe o layout no disco e o
    # adapter; aqui so se decide o que fazer com a resposta.
    from app.adapters.pi.sessions import is_subagent_transcript
    return is_subagent_transcript(path)


def _pi_root_transcript(path: str) -> Optional[str]:
    from app.adapters.pi.sessions import root_transcript
    return root_transcript(path) or None


# Panes ja avisados sobre bilhete sem frescor. list() e polled (de segundo em segundo), entao um
# warning por varredura entupiria o journal; um por pane+motivo basta pra um /proc cronicamente
# ilegivel nao ficar calado pra sempre. ponytail: set simples — o teto e o numero de panes da
# maquina, nao ha o que expirar.
_PI_TICKET_WARNED: set[tuple[str, str]] = set()


def _warn_bilhete_once(pane_id: str, motivo: str) -> None:
    if (pane_id, motivo) not in _PI_TICKET_WARNED:
        _PI_TICKET_WARNED.add((pane_id, motivo))
        _log.warning("pi: bilhete de %s recusado (%s); usando CP_PI_SESSION", pane_id, motivo)


def pi_session_file(pane_id: str, pid: Optional[int] = None,
                    cwd: str = "") -> Optional[str]:
    """Transcript de um pane Pi: bilhete da extensao primeiro, env do wrapper depois.

    Nenhum dos dois presente -> None, e a sessao entra na lista SEM transcript. Chutar o arquivo
    mais novo do cwd (o que resolve_jsonl faz pro Claude) faria a sessao Pi abrir mostrando a
    conversa de outro agente.
    """
    base = (_config_dir_of(pid) if pid else None) or Path.home() / ".claude"
    ticket = Path(base) / ".claude-pocket-pi" / f"{pane_id.lstrip('%')}.json"
    sid = _pi_sid_of(pid) if pid else None
    try:
        data = json.loads(ticket.read_text())
        f, ts = data.get("file"), data.get("ts")
        # Bilhete de OUTRA encarnacao do pane: o tmux reusa %pane_id apos um restart do servidor e o
        # .jsonl da sessao anterior continua no disco, entao o exists() abaixo nao pega nada — o pane
        # novo abriria a conversa do pane velho. O criterio e FRESCOR, nunca "os ids divergem": a
        # extensao reescreve o bilhete a cada agent_start justamente porque /tree, /fork e troca de
        # sessao mudam o arquivo com a sessao ja rodando, enquanto o CP_PI_SESSION fica congelado no
        # /proc desde o exec. Depois de um fork a divergencia e o comportamento CORRETO; rejeitar por
        # ela devolveria a conversa anterior pelo resto da vida do pane. Bilhete escrito ANTES de o
        # processo deste pane nascer e que e de outra sessao.
        nasceu = _proc_start_time(pid) if pid else None
        # 2s de folga: o ts vem do Date.now() da extensao e o nascimento, do btime+ticks do kernel —
        # granularidades e relogios diferentes, e o bilhete do session_start nasce colado no exec.
        if nasceu is None or not isinstance(ts, (int, float)):
            # Frescor INDETERMINAVEL: /proc/<pid>/stat ilegivel (pid morto, permissao, kernel sem
            # /proc) ou bilhete sem `ts` numerico (extensao antiga, escrita parcial). Recusa, igual a
            # um bilhete velho — deixar passar era o furo silencioso: o guarda simplesmente nao
            # rodava e o pane reusado abria a conversa da encarnacao ANTERIOR, tracked=True e sem
            # nenhum rastro. Sem frescor o CP_PI_SESSION e o unico sinal que ainda prova de quem e
            # o pane (vem do /proc do processo VIVO).
            _warn_bilhete_once(pane_id, "nascimento" if nasceu is None else "ts")
            f = None
        elif ts < nasceu - 2:
            f = None
        elif f and _pi_is_subagent(f):
            # O Pi dispara `agent_start` TAMBEM pro subagente (Task tool), com um ctx cujo
            # getSessionFile() aponta pro transcript do subagente — e o publishPane da extensao
            # reescreve o bilhete com ele. Aceitar isso trocava a conversa inteira da sessao pela do
            # subagente no app (medido 2026-07-30, numa sessão real: bilhete do pane %26 caiu em
            # `…_18e48e08-…/44bad0fb/run-2/session.jsonl`), enquanto o terminal seguia normal — ele
            # nao le o bilhete. Tratar aqui e mais forte que consertar so a extensao: pega TODA
            # sessao Pi ja de pe, sem reinstalar nem reiniciar nada. O caminho do subagente carrega
            # a raiz dentro dele, entao subimos pra ela em vez de devolver None e deixar a sessao
            # sem transcript ate o proximo turno do agente principal reescrever o bilhete.
            _warn_bilhete_once(pane_id, "subagente")
            f = _pi_root_transcript(f)
        # Bilhete FRESCO vale mesmo com o arquivo ainda inexistente: o Pi so escreve o .jsonl no 1o
        # turno, e a extensao publica o bilhete la no session_start. Exigir exists() aqui deixava
        # TODA sessao Pi recem-criada pelo app como "sem id" — sem transcript e inclicavel — ate
        # alguem digitar a primeira mensagem no terminal, que e justamente o que nao da pra fazer
        # pelo celular. O caso que o exists() guardava (bilhete orfao de uma encarnacao anterior do
        # pane, apontando pra .jsonl deletado) ja e coberto pelo teste de frescor acima, que e mais
        # forte: compara o bilhete com o nascimento DESTE processo. Mesmo contrato do Claude, cujo
        # create() tambem fixa um caminho que so passa a existir depois.
        if f:
            return f
    except (OSError, ValueError):
        pass
    return _pi_transcript_of_id(cwd, sid) if sid else None


# Cadencia do cache de statusline da lista (list_with_state): TTL por sessao + teto de capturas de
# pane por chamada (o custo real e o fork do tmux).
_STATUS_TTL = 20.0
_STATUS_BUDGET = 2


class KillFailed(Exception):
    """A sessao continuou de pe depois do kill. Existe pra a rota DELETE reportar em vez de responder
    {"ok": true} e a UI sumir com um card de sessao que segue viva (ver SessionRegistry.kill)."""

    def __init__(self, name: str):
        super().__init__(f"a sessao '{name}' continua de pe depois do kill-session")
        self.name = name


class SessionRegistry:
    # Cache name -> ultimo jsonl resolvido por sinal CONFIAVEL (cmdline --session-id / fd). De classe
    # (compartilhado entre instancias: api.registry e sse._registry). Estabiliza a resolucao quando o
    # processo que carrega o --session-id SOME transitoriamente (a sessao dirigida por job/harness
    # spawna claude por turno) -> sem isto a resolucao oscilava pro mtime e o watcher do SSE limpava o
    # chat. Atualizado quando um sinal confiavel reaparece (ex: /clear -> session-id novo).
    _jsonl_cache: dict[str, str] = {}
    # nomes cujo cache veio do fd ABERTO (verdade do FS, nao chute). Mantido entre polls sem fd p/ nao
    # oscilar pro --session-id da cmdline (resume: o id da cmdline nunca vira arquivo). De classe.
    _fd_locked: set[str] = set()
    # DIAG: ultima resolucao logada por nome ("<jsonl>|<tracked>") -> loga so quando MUDA (o momento do
    # split/cross-wire), sem spammar a cada poll. Remover quando o bug de colisao estiver resolvido.
    _last_res: dict[str, str] = {}
    # Statusline por sessao: name -> (monotonic da captura, linha crua ou None). De classe
    # (compartilhado entre api.registry e as instancias do sse) — uma captura serve todos.
    _status_cache: dict[str, tuple[float, Optional[str]]] = {}
    # Texto do spinner ("Hyperspacing… (1m51s · ↓2.1k tokens)") extraido da MESMA captura do sweep:
    # o fast-path de marcador deixa label=None e o card nunca mostrava a barrinha de "trabalhando".
    _label_cache: dict[str, Optional[str]] = {}
    # Nomes ja avisados por _agent_pane (Task 5.5): sessao com 2+ panes e nenhum reconhecido como
    # agente. De classe pela MESMA razao das demais acima (list() roda em ambas instancias).
    _SEM_AGENTE_AVISADAS: set[str] = set()

    def __init__(self, projects_dir: Path | None = None):
        self.projects_dir = Path(projects_dir or settings.projects_dir)

    def resolve_jsonl(self, cwd: str, projects_dir: Path | None = None) -> Optional[str]:
        # FALLBACK por cwd: jsonl mais recente do dir do projeto. So usado quando nao ha --session-id
        # nem fd aberto. NAO confiavel com varias sessoes no mesmo cwd (colide) -> por isso o
        # cmdline --session-id (em resolve()) vem primeiro.
        proj = (projects_dir or self.projects_dir) / sanitize_cwd(cwd)
        if not proj.is_dir():
            return None

        def _mtime(f: Path) -> float:
            # arquivo pode sumir entre o glob e o stat (sessao encerrando) -> nao deixar OSError subir
            # ate o /api/sessions virar 500; o sumido vai pro fim da ordenacao (mtime 0).
            try:
                return f.stat().st_mtime
            except OSError:
                return 0.0

        files = sorted(proj.glob("*.jsonl"), key=_mtime, reverse=True)
        return str(files[0]) if files else None

    def _aux_open_jsonls(self, pids: list[int]) -> set[str]:
        # realpaths de jsonl que processos auxiliares (subagente --agent / daemon) seguram abertos AGORA.
        # Excluidos do "mais recente" em _newest_after_clear pra um Task em voo nao virar o transcript da
        # sessao. Best-effort: fd raramente fica aberto em idle -> set vazio na maioria dos polls.
        out: set[str] = set()
        for p in pids:
            cmd = _cmdline(p)
            if not ("daemon" in cmd or "--bg-" in cmd or "--agent" in cmd):
                continue
            cdir = _config_dir_of(p)
            j = _open_jsonl(p, (cdir / "projects") if cdir else self.projects_dir)
            if j:
                out.add(os.path.realpath(j))
        return out

    def _cwd_has_siblings(self, cwd: str) -> bool:
        # >1 sessao tmux com este MESMO cwd? Com varias, seguir o jsonl mais novo do cwd (newest-by-mtime)
        # cruza o transcript de uma sessao pra outra -> a resolucao por mtime fica ambigua. ponytail: 1
        # fork tmux por chamada; aceitavel (poucas sessoes). Fail-safe: erro -> trata como sem irmaos.
        # Task 5.5 (achado I1 da revisao): `#{pane_current_path}` e o cwd VIVO daquele pane -- um `cd`
        # manual num split muda SO o campo dele. list() agora entrega o cwd do pane do AGENTE; contar
        # so os panes ATIVOS (list_panes_active) deixava as duas pontas olhando cwds diferentes, e uma
        # sessao com split "sem irmao" caia no newest-by-mtime que esta guarda existe pra evitar --
        # exatamente o "sem id" que a Task 5.5 conserta, reaparecendo por outra porta. QUALQUER pane da
        # sessao com esse cwd conta (superset seguro: sobre-contar so empurra pro caminho <sid>.jsonl
        # direto, nunca pro mtime ambiguo).
        # Task 6: a sessao de shell ESCONDIDA nasce com o MESMO cwd do agente (new_hidden_shell usa
        # info.cwd) e SOBREVIVE reatada entre polls -- sem excluir aqui, abrir o shell uma vez faria
        # a sessao contar "irmao" pra sempre, e o resolve_tracked perderia _newest_after_clear (o
        # catch-up do /clear) na sessao pra sempre, mesmo sem NENHUMA outra sessao Claude no cwd.
        try:
            return sum(1 for panes in tmux.list_panes_all().values()
                       if not panes[0].get("hidden") and any(p.get("cwd") == cwd for p in panes)) > 1
        except Exception:
            return False

    def resolve(self, name: str, cwd: str) -> Optional[str]:
        return self.resolve_tracked(name, cwd)[0]

    def _log_change(self, name: str, jsonl: Optional[str], tracked: bool) -> None:
        # DIAG: loga a resolucao SO quando muda pra um nome (baseline no 1o poll, depois so transicoes).
        key = f"{jsonl}|{tracked}"
        if self._last_res.get(name) == key:
            return
        prev = self._last_res.get(name)
        self._last_res[name] = key
        _log.info("RESOLVE name=%s jsonl=%s tracked=%s prev=%s",
                  name, (jsonl or "").rsplit("/", 1)[-1], tracked,
                  (prev or "-").rsplit("/", 1)[-1].split("|")[0])

    def resolve_tracked(self, name: str, cwd: str, pid=_UNSET,
                        children: Optional[dict[int, list[int]]] = None) -> tuple[Optional[str], bool]:
        jsonl, tracked = self._resolve_tracked_impl(name, cwd, pid, children)
        self._log_change(name, jsonl, tracked)
        return jsonl, tracked

    def _resolve_tracked_impl(self, name: str, cwd: str, pid=_UNSET,
                        children: Optional[dict[int, list[int]]] = None) -> tuple[Optional[str], bool]:
        # Mapeia uma sessao tmux -> o jsonl CERTO + se o vinculo e CONFIAVEL (tracked).
        # tracked=True so com sinal DETERMINISTICO: --session-id do cmdline, fd aberto, ou cache
        # (semeado por um desses / pelo create()). tracked=False = chute newest-by-mtime, que COLIDE
        # com varias sessoes bare no mesmo cwd -> a UI marca "sem id" e desliga o chat (evita mostrar
        # /trocar transcript errado). Determinismo so com --session-id: o "+" do app, ou o wrapper
        # `claude --session-id <uuid>` no terminal.
        # pid/children: quando a listagem ja os tem (pane_pid em lote + mapa /proc unico), evita um fork
        # tmux e uma re-varredura do /proc por sessao. _UNSET = resolve sozinho (caminho single-session).
        if pid is _UNSET:
            pid = tmux.pane_pid(name)
        if pid is not None:
            pids = _descendant_pids(pid, children)
            # jsonls que processos AUXILIARES (subagente --agent / daemon) seguram abertos AGORA: sao
            # transcripts de outra sessao logica -> nunca devem virar o transcript do REPL principal.
            aux_open = self._aux_open_jsonls(pids)
            # 1. fd aberto do REPL = transcript REALMENTE ativo agora (mais preciso que o cmdline, que
            #    congela no boot). Vem ANTES do --session-id: apos um /clear (que rola session-id NOVO
            #    sem mexer no cmdline) o claude passa a escrever num jsonl novo -> o fd aponta pra ele.
            #    Pula os auxiliares (subagente/daemon) p/ nao pegar o transcript de um deles.
            for p in pids:
                cmd = _cmdline(p)
                if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                    continue
                cdir = _config_dir_of(p)
                j = _open_jsonl(p, (cdir / "projects") if cdir else self.projects_dir)
                if j:
                    self._jsonl_cache[name] = j
                    self._fd_locked.add(name)  # fd = verdade -> trava p/ os polls sem fd nao reverterem
                    return j, True
            # 1.5. Marcador do hook por cmdline sid: DETERMINISTICO e reescrito a cada evento -> vem
            #      ANTES do fd-lock duravel. Apos um /clear que rola transcript novo escrito em
            #      append-and-close (fd quase nunca aberto no poll) e cujo sid novo NUNCA vai pro
            #      cmdline, o fd-lock ficava preso no transcript PRE-clear e o chat nao migrava. O
            #      marcador sabe o transcript ativo do boot_id -> deixa ele destravar o cache velho.
            for p in pids:
                cmd = _cmdline(p)
                if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                    continue
                sid = _session_id_from_cmdline(cmd)
                if not sid:
                    continue
                cdir = _config_dir_of(p)
                config_base = cdir if cdir else self.projects_dir.parent
                marker = _active_marker_jsonl(config_base, sid, aux_open)
                if marker:
                    if self._jsonl_cache.get(name) != marker:
                        self._fd_locked.discard(name)  # transcript rolou (/clear|resume) -> solta o lock velho
                    self._jsonl_cache[name] = marker
                    return marker, True
            # fd AUSENTE neste instante: se ja travamos por fd (transcript REAL desta sessao, pego num
            # write anterior), MANTEM o cache. Sem isto, um resume cujo --session-id da cmdline nunca
            # vira arquivo oscilava fd<->id entre writes (e o watcher do SSE resetava o chat).
            if name in self._fd_locked:
                cached = self._jsonl_cache.get(name)
                if cached:
                    return cached, True
                self._fd_locked.discard(name)
            # 2. cmdline --session-id (DETERMINISTICO; app-created sempre, manual com flag). Vale mesmo
            #    sem o arquivo existir ainda (sessao recem-criada) -> o tailer segue quando aparecer.
            #    PULA os processos auxiliares da arvore do claude, que carregam um --session-id PROPRIO
            #    (transitorio) != o do REPL principal -> sem isto resolvia pro jsonl errado/inexistente:
            #      - `claude daemon` + bg-pty-host/spare (sockets em /tmp/cc-daemon-*): contem "daemon"/"--bg-"
            #      - SUB-AGENTES (`--agent`): cada Task/subagent roda seu proprio session-id.
            #    O --session-id CONGELA no boot: o /clear gera um session-id novo e o cmdline segue o
            #    velho -> _newest_after_clear segue o jsonl mais recente do projeto (= transcript pos-clear).
            for p in pids:
                cmd = _cmdline(p)
                if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                    continue
                sid = _session_id_from_cmdline(cmd)
                if sid:
                    cdir = _config_dir_of(p)
                    proj = (cdir / "projects") if cdir else self.projects_dir
                    # Marcador do hook ja tratado no passo 1.5 (antes do fd-lock). Aqui so o fallback
                    # deterministico por <sid>.jsonl / newest-after-clear quando nao ha marcador.
                    projdir = proj / sanitize_cwd(cwd)
                    sid_jsonl = str(projdir / f"{sid}.jsonl")
                    # _newest_after_clear (segue o jsonl mais NOVO do cwd pra pegar o pos-/clear) so e
                    # seguro com UMA sessao na pasta. Com VARIAS sessoes no mesmo cwd, o jsonl mais novo
                    # de uma (ex: resume/clear) CONTAMINA as outras (vira o transcript delas). Nesse caso
                    # usa o <id>.jsonl DIRETO; o fd (passo 1) ainda corrige /clear+resume da PROPRIA
                    # sessao quando pega o arquivo aberto no write.
                    if self._cwd_has_siblings(cwd):
                        j = sid_jsonl
                    else:
                        j = _newest_after_clear(projdir, sid_jsonl, aux_open)
                    self._jsonl_cache[name] = j
                    return j, True
            # 2.5. Marcador do hook casado por PID (sessao BARE: `claude` sem --session-id, nada no
            #      cmdline). O state_hook grava o pid do REPL no marcador; se ele e descendente deste
            #      pane, o transcript e desta sessao — DETERMINISTICO, vira tracked (o chat liga).
            #      Cobre tambem resume feito por fora. So nao existe marcador antes do 1o evento de
            #      hook da sessao -> cai nos passos seguintes ate o 1o prompt.
            cdir_m = _config_dir_of(pid)
            config_base_m = cdir_m if cdir_m else self.projects_dir.parent
            marker = _marker_by_pids(config_base_m, pids, aux_open)
            if marker:
                self._jsonl_cache[name] = marker
                return marker, True
        # 3. cache: ultimo sinal confiavel. Estabiliza quando o processo com --session-id some
        #    transitoriamente (senao a resolucao oscilava pro mtime e o watcher limpava o chat).
        cached = self._jsonl_cache.get(name)
        if cached:
            return cached, True
        # 4. fallback: mais recente por mtime (ambiguo com varias sessoes bare no mesmo cwd) -> NAO tracked.
        # usa o config dir da sessao (lido do pane pid, herdado pela arvore) pra achar o jsonl certo
        # quando a sessao roda num config dir != o do backend. ponytail: le do pane pid; se um alias
        # setasse CLAUDE_CONFIG_DIR so no exec do claude (nao exportado), cairia no dir do backend.
        cdir = _config_dir_of(pid) if pid is not None else None
        proj = (cdir / "projects") if cdir else self.projects_dir
        return self.resolve_jsonl(cwd, proj), False

    def _forget(self, name: str) -> None:
        self._jsonl_cache.pop(name, None)
        self._fd_locked.discard(name)
        # Nome pode ser reusado por outra sessao: sem isto a nova herdaria a statusline da morta
        # por ate _STATUS_TTL (e o dict cresceria sem poda a cada create/kill).
        self._status_cache.pop(name, None)
        self._label_cache.pop(name, None)

    def _repl_sid(self, pid, children: Optional[dict[int, list[int]]] = None) -> Optional[str]:
        # --session-id do REPL principal da sessao (pula daemon/agent). Identidade do DONO de um
        # transcript: <sid>.jsonl PERTENCE a sessao cujo cmdline traz esse sid. Usado na guarda de
        # colisao. None se ausente (REPL bare sem flag / sem pid).
        if pid is None:
            return None
        for p in _descendant_pids(pid, children):
            cmd = _cmdline(p)
            if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                continue
            sid = _session_id_from_cmdline(cmd)
            if sid:
                return sid
        return None

    def _dedupe_collisions(self, infos: list[SessionInfo], sids: dict[str, Optional[str]]) -> list[SessionInfo]:
        # 2+ sessoes resolvidas pro MESMO jsonl = colisao (uma tomou emprestado o transcript de outra
        # via marcador/fallback-mtime). So a DONA (cmdline sid == basename do jsonl) mantem; as demais
        # sao rebaixadas (jsonl=None, tracked=False) -> a UI nao duplica e o send nao rota pro terminal
        # errado. Sem dona clara (todas resumiram transcript de terceiro) -> rebaixa todas (nao arriscar
        # transcript errado pra ninguem). Roda no list() (unico ponto com a lista TODA); a resolucao
        # por-sessao segue intacta.
        groups: dict[str, list[SessionInfo]] = {}
        for info in infos:
            if info.jsonl:
                groups.setdefault(os.path.realpath(info.jsonl), []).append(info)
        for jsonl, group in groups.items():
            if len(group) < 2:
                continue
            base = os.path.basename(jsonl).removesuffix(".jsonl")
            owner = next((i for i in group if sids.get(i.name) == base), None)
            if owner is None:
                # Desempate por TRACKED. O teste acima (sid do cmdline == nome do arquivo) so acerta
                # quando o claude escreve no proprio boot_id — e numa sessao RESUMIDA ele nunca faz
                # isso: o cmdline congela no boot_id e o transcript vai pro uuid resumido. E o mesmo
                # fato que o _active_marker_jsonl ja documenta ("o <boot_id>.jsonl do cmdline NUNCA
                # nasce"), so que ali ele e tratado e aqui nao era -> owner=None e o `for` abaixo
                # rebaixava TODAS, inclusive quem tinha vinculo deterministico.
                # MEDIDO nesta maquina: grupo [jeffer1312 (sid=dea09039, jsonl=bdabe8c1, tracked),
                # probepaste (bare, sem sid, mesmo jsonl por chute de mtime)] -> nenhuma dona pelo
                # sid, as duas rebaixadas, e a sessao ATIVA ficava "sem id" na UI (o chat desligava).
                # Alternava entre as sessoes porque o newest-by-mtime que a bare reivindica e sempre
                # o transcript de quem acabou de escrever.
                # tracked=True so vem de sinal DETERMINISTICO (marcador do hook casado por pid da
                # arvore, fd aberto, ou cache semeado por um deles) — prova de propriedade mais forte
                # que o chute newest-by-mtime de uma sessao bare. Exige UNICO: com 2+ tracked no mesmo
                # jsonl nao ha dona obvia (duas resumiram o mesmo transcript), e com 0 tampouco —
                # nos dois casos segue rebaixando todas, como antes.
                tracked = [i for i in group if i.tracked]
                if len(tracked) == 1:
                    owner = tracked[0]
            for info in group:
                if info is owner:
                    continue
                _log.info("COLLISION name=%s dropped borrowed jsonl=%s owner=%s",
                          info.name, base, owner.name if owner else "none")
                info.jsonl = None
                info.tracked = False
        return infos

    @staticmethod
    def _branch_of(cwd: Optional[str]) -> Optional[str]:
        """Delega pro helper publico git_ops.branch_of (mantido pra nao quebrar chamadores)."""
        return branch_of(cwd)

    @staticmethod
    def _agent_pane(panes: list[dict], children: dict[int, list[int]]) -> dict:
        """Escolhe, entre os panes de UMA sessao, o que roda o agente (Task 5.5).

        list_panes_active() so trazia o pane ATIVO — e "ativo" e por JANELA, nao por sessao: uma
        segunda janela/split (o botao `+` da Task 6) fica marcada ativa TAMBEM, e o antigo dedup por
        nome ficava com a PRIMEIRA da varredura, arbitrario. Com o agente numa janela e o shell na
        outra em primeiro plano, provider/jsonl/pane_id saiam todos do pane ERRADO (medido: o shell
        vira "sem id" na lista).
        Reusa o predicado ESTRITO do agentpane (_pane_do_agente, Task 1) e o MESMO mapa /proc que
        list() ja construiu pra sessao inteira -> zero fork NOVO (achado menor da revisao: a leitura
        de /proc nao e zero, e sim proporcional ao numero de panes candidatos da sessao — barata
        porque o mapa `children` ja esta pronto, mas nao e de graca). Nenhum pane bate -> cai no
        pane ATIVO, o comportamento de sempre (None = nao sei, nao decide um comportamento novo
        sozinho).
        """
        if len(panes) > 1:
            # Achado menor da revisao: com 2+ panes do agente na MESMA sessao (caso raro), o ATIVO
            # ganha o desempate -- preserva o comportamento de antes desta task pra esse caso, em vez
            # de arbitrario "o primeiro da varredura".
            for p in sorted(panes, key=lambda p: not p["active"]):
                if p["pid"] is not None and agentpane._pane_do_agente(p["pid"], children):
                    return p
            name = panes[0]["name"]
            if name not in SessionRegistry._SEM_AGENTE_AVISADAS:
                # Falha aparece, nao some — mas UMA vez por nome (list() e polled a cada segundo;
                # logar em TODO poll enquanto a sessao seguir sem agente reconhecido enche o journal
                # a toa). ponytail: dedup por NOME nunca expira (nem no kill/recria, ao contrario do
                # agentpane._AVISADAS) — pior caso e uma sessao rara, apos recriada, ficar calada de
                # novo neste caso; upgrade so se virar reclamacao real.
                SessionRegistry._SEM_AGENTE_AVISADAS.add(name)
                _log.warning("list: %r tem %d panes e nenhum parece do agente; "
                             "caindo no pane ATIVO", name, len(panes))
        return next((p for p in panes if p["active"]), panes[0])

    def list(self) -> list[SessionInfo]:
        # Resolucao de jsonl/tracked de todas as sessoes. Otimizado: UM mapa /proc + UMA chamada tmux
        # (pane_pid em lote) reusados por sessao -> O(P + S·descendentes) em vez de O(S·P). NAO calcula
        # state (sai 'idle' default): este caminho so resolve transcript; quem quer state usa
        # list_with_state(). Usado por varios endpoints que so precisam do jsonl por nome.
        children = _proc_children_map()
        out = []
        sids: dict[str, Optional[str]] = {}
        for panes in tmux.list_panes_all().values():
            # Sessao de shell ESCONDIDA (Task 6, botao "+" do painel de terminal): marcada por
            # opcao de usuario tmux (@cp_hidden), herdada por TODOS os panes/janelas da sessao (
            # confirmado na revisao), lida de carona no MESMO list-panes acima -- sem isto ela
            # viraria CARD nas tres views (lista, board, canvas), porque pane nao reconhecido vira
            # Claude por padrao logo abaixo. list_with_state() reusa esta mesma lista (nao chama
            # list_panes_all de novo), entao o pulo vale nas duas.
            #
            # Checado ANTES do `_agent_pane` (achado da revisao, minor): usuario dividindo o
            # proprio shell escondido (2+ panes, nenhum "agente") faria `_agent_pane` nao achar
            # ninguem, logar o warning "nenhum parece do agente" pra sempre (suja
            # `_SEM_AGENTE_AVISADAS`, que nunca expira) e pagar a descida de /proc por pane -- tudo
            # sobre uma sessao que o app ignora DE PROPOSITO. Qualquer pane serve pra checar: a
            # marca e por sessao, todos concordam.
            if panes[0].get("hidden"):
                _log.debug("list: sessao %r pulada (marcada @cp_hidden)", panes[0]["name"])
                continue
            p = self._agent_pane(panes, children)
            # A TUI Codex agora vive no tmux, mas sua identidade/historico continuam vindo do
            # sidecar + rollout. Nao a tratar tambem como Claude (duplicaria a sessao e tentaria
            # resolver ~/.claude/projects).
            if codex_sessions.exists(p["name"]):
                # O filtro e por NOME. Se um sidecar ficar ORFAO (crash antes do cleanup) e alguem
                # criar uma sessao Claude com o mesmo nome, ela sumiria da lista SEM explicacao --
                # o usuario perderia acesso a uma sessao viva. Nao da pra distinguir aqui sem custo,
                # entao pelo menos NAO e silencioso: o log diz qual nome foi filtrado e por que.
                _log.debug("list: pane %r filtrado por sidecar Codex de mesmo nome", p["name"])
                continue
            # Pi anda no MESMO caminho tmux que o Claude (pane real, mtime real) — so a resolucao do
            # jsonl muda: o --session-id nao sobrevive no cmdline (Task 0, fato 7) e resolve_tracked
            # cairia no fallback newest-by-mtime, que pegaria o transcript do CLAUDE do mesmo cwd (a
            # regressao mais cara desta task). Resolve pelo bilhete da extensao / env do wrapper.
            prov = provider_of_pane(p["pid"], children)
            if prov == "pi":
                jsonl = pi_session_file(p.get("pane_id", ""), p["pid"], p["cwd"])
                # tracked segue o TRANSCRITO, nao o provider. O bilhete/env sao deterministicos
                # (nunca um chute como o newest-by-mtime do Claude), mas quando NENHUM dos dois
                # resolve um arquivo nao ha vinculo nenhum: /events e /history exigem info.jsonl e
                # devolvem 404. Com o True fixo a lista mostrava um card clicavel que abria um chat
                # quebrado e, por ser "tracked", sem nenhuma das afordancias de sessao sem id. Com
                # False as duas views cinzam a linha e explicam. E temporario por construcao: no 1o
                # turno o Pi escreve o transcript, o bilhete passa a resolver e a proxima varredura
                # devolve tracked=True sozinha (list() e polled).
                tracked = jsonl is not None
            else:
                jsonl, tracked = self.resolve_tracked(p["name"], p["cwd"], p["pid"], children)
            link = ThenLink(p["name"]).get()
            pair = PairLink(p["name"]).get()
            info = SessionInfo(name=p["name"], cwd=p["cwd"], jsonl=jsonl, tracked=tracked,
                               branch=self._branch_of(p["cwd"]),
                               then_target=link.get("target") if link else None,
                               pair_peers=pair.get("peers") if pair else None,
                               pair_gid=pair.get("gid") if pair else None,
                               pair_task=pair.get("task") if pair else None)
            if prov == "pi":
                info.provider = "pi"
            # Motor da sessão, do mesmo pid que já resolve o config_dir. É uma leitura de
            # /proc/<pid>/environ por sessão (a mesma ordem de custo do _config_dir_of ao lado) —
            # não é de graça, mas é local e sem rede. Feature em tick do SSE tem que ser barata.
            info.engine = _engine_of(p["pid"]) if p.get("pid") else None
            out.append(info)
            sids[p["name"]] = self._repl_sid(p["pid"], children)
        # Guarda de colisao: 2+ sessoes no mesmo jsonl -> so a dona mantem (mata a duplicata/cross-wire).
        self._dedupe_collisions(out, sids)
        # Sessoes Codex: a TUI vive no tmux, mas a identidade vem dos sidecars duraveis (sobrevivem
        # a restart; o historico esta no rollout). O client vivo e reaberto sob demanda.
        for meta in codex_sessions.list_all():
            out.append(SessionInfo(
                name=meta["name"], cwd=meta.get("cwd"), jsonl=meta.get("rollout_path"),
                provider="codex", tracked=True,
                branch=self._branch_of(meta.get("cwd")),
                then_target=(ThenLink(meta["name"]).get() or {}).get("target"),
                pair_peers=(PairLink(meta["name"]).get() or {}).get("peers"),
                pair_gid=(PairLink(meta["name"]).get() or {}).get("gid"),
                pair_task=(PairLink(meta["name"]).get() or {}).get("task"),
            ))
        return out

    async def list_with_state(self, infos: Optional[list[SessionInfo]] = None) -> list[SessionInfo]:
        # Listagem COM estado vivo por sessao (pro /api/sessions). Faz a resolucao otimizada (sync, num
        # thread) e por cima classifica o pane de cada sessao concorrentemente. `infos` opcional: um
        # snapshot ja resolvido (ex: cache compartilhado dos pollers do SSE) pula a re-resolucao.
        if infos is None:
            infos = await asyncio.to_thread(self.list)
        if not infos:
            return infos
        # Estado pela marca dos hooks quando existe (custo ~0); senao cai no pane (fallback).
        # NOTA: o sweep de STATUSLINE (mais abaixo) captura pane mesmo de sessao com marcador —
        # a statusline nao tem outra fonte. O "custo ~0" continua valendo pra CLASSIFICACAO; o
        # sweep e limitado a _STATUS_BUDGET capturas por chamada com TTL de _STATUS_TTL.
        def _sid(jsonl):
            return Path(jsonl).stem if jsonl else None
        pending = []  # infos sem marcador (ou awaiting) -> precisa raspar o pane
        for info in infos:
            # Sessoes Codex nao vivem no tmux -> nunca raspar o pane (capture_pane erraria numa
            # sessao inexistente). O estado vivo (working/idle) chega em runtime pelo adapter via SSE;
            # aqui fica o default idle + last_activity do rollout.
            if getattr(info, "provider", "claude") == "codex":
                info.last_activity = _jsonl_mtime(info.jsonl)
                continue
            marker = hook_state.get_state(_sid(info.jsonl))
            # Marker autoritativo pra working/idle/dead (custo ~0). Pra awaiting_input o marcador NAO
            # carrega a pergunta -> raspa o pane (junto das sem-marcador) pra pegar question/options.
            # LIMITACAO CONHECIDA (rate-limit radar, feature #8): este fast-path PULA a captura do pane,
            # entao rate_limit_reset() NUNCA roda por aqui -> limited/limit_reset ficam no default
            # (False/None). Uma sessao rate-limited fica working/idle (o banner nao e menu), logo cai
            # SEMPRE neste caminho de marcador -> na pratica o chip "limitado"/notify_limited/auto-resume
            # so disparam pela sessao com o chat aberto (StateMonitor raspa o pane), nunca pelo radar da
            # lista. NAO corrigido de proposito: fazer o watchdog raspar o pane de toda sessao working/idle
            # a cada poll so faz sentido depois que _LIMIT_RE (app/state.py) for calibrado contra o banner
            # REAL — hoje e um chute nao-calibrado, entao a deteccao nao funcionaria de verdade mesmo com
            # a plumbing pronta. Calibrar _LIMIT_RE primeiro; so entao vale mover a deteccao pro watchdog.
            # awaiting_input SEMPRE raspa o pane (o marcador nao carrega question/options). O que
            # segurava a tempestade de capture_pane era marcador awaiting PRESO — a Notification de
            # "idle 60s" do Claude Code chega DEPOIS do Stop e nada corrigia, entao a sessao parada
            # raspava a cada poll (e, com o fast-path stale antigo, mostrava "aguardando" falso pra
            # sempre). Corrigido na RAIZ: pane raspado sem menu REBAIXA o marcador pra idle
            # (demote_awaiting, abaixo) -> proximo poll cai no fast-path de marcador como idle.
            if marker and marker[0] != "awaiting_input":
                info.state = marker[0]
                info.last_activity = _jsonl_mtime(info.jsonl)
                if marker[0] != "working":
                    # Turno acabou (hook e autoritativo): o spinner cacheado e do PASSADO — sem
                    # isto o proximo working herdava a barrinha do turno anterior como se fosse
                    # ao vivo (label fantasma).
                    self._label_cache.pop(info.name, None)
            else:
                pending.append(info)
        if pending:
            frames = await asyncio.gather(*[asyncio.to_thread(tmux.capture_pane, info.name) for info in pending])
            classified = [classify(t) for t in frames]
            spinners = [_live_spinner(t) for t in frames]
            spin_idx = [k for k, c in enumerate(classified) if c[0] == "working"]
            if spin_idx:
                await asyncio.sleep(0.15)
                f2 = await asyncio.gather(*[asyncio.to_thread(tmux.capture_pane, pending[k].name) for k in spin_idx])
                for j, k in enumerate(spin_idx):
                    sp2 = _live_spinner(f2[j])
                    if sp2 is None or sp2 == spinners[k]:
                        classified[k] = ("idle", None, None, None)
            for info, c, frame in zip(pending, classified, frames):
                info.state = c[0]
                info.label = c[1]
                info.question = c[2]
                info.options = c[3]
                info.last_activity = _jsonl_mtime(info.jsonl)
                # Pane (verdade) contradisse marcador awaiting (Notification de idle-60s, nao menu):
                # rebaixa pra idle no hook_state (mapa+sidecar) — mata o "aguardando" fantasma e
                # devolve a sessao ao fast-path (anti-tempestade). Grace: ver _AWAITING_DEMOTE_GRACE_S.
                sid = _sid(info.jsonl)
                m = hook_state.get_state(sid)
                if (m and m[0] == "awaiting_input" and c[0] != "awaiting_input"
                        and time.time() - m[1] > _AWAITING_DEMOTE_GRACE_S):
                    hook_state.demote_awaiting(sid)
                # Rate-limit radar (feature #8): so pane-derivado, entao so nas infos raspadas aqui
                # (marker path fica com o default False/None, igual a label/question/options).
                info.limit_reset = rate_limit_reset(frame)
                info.limited = info.limit_reset is not None
                # Statusline + label de graca: o frame ja foi capturado pra classificar.
                self._status_cache[info.name] = (time.monotonic(), _pane_status(frame))
                self._label_cache[info.name] = c[1]
        # Statusline pros cards (modelo/contexto/⚡5h/📅7d): cache com TTL — capturar o pane de TODAS
        # por tick seria a tempestade de forks que o fast-path de marcador evita. No maximo
        # _STATUS_BUDGET capturas por chamada, das entradas mais VELHAS do cache; quem foi raspada
        # acima ja atualizou de graca. ponytail: cadencia ~(N/_STATUS_BUDGET)×poll — com 15 sessoes
        # e poll 1.5s, ciclo completo ~11s; statusline muda devagar, atraso e invisivel.
        now_m = time.monotonic()
        stale = [
            i for i in infos
            if getattr(i, "provider", "claude") != "codex"
            and all(i is not p for p in pending)
            and now_m - self._status_cache.get(i.name, (0.0, None))[0] > _STATUS_TTL
        ]
        stale.sort(key=lambda i: self._status_cache.get(i.name, (0.0, None))[0])
        for info in stale[:_STATUS_BUDGET]:
            try:
                pane = await asyncio.to_thread(tmux.capture_pane, info.name)
                self._status_cache[info.name] = (time.monotonic(), _pane_status(pane))
                # Spinner da MESMA captura (classify e puro/regex): e o que devolve a barrinha de
                # "trabalhando" pro card quando o estado veio do marcador (que nao traz label).
                # SO grava se a captura PARECE working — captura unica nao distingue spinner vivo
                # de marcador congelado no scrollback (o caminho pending faz captura dupla pra
                # isso; aqui dobrar o fork nao vale — pane nao-working derruba o label e o proximo
                # sweep re-avalia).
                st_c, lbl_c = classify(pane)[:2]
                if st_c == "working":
                    self._label_cache[info.name] = lbl_c
                else:
                    self._label_cache.pop(info.name, None)
            except Exception as e:
                # tmux engasgado (transiente): carimba o relogio (senao o nome quebrado monopolizaria
                # o budget a cada chamada) mas PRESERVA a ultima statusline boa — apagar aqui piscava
                # o badge do card e forcava re-emissao da lista a toa. Sessao morta de verdade sai da
                # lista sozinha (e kill via app limpa no _forget). Logado em debug: tmux quebrado
                # cronico deixaria toda statusline congelada sem rastro nenhum.
                _log.debug("statusline capture falhou pra %s: %r", info.name, e)
                prev = self._status_cache.get(info.name, (0.0, None))[1]
                self._status_cache[info.name] = (time.monotonic(), prev)
                # Label NAO segue o preserve do status_line: statusline velha ainda e verdadeira
                # (modelo/custo mudam devagar); spinner velho vira fantasma — melhor sem barrinha.
                self._label_cache.pop(info.name, None)
        for info in infos:
            if getattr(info, "provider", "claude") != "codex":
                # Sidecar antes do pane: a captura traz a linha ja CORTADA na largura da janela
                # (quem renderiza trunca antes de imprimir, ver app/statusline.py). Ler o arquivo e
                # muito mais barato que a captura — nao entra no budget de forks acima.
                info.status_line = (_sidecar_status(_sid(info.jsonl))
                                    or self._status_cache.get(info.name, (0.0, None))[1])
                # So preenche o buraco do fast-path: quem foi classificada pelo pane ja tem label
                # fresco (e idle de verdade fica sem label — o cache so vale se ainda working).
                # getattr: fakes de teste nao tem o campo.
                if info.state == "working" and getattr(info, "label", None) is None:
                    info.label = self._label_cache.get(info.name)
        # Travada (feature #7): "working" ha mais de CP_STALL_SECONDS sem o transcript avancar. So o
        # bool derivado pra UI/sig — o push (1x, com dedupe) e responsabilidade do stall_watch, nao daqui.
        now = time.time()
        for info in infos:
            info.stalled = (
                info.state == "working"
                and info.last_activity is not None
                and (now - info.last_activity) > runtime_config.get("stall_seconds")
            )
        # Estado de git por sessão — SÓ aqui (payload do /api/sessions), nunca em list(): git_summary
        # forka `git status` e list() é o caminho leve chamado por kill()/resume/SSE. E como
        # list_with_state é awaitado direto no event loop (/api/sessions, sse, stall_watch), o loop
        # de forks vai pro threadpool via asyncio.to_thread — rodar na corrotina congelaria o backend
        # inteiro no cache-miss. Gate em .git e except GitError moram no git_summary; cache de 3s
        # segura o custo vs o poll de 2s.
        def _decorate_git() -> None:
            for info in infos:
                summary = git_summary(info.cwd)
                if summary is not None:
                    info.git_dirty = summary["dirty"]
                    info.git_ahead = summary["ahead"]
                    info.git_behind = summary["behind"]
                # Plano vive AQUI dentro, no mesmo to_thread: le markdown do disco, e ler arquivo na
                # corrotina e a mesma classe de erro que motivou o to_thread do git.
                _decorate_plan(info)

        await asyncio.to_thread(_decorate_git)
        for info in infos:
            _decorate_loop(info)
        return infos

    def create(self, name: str, cwd: str, config_dir: str | None = None,
               resume_session_id: str | None = None, provider: str = "claude",
               engine: str | None = None) -> SessionInfo:
        # Nome tmux nao aceita "."/":"/espaco -> sanitiza igual ao rename. Varias sessoes na MESMA
        # pasta sao permitidas: cada uma tem nome unico + --session-id proprio -> jsonl proprio.
        name = sanitize_session_name(name)
        if not name:
            raise ValueError("nome invalido")
        # Motor de modelo: valida ANTES de criar o pane. Motor inexistente com env vazio faria a
        # sessão subir na conta Anthropic ACHANDO que é o motor pedido — falha silenciosa.
        if engine:
            from app import engines
            if engine not in engines.listar():
                raise ValueError(f"motor '{engine}' nao existe")
        # Codex nao e tmux: o caminho async (spawn do app-server, thread/start) roda no loop
        # principal via create_codex(); o create() sync spawnaria o AppServerClient num loop
        # descartavel (asyncio.run) que morre ao retornar -> orfanaria o subprocess/reader task.
        # Por isso o create() sync e Claude-only e recusa Codex alto (Task 6 fia o endpoint async).
        if provider == "codex":
            raise ValueError("sessoes Codex sao criadas via create_codex (async)")
        # Pi anda no MESMO caminho tmux do Claude, mas duas coisas daqui pra baixo sao Claude puro e
        # recusam alto em vez de "quase funcionar":
        #  - motor: o `cp-engine --exec` so exporta ANTHROPIC_* / CLAUDE_CODE_*, que o pi ignora ->
        #    a sessao subiria na conta do proprio pi PARECENDO estar no motor pedido.
        #  - resume: o branch abaixo monta `claude --resume <uuid>` LITERAL -> aceitar aqui spawnaria
        #    um Claude com cara de sessao Pi, lendo o transcript do agente errado. O equivalente no Pi
        #    seria `pi --session <id>` (o wrapper ja respeita esse flag); enquanto nao existir, 400.
        if provider == "pi":
            if engine:
                raise ValueError("motor so vale para provider claude")
            if resume_session_id is not None:
                raise ValueError("resume de sessao pi ainda nao e suportado")
        # Unicidade contra tmux (Claude) E sidecars Codex: sem o segundo check, um nome de sessao
        # Codex reusado aqui geraria DOIS SessionInfo com o mesmo name no list() (front keyed por
        # nome) e o kill(name) cairia no branch Codex (checado 1o) -> fecharia o client Codex sem
        # matar o pane tmux (pane orfao inkillavel).
        if tmux.has_session(name) or codex_sessions.exists(name):
            raise ValueError("ja existe uma sessao com esse nome")
        # resume_session_id (retomar conversa MORTA do Arquivo): reusa o uuid existente e sobe com
        # `--resume` em vez de `--session-id` -> o claude CONTINUA aquele jsonl (nao comeca um novo).
        # Mesmo uuid ja validado no endpoint, mas revalida aqui tambem (vai direto pro comando do shell).
        # ponytail: resume so cobre o path do Claude por ora (--resume nao existe no Codex — a Task 5
        # do plano de Codex resolve o resume dele por fora deste branch).
        if resume_session_id is not None:
            try:
                uuid.UUID(resume_session_id)
            except (ValueError, AttributeError, TypeError):
                raise ValueError("session_id invalido")
            sid = resume_session_id
            cmd = f"claude --resume {sid}"
        else:
            sid = str(uuid.uuid4())
            # spawn_command vem do Adapter do provider (import local: get_adapter->ClaudeAdapter nao
            # importa registry, mas evita qualquer ciclo se um adapter futuro vier a importar daqui).
            from app.adapters import get_adapter
            cmd = " ".join(get_adapter(provider).spawn_command(cwd, sid))
        if engine:
            # `cp-engine --exec` aplica o env DENTRO do pane (os.execvpe). Não usamos `tmux -e` porque
            # a key ficaria em /proc/<pid>/cmdline, legível por qualquer usuário da máquina. Depois do
            # exec o cmdline é o do claude, então a resolução de transcript por --session-id/--resume
            # continua funcionando.
            cmd = f"cp-engine --exec {engine} -- {cmd}"
        base = (Path(config_dir) / "projects") if config_dir else self.projects_dir
        # Pi tem layout PROPRIO (~/.pi/agent/sessions/<slug>/<ts>_<uuid>.jsonl) e o arquivo so nasce
        # quando a TUI grava o 1o turno -> nao ha path pra pre-semear. jsonl=None e cache INTOCADO
        # (ver o final do metodo): o _jsonl_cache e de CLASSE, compartilhado com o sse, e um path do
        # layout do Claude ali seria um arquivo que nunca existe, devolvido por resolve() pra sempre.
        # Quem liga o pane ao transcript e o bilhete que a extensao escreve (ver pi_session_file).
        jsonl = None if provider == "pi" else str(base / sanitize_cwd(cwd) / f"{sid}.jsonl")
        # Pré-confia a pasta no .claude.json: sem isto, uma sessão criada pelo app numa pasta NOVA
        # nasce presa no "trust this folder?" do Claude Code (invisível/ininteragível pelo chat até
        # aceitar na TUI). Só é o 1º acesso à pasta — depois o próprio Claude Code grava. Best-effort.
        # Pi não lê o .claude.json e tem o próprio fluxo de confiança -> escrever ali só sujaria a
        # lista de pastas confiadas do Claude com pasta que ele talvez nunca abra.
        if provider != "pi":
            _pretrust_cwd(cwd, config_dir)
        if not tmux.new_session(name, cwd, cmd, config_dir):
            raise ValueError("falha ao criar sessao no tmux")
        # Sessao NOVA = sid novo = transcript fresco. A fila duravel e keyed pelo NOME (sobrevive ao
        # fim da sessao antiga), entao entradas remanescentes de uma sessao morta de mesmo nome
        # fantasmariam aqui via merged_history. Limpa igual o /clear faz. Seguro: a sessao nova ainda
        # nem aceitou input, nao ha fila legitima a preservar.
        PromptQueue(name).clear()
        # Mesmo motivo, pro vinculo 'then' (feature #12): nome reusado nao deve herdar um encadeamento
        # de uma sessao antiga e ja morta.
        ThenLink(name).clear()
        # Fixa o jsonl FRESCO no cache na hora: resolve() devolve este uuid mesmo antes do claude
        # escrever o arquivo, evitando o fallback newest-by-mtime pescar um jsonl ja existente da pasta.
        # Pi (jsonl=None) nao entra no cache — nao ha path a fixar, e a resolucao dele nem passa por aqui.
        if jsonl is not None:
            self._jsonl_cache[name] = jsonl
        return SessionInfo(name=name, cwd=cwd, jsonl=jsonl, provider=provider, engine=engine)

    async def create_codex(self, name: str, cwd: str,
                           initial_prompt: str | None = None) -> SessionInfo:
        # Caminho Codex: spawna um app-server WebSocket local, abre um thread e cria uma TUI
        # `codex --remote` no tmux ligada ao mesmo servidor. O backend conserva o controle JSON-RPC.
        name = sanitize_session_name(name)
        if not name:
            raise ValueError("nome invalido")
        # Unicidade contra sessoes tmux (Claude) E sidecars Codex existentes.
        if tmux.has_session(name) or codex_sessions.exists(name):
            raise ValueError("ja existe uma sessao com esse nome")
        client = AppServerClient()
        try:
            endpoint = await client.start_shared()
            await client.request("initialize", {
                "clientInfo": codex_adapter._CLIENT_INFO, "capabilities": None})
            codex_adapter.ensure_tmux_tui(
                name, cwd, None, endpoint, initial_prompt=initial_prompt,
            )

            # A TUI cria a thread e publica sua identidade a todos os clientes do app-server.
            # Isso tambem garante que o rollout ja exista, permitindo `codex resume` no restart.
            async def _tui_thread() -> dict:
                async for notification in client.notifications():
                    if notification.get("method") != "thread/started":
                        continue
                    thread = (notification.get("params") or {}).get("thread") or {}
                    if thread.get("cwd") == cwd:
                        return thread
                raise ConnectionError("app-server encerrou antes de a TUI criar a thread")

            thread = await asyncio.wait_for(_tui_thread(), timeout=20)
        except Exception:
            # Falha no handshake: nao deixa o subprocess orfao.
            await client.close()
            if tmux.has_session(name):
                tmux.kill_session(name)
            raise
        thread_id = thread.get("id")
        rollout_path = thread.get("path")
        if not thread_id or not rollout_path:
            await client.close()
            tmux.kill_session(name)
            raise ValueError("thread/start nao devolveu id/path")
        # save() (mkdir+write_text -> pode dar OSError: disco cheio/permissao) e attach() rodam com o
        # app-server JA spawnado -> qualquer falha aqui tem que fechar o client, senao vira orfao. Se
        # save deu certo mas attach falhou, remove o sidecar recem-escrito (estado consistente: nao
        # fica sidecar apontando pra um client fechado).
        try:
            # Sidecar duravel: sobrevive ao restart do backend (identidade + ponteiro pro rollout).
            codex_sessions.save(name, thread_id, rollout_path, cwd)
            # Client vivo (efemero) anexado no adapter; limpa fila/then herdados de nome reusado.
            # A sessao nova ainda nao tem escolha explicita de modelo; o catalogo/picker e os
            # eventos dos turnos populam o display depois.
            from app.adapters import get_adapter
            adapter = get_adapter("codex")
            adapter.attach(name, client, thread_id, watch_tmux=True)
            # ASSINA a thread que a TUI criou. Sem isto o backend so recebe eventos globais do
            # app-server -- nada de turn/*, item/* ou tokenUsage -- e a sessao fica "viva mas
            # surda": estado congelado, sem preview/statusline e com a fila do celular presa (o
            # drain-on-complete mora no turn/completed). Em background porque thread/resume so
            # e aceito depois que o 1o turno grava o rollout; ver _subscribe_when_ready.
            adapter.start_subscription(name, cwd)
        except Exception:
            await client.close()
            codex_sessions.delete(name)  # idempotente; remove sidecar orfao se save ja tinha passado
            if tmux.has_session(name):
                tmux.kill_session(name)
            raise
        PromptQueue(name).clear()
        ThenLink(name).clear()
        return SessionInfo(name=name, cwd=cwd, jsonl=rollout_path, provider="codex")

    def rename(self, old: str, new: str) -> None:
        if codex_sessions.exists(old):
            from app.adapters import get_adapter
            codex_sessions.rename(old, new)
            get_adapter("codex").rename(old, new)
        # Cache e keyed por NOME -> ao renomear, move a entrada pro nome novo e esquece o velho. Senao
        # o nome velho apontaria pro jsonl pra sempre (reuso futuro = transcript errado) e o nome novo
        # cairia no fallback newest-by-mtime ate um sinal confiavel reaparecer.
        j = self._jsonl_cache.pop(old, None)
        if j is not None:
            self._jsonl_cache[new] = j
        if old in self._fd_locked:           # move o fd-lock junto com o cache
            self._fd_locked.discard(old)
            self._fd_locked.add(new)
        st = self._status_cache.pop(old, None)   # statusline move junto (mesma sessao, so outro nome)
        if st is not None:
            self._status_cache[new] = st
        if old in self._label_cache:
            self._label_cache[new] = self._label_cache.pop(old)
        # A fila duravel tambem e keyed por NOME -> move junto, senao a sessao renomeada perde as
        # entradas nao-drenadas e elas ficam orfas no nome velho (fantasma se reusarem `old`).
        PromptQueue(old).rename(new)
        # Vinculo 'then' (feature #12): mesmo motivo — keyed por NOME, move junto pra sessao renomeada
        # nao perder o encadeamento armado.
        ThenLink(old).rename(new)
        # Pareamento: move o próprio sidecar E re-aponta o do PAR (que referencia o nome velho) —
        # senão o par ficaria pareado com um fantasma e o unpair simétrico quebrava. Sob o lock do
        # módulo pair (rename_pair): sem ele, um unpair concorrente podia ser ressuscitado.
        rename_pair(old, new)
        # L71 da revisao final: o shell escondido e keyed por NOME (`term-<nome>`) e NAO acompanha o
        # rename sozinho -- ele ficava orfa pra sempre, invisivel no app (marcado @cp_hidden) e fora
        # do alcance do `kill()`, que so procura `term-<nome NOVO>`. Pior: a aba Shell do nome novo
        # criaria um shell NOVO e o velho seguiria vivo consumindo o nome, ate colidir com uma
        # sessao futura.
        # RENOMEIA, nao mata: `rename-session` nao mexe no cwd nem no que esta rodando no pane -- o
        # shell continua no mesmo diretorio, que e o diretorio da sessao renomeada. (O perigo de
        # "shell no diretorio errado" e outro caminho: reatar um `term-<nome>` orfa de OUTRO repo,
        # tratado no tmux.new_hidden_shell.) Matar em silencio derrubaria um `npm run dev` que
        # estivesse rodando ali, e o unico registro disso e um `_log.debug`.
        # Kill so como FALLBACK: renomear falha se `term-<novo>` ja existir (shell de uma vida
        # anterior daquele nome), e ai deixar o velho vivo traz de volta o orfa que este bloco
        # existe pra evitar.
        # A marca e o gate, como no `_kill_hidden_shell`: sem ela, um `term-<velho>` de TERCEIRO
        # seria sequestrado pelo rename. `is_hidden` mira `={nome}:` (exato), entao o rename so
        # roda quando a sessao existe de verdade -- sem risco do prefix-match do tmux pegar
        # `term-<velho>-2`.
        alvo = f"term-{old}"
        if tmux.is_hidden(alvo) and not tmux.rename_session(alvo, f"term-{new}"):
            _log.info("rename: %r nao pode virar %r (nome ja ocupado?) — matando o shell escondido",
                      alvo, f"term-{new}")
            self._kill_hidden_shell(old)

    @staticmethod
    def _kill_hidden_shell(name: str) -> None:
        # Task 6 (achado da revisao, rodada 2): mata `term-<name>` SO se a marca confirmar que a
        # sessao e NOSSA -- consulta DIRETA ao tmux (`is_hidden`), nao inferida de `self.list()`
        # (que tambem filtra por sidecar Codex, e "sumir da lista" nao e o mesmo que "estar
        # marcada"). Sem esta checagem, um "term-<name>" de TERCEIRO (o mesmo cenario alcancavel
        # que o I1 reconheceu na rota /shell) seria derrubado JUNTO quando o agente `name` fosse
        # encerrado, com trabalho rodando e sem afordancia nenhuma pro dono perceber -- so um
        # `_log.debug`. Best-effort: falhar aqui NAO pode derrubar o kill principal, que ja
        # aconteceu antes desta chamada.
        alvo = f"term-{name}"
        if tmux.is_hidden(alvo) and not tmux.kill_session(alvo):
            _log.debug("kill: shell escondido de %r nao saiu (pode nao existir)", name)

    def kill(self, name: str) -> None:
        # Levanta KillFailed quando a sessao SOBREVIVE. Antes o retorno do tmux era descartado e a
        # limpeza duravel (cache, fila, then, pareamento) rodava do mesmo jeito: o card sumia da UI, o
        # pareamento se desfazia, a rota respondia {"ok": true} — e a sessao reaparecia na varredura
        # seguinte, sem fila e sem par, parecendo um bug sem relacao com o "encerrar" de minutos antes.
        # Pesa mais no Windows, onde o kill-session do psmux nao derruba a sessao (medido).
        if codex_sessions.exists(name):
            # Sessao Codex: fecha app-server e TUI tmux, apaga o sidecar e limpa estado duravel.
            from app.adapters import get_adapter
            get_adapter("codex").close_sync(name)
            if not tmux.kill_session(name):
                raise KillFailed(name)
            self._kill_hidden_shell(name)
            codex_sessions.delete(name)
            self._forget(name)
            PromptQueue(name).clear()
            ThenLink(name).clear()
            self._clear_pair(name)
            return
        # Limpa o sidecar do AskUserQuestion ANTES de matar (precisa do processo vivo pra resolver o
        # jsonl), best-effort: cleanup nunca bloqueia/quebra o kill. Senao um stale reabriria o stepper
        # numa sessao futura de mesmo nome.
        try:
            jsonl = next((s.jsonl for s in self.list() if s.name == name), None)
            if jsonl:
                clear_pending_askq(jsonl)
        except Exception:
            pass
        if not tmux.kill_session(name):
            raise KillFailed(name)
        self._kill_hidden_shell(name)
        self._forget(name)  # cache invalido: nome pode ser reusado por outra sessao depois
        # Sessao morta nao deixa fila pra tras: senao acumula orfaos e uma futura sessao de mesmo
        # nome herdaria essas entradas como bubble-fantasma (mesmo motivo do clear no create()).
        PromptQueue(name).clear()
        ThenLink(name).clear()  # mesmo motivo, pro vinculo 'then' (feature #12)
        self._clear_pair(name)

    @staticmethod
    def _clear_pair(name: str) -> None:
        # Sessão morta SAI do grupo (leave: sob lock, atualiza os demais membros): sem isto os
        # companheiros apontariam pra um fantasma (badge preso). Best-effort, nunca bloqueia o
        # kill — mas LOGA: engolir calado deixava o badge-fantasma indiagnosticável.
        try:
            pair_leave(name)
        except Exception as e:
            _log.warning("kill(%s): falha ao sair do grupo de pareamento: %r", name, e)

    # ── Resume de sessao "sem id" ────────────────────────────────────────────────
    # Uma sessao aberta com `claude` cru (sem --session-id) JA tem um transcript <uuid>.jsonl; so nao da
    # pra ligar o pane a ele com seguranca (o uuid nao esta no cmdline). Relançar o pane com
    # `claude --resume <uuid>` poe o uuid no cmdline -> resolve() volta a rastrear (tracked=True) e o chat
    # abre CONTINUANDO a mesma conversa. Reusa kill+new_session (trata cores/config-dir corretamente).

    def _pane_of(self, name: str) -> Optional[dict]:
        return next((p for p in tmux.list_panes_active() if p["name"] == name), None)

    def _first_user_text(self, jsonl: str, max_lines: int = 60) -> str:
        # Preview do candidato = 1a msg de usuario da conversa (identifica "qual conversa e essa"). Le so
        # as primeiras linhas; import local pra evitar ciclo (transcript -> models -> ...).
        from app.transcript import parse_line
        try:
            with open(jsonl, encoding="utf-8", errors="replace") as fh:
                for _, line in zip(range(max_lines), fh):
                    for ev in parse_line(line):
                        if ev.kind == "user_msg" and ev.text:
                            return ev.text[:100]
        except OSError:
            pass
        return ""

    @staticmethod
    def _refuse_non_claude_resume(pane: dict) -> None:
        # O resume e Claude-only de ponta a ponta: os candidatos saem de ~/.claude/projects e o
        # relance e `claude --resume <uuid>` DEPOIS de matar o pane. Numa sessao Pi sem transcript
        # (que agora aparece como "sem id" e por isso ganha o botao de retomar), isso ofereceria
        # conversas do CLAUDE daquele cwd e, se o usuario escolhesse uma, mataria a sessao Pi viva
        # pra subir um claude no lugar dela. Recusa com uma frase que diz o que FAZER.
        # ponytail: recusar e o piso. Retomar de verdade exige varrer ~/.pi/agent/sessions e
        # relancar com `pi --session <id>` exportando CP_PI_SESSION — o upgrade, quando alguem
        # topar com isto de verdade.
        prov = provider_of_pane(pane.get("pid"))
        if prov != "claude":
            raise ValueError(
                f"retomar so vale pra sessao Claude (esta e {prov}); "
                "feche o pane e abra de novo pelo wrapper `pi`")

    def resume_candidates(self, name: str) -> tuple[str, bool, list[dict]]:
        # (cwd, ambiguo, candidatos). ambiguo = ha OUTRA sessao tmux no mesmo cwd -> o "mais recente por
        # mtime" pode ser de outra sessao (a UI pede confirmacao). candidatos = ate 6 jsonls recentes do
        # cwd, cada um com preview + se ja esta em uso por outra sessao viva.
        pane = self._pane_of(name)
        if pane is None:
            raise ValueError("sessao nao encontrada")
        self._refuse_non_claude_resume(pane)
        cwd = pane["cwd"]
        cdir = _config_dir_of(pane["pid"]) if pane.get("pid") else None
        proj = ((cdir / "projects") if cdir else self.projects_dir) / sanitize_cwd(cwd)
        files = sorted(proj.glob("*.jsonl"),
                       key=lambda f: (f.stat().st_mtime if f.exists() else 0.0), reverse=True)[:6] \
            if proj.is_dir() else []
        taken = {os.path.realpath(s.jsonl) for s in self.list() if s.jsonl and s.name != name}
        cands = [{
            "session_id": f.stem,
            "mtime": _jsonl_mtime(str(f)),
            "preview": self._first_user_text(str(f)),
            "in_use": os.path.realpath(str(f)) in taken,
        } for f in files]
        return cwd, self._cwd_has_siblings(cwd), cands

    def resume(self, name: str, session_id: str) -> SessionInfo:
        # Relança o pane com `claude --resume <session_id>`, continuando a conversa. Valida o uuid (vai
        # DIRETO pro comando do shell -> barra injecao) e exige o .jsonl existir (nao resume fantasma).
        try:
            uuid.UUID(session_id)
        except (ValueError, AttributeError, TypeError):
            raise ValueError("session_id invalido")
        pane = self._pane_of(name)
        if pane is None:
            raise ValueError("sessao nao encontrada")
        # Tambem AQUI, e nao so no resume_candidates: com session_id vindo do corpo o endpoint pula
        # a listagem de candidatos e cai direto no relance (que mata o pane).
        self._refuse_non_claude_resume(pane)
        cwd = pane["cwd"]
        cdir = _config_dir_of(pane["pid"]) if pane.get("pid") else None
        # Motor da sessão que está morrendo. Sem reaplicar, uma sessão Kimi ressuscita na conta
        # Anthropic continuando um transcript de Kimi — calado. Tem que ler ANTES do kill_session: o
        # /proc do pane some com ele.
        motor = _engine_of(pane["pid"]) if pane.get("pid") else None
        if motor:
            from app import engines
            if motor not in engines.listar():
                # Motor apagado no app depois de a sessão nascer: melhor voltar na conta Anthropic (o
                # badge mostra isso) do que recusar o resume e deixar a sessão inacessível.
                motor = None
        proj = ((cdir / "projects") if cdir else self.projects_dir) / sanitize_cwd(cwd)
        jsonl = proj / f"{session_id}.jsonl"
        if not jsonl.exists():
            raise ValueError("transcript nao encontrado")
        tmux.kill_session(name)
        self._forget(name)
        cmd = f"claude --resume {session_id}"
        if motor:
            cmd = f"cp-engine --exec {motor} -- {cmd}"
        if not tmux.new_session(name, cwd, cmd, str(cdir) if cdir else None):
            raise ValueError("falha ao relançar a sessao")
        # Fixa o transcript resumido no cache: resolve() ja o devolveria (o --resume esta no cmdline),
        # mas semear evita a janela onde o pane ainda esta subindo e cairia no fallback por mtime.
        self._jsonl_cache[name] = str(jsonl)
        return SessionInfo(name=name, cwd=cwd, jsonl=str(jsonl), tracked=True, engine=motor)
