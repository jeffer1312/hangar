import asyncio
import os
import re
import time
import uuid
from pathlib import Path
from typing import Optional
from app import tmux
from app.config import settings
from app.models import SessionInfo
from app.pqueue import PromptQueue
from app.askquestion import clear_pending_askq
from app.state import classify, _live_spinner
from app.hook_state import hook_state

# Sentinela: distingue "pid nao informado" (resolve sozinho via tmux) de "pid=None" (sem pane).
_UNSET = object()


def sanitize_cwd(cwd: str) -> str:
    return re.sub(r"[^A-Za-z0-9]", "-", cwd)


def _proc_children_map() -> dict[int, list[int]]:
    # Mapa ppid->filhos varrendo o /proc/*/stat UMA vez. Caro (le o stat de todo processo da maquina);
    # por isso a listagem constroi UM mapa e reusa pra todas as sessoes (em vez de re-varrer por sessao).
    children: dict[int, list[int]] = {}
    try:
        entries = os.listdir("/proc")
    except OSError:
        return children
    for entry in entries:
        if not entry.isdigit():
            continue
        try:
            # ppid = 4o campo do stat; usar rsplit(')') pra nao quebrar com espaco/parenteses no comm.
            with open(f"/proc/{entry}/stat", encoding="utf-8", errors="replace") as fh:
                after = fh.read().rsplit(")", 1)[-1].split()
            ppid = int(after[1])
        except (OSError, ValueError, IndexError):
            continue
        children.setdefault(ppid, []).append(int(entry))
    return children


def _descendant_pids(root: int, children: Optional[dict[int, list[int]]] = None) -> list[int]:
    # root + todos os descendentes. O claude pode ser filho do shell do pane (sessao manual) ou o
    # proprio pane (app-criada com `claude` como comando). children: mapa pre-construido reusavel; se
    # None, constroi sob demanda (caminho single-session do SSE).
    if children is None:
        children = _proc_children_map()
    out, stack = [], [root]
    while stack:
        p = stack.pop()
        out.append(p)
        stack.extend(children.get(p, []))
    return out


def _open_jsonl(pid: int, projects_dir: Path) -> Optional[str]:
    # 1o fd aberto apontando pra um *.jsonl dentro do projects_dir (= o transcript ativo do claude).
    # NOTA: o claude NAO segura esse fd em idle (abre/escreve/fecha) -> quase sempre None. Mantido so
    # como sinal extra confiavel QUANDO presente; a resolucao real vem do --session-id do cmdline.
    fddir = f"/proc/{pid}/fd"
    try:
        fds = os.listdir(fddir)
    except OSError:
        return None
    base = str(projects_dir)
    for fd in fds:
        try:
            target = os.readlink(f"{fddir}/{fd}")
        except OSError:
            continue
        if target.endswith(".jsonl") and target.startswith(base + os.sep):
            return target
    return None


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


# Resume: o claude relanca com um --session-id NOVO (cmdline) mas CONTINUA escrevendo o transcript
# ANTIGO -> o jsonl do id novo NUNCA aparece. Num poll so nao da pra distinguir "sessao nova (arquivo
# a nascer)" de "resume (arquivo nunca vem, outro transcript vivo)". Discrimina por PERSISTENCIA: o
# sid_jsonl ausente por > _RESUME_GRACE s, com um transcript VIVO no cwd, = resume -> segue o vivo.
_RESUME_GRACE = 8.0          # s sem o sid_jsonl aparecer antes de tratar como resume
_RESUME_LIVE_WINDOW = 300.0  # s: jsonl com mtime nesta janela = transcript ATIVO (nao um velho qualquer)


def _newest_live(projdir: Path, exclude: set[str], window: float = _RESUME_LIVE_WINDOW) -> Optional[str]:
    # Mais recente *.jsonl do projdir (fora de `exclude`) cujo mtime esta dentro de `window`s de agora,
    # ou None. Distingue um transcript sendo escrito AGORA (resume) de jsonls velhos do mesmo cwd.
    now = time.time()
    best, best_mt = None, 0.0
    try:
        for f in projdir.glob("*.jsonl"):
            if os.path.realpath(str(f)) in exclude:
                continue
            try:
                mt = f.stat().st_mtime
            except OSError:
                continue
            if mt > best_mt and (now - mt) <= window:
                best, best_mt = str(f), mt
    except OSError:
        pass
    return best


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


def _cmdline(pid: int) -> str:
    # cmdline crua do processo (args separados por NUL -> espaco).
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            return fh.read().replace(b"\x00", b" ").decode(errors="replace")
    except OSError:
        return ""


def _config_dir_of(pid: int) -> Optional[Path]:
    # CLAUDE_CONFIG_DIR do processo claude (setado pelo alias/picker). None se ausente -> fallback.
    try:
        with open(f"/proc/{pid}/environ", "rb") as fh:
            for kv in fh.read().split(b"\x00"):
                if kv.startswith(b"CLAUDE_CONFIG_DIR="):
                    # surrogateescape = round-trip fiel dos bytes POSIX (a camada de fs do Python usa o
                    # mesmo) -> o Path ainda casa no disco mesmo com path nao-UTF-8. "replace" corromperia.
                    return Path(kv.split(b"=", 1)[1].decode("utf-8", "surrogateescape"))
    except OSError:
        return None
    return None


class SessionRegistry:
    # Cache name -> ultimo jsonl resolvido por sinal CONFIAVEL (cmdline --session-id / fd). De classe
    # (compartilhado entre instancias: api.registry e sse._registry). Estabiliza a resolucao quando o
    # processo que carrega o --session-id SOME transitoriamente (a sessao dirigida por job/harness
    # spawna claude por turno) -> sem isto a resolucao oscilava pro mtime e o watcher do SSE limpava o
    # chat. Atualizado quando um sinal confiavel reaparece (ex: /clear -> session-id novo).
    _jsonl_cache: dict[str, str] = {}
    # name -> monotonic do 1o poll em que o sid_jsonl estava ausente COM um transcript vivo no cwd.
    # Mede a graca de resume (ver _RESUME_GRACE): sessao nova escreve seu sid_jsonl em segundos; se
    # apos a graca ele AINDA nao existe, e resume -> segue o transcript vivo. De classe igual ao cache.
    _resume_seen: dict[str, float] = {}

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

    def resolve(self, name: str, cwd: str) -> Optional[str]:
        return self.resolve_tracked(name, cwd)[0]

    def resolve_tracked(self, name: str, cwd: str, pid=_UNSET,
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
                    return j, True
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
                    projdir = proj / sanitize_cwd(cwd)
                    sid_jsonl = str(projdir / f"{sid}.jsonl")
                    if os.path.exists(sid_jsonl):
                        # boot-id ja escrito -> _newest_after_clear trata o /clear (segue o pos-clear).
                        self._resume_seen.pop(name, None)
                        j = _newest_after_clear(projdir, sid_jsonl, aux_open)
                    else:
                        # sid_jsonl AUSENTE: sessao nova (arquivo a nascer) OU resume (id novo, transcript
                        # antigo; o id novo nunca aparece). Travado num transcript real? mantem (resume e
                        # permanente, sem oscilar no idle). Senao, mede a graca contra um transcript VIVO.
                        cached = self._jsonl_cache.get(name)
                        if cached and cached != sid_jsonl and os.path.exists(cached):
                            j = cached
                        else:
                            live = _newest_live(projdir, aux_open)
                            if live:
                                first = self._resume_seen.setdefault(name, time.monotonic())
                                j = live if (time.monotonic() - first) >= _RESUME_GRACE else sid_jsonl
                            else:
                                self._resume_seen.pop(name, None)
                                j = sid_jsonl
                    self._jsonl_cache[name] = j
                    return j, True
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

    def list(self) -> list[SessionInfo]:
        # Resolucao de jsonl/tracked de todas as sessoes. Otimizado: UM mapa /proc + UMA chamada tmux
        # (pane_pid em lote) reusados por sessao -> O(P + S·descendentes) em vez de O(S·P). NAO calcula
        # state (sai 'idle' default): este caminho so resolve transcript; quem quer state usa
        # list_with_state(). Usado por varios endpoints que so precisam do jsonl por nome.
        children = _proc_children_map()
        out = []
        for p in tmux.list_panes_active():
            jsonl, tracked = self.resolve_tracked(p["name"], p["cwd"], p["pid"], children)
            out.append(SessionInfo(name=p["name"], cwd=p["cwd"], jsonl=jsonl, tracked=tracked))
        return out

    async def list_with_state(self) -> list[SessionInfo]:
        # Listagem COM estado vivo por sessao (pro /api/sessions). Faz a resolucao otimizada (sync, num
        # thread) e por cima classifica o pane de cada sessao concorrentemente.
        infos = await asyncio.to_thread(self.list)
        if not infos:
            return infos
        # Estado pela marca dos hooks quando existe (custo ~0); senao cai no pane (fallback).
        def _sid(jsonl):
            return Path(jsonl).stem if jsonl else None
        pending = []  # infos sem marcador -> precisa raspar o pane
        for info in infos:
            marker = hook_state.get_state(_sid(info.jsonl))
            if marker:
                info.state = marker[0]
                info.last_activity = _jsonl_mtime(info.jsonl)
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
            for info, c in zip(pending, classified):
                info.state = c[0]
                info.last_activity = _jsonl_mtime(info.jsonl)
        return infos

    def create(self, name: str, cwd: str, config_dir: str | None = None) -> SessionInfo:
        # Nome tmux nao aceita "."/":"/espaco -> sanitiza igual ao rename. Varias sessoes na MESMA
        # pasta sao permitidas: cada uma tem nome unico + --session-id proprio -> jsonl proprio.
        name = re.sub(r"[^A-Za-z0-9_-]", "-", name.strip()).strip("-")
        if not name:
            raise ValueError("nome invalido")
        if tmux.has_session(name):
            raise ValueError("ja existe uma sessao com esse nome")
        sid = str(uuid.uuid4())
        base = (Path(config_dir) / "projects") if config_dir else self.projects_dir
        jsonl = str(base / sanitize_cwd(cwd) / f"{sid}.jsonl")
        if not tmux.new_session(name, cwd, f"claude --session-id {sid}", config_dir):
            raise ValueError("falha ao criar sessao no tmux")
        # Sessao NOVA = sid novo = transcript fresco. A fila duravel e keyed pelo NOME (sobrevive ao
        # fim da sessao antiga), entao entradas remanescentes de uma sessao morta de mesmo nome
        # fantasmariam aqui via merged_history. Limpa igual o /clear faz. Seguro: a sessao nova ainda
        # nem aceitou input, nao ha fila legitima a preservar.
        PromptQueue(name).clear()
        # Fixa o jsonl FRESCO no cache na hora: resolve() devolve este uuid mesmo antes do claude
        # escrever o arquivo, evitando o fallback newest-by-mtime pescar um jsonl ja existente da pasta.
        self._jsonl_cache[name] = jsonl
        return SessionInfo(name=name, cwd=cwd, jsonl=jsonl)

    def rename(self, old: str, new: str) -> None:
        # Cache e keyed por NOME -> ao renomear, move a entrada pro nome novo e esquece o velho. Senao
        # o nome velho apontaria pro jsonl pra sempre (reuso futuro = transcript errado) e o nome novo
        # cairia no fallback newest-by-mtime ate um sinal confiavel reaparecer.
        j = self._jsonl_cache.pop(old, None)
        if j is not None:
            self._jsonl_cache[new] = j
        # A fila duravel tambem e keyed por NOME -> move junto, senao a sessao renomeada perde as
        # entradas nao-drenadas e elas ficam orfas no nome velho (fantasma se reusarem `old`).
        PromptQueue(old).rename(new)

    def kill(self, name: str) -> None:
        # Limpa o sidecar do AskUserQuestion ANTES de matar (precisa do processo vivo pra resolver o
        # jsonl), best-effort: cleanup nunca bloqueia/quebra o kill. Senao um stale reabriria o stepper
        # numa sessao futura de mesmo nome.
        try:
            jsonl = next((s.jsonl for s in self.list() if s.name == name), None)
            if jsonl:
                clear_pending_askq(jsonl)
        except Exception:
            pass
        tmux.kill_session(name)
        self._forget(name)  # cache invalido: nome pode ser reusado por outra sessao depois
        # Sessao morta nao deixa fila pra tras: senao acumula orfaos e uma futura sessao de mesmo
        # nome herdaria essas entradas como bubble-fantasma (mesmo motivo do clear no create()).
        PromptQueue(name).clear()
