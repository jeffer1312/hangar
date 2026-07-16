import asyncio
import json
import logging
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
from app.chain import ThenLink
from app.pair import PairLink, rename_pair, unpair_both
from app.adapters.codex import sessions as codex_sessions
from app.adapters.codex import adapter as codex_adapter
from app.adapters.codex.appserver import AppServerClient
from app.askquestion import clear_pending_askq
from app.state import classify, _live_spinner, rate_limit_reset
from app.hook_state import hook_state

# Sentinela: distingue "pid nao informado" (resolve sozinho via tmux) de "pid=None" (sem pane).
_UNSET = object()

_log = logging.getLogger("claude_pocket.registry")


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
    # nomes cujo cache veio do fd ABERTO (verdade do FS, nao chute). Mantido entre polls sem fd p/ nao
    # oscilar pro --session-id da cmdline (resume: o id da cmdline nunca vira arquivo). De classe.
    _fd_locked: set[str] = set()
    # DIAG: ultima resolucao logada por nome ("<jsonl>|<tracked>") -> loga so quando MUDA (o momento do
    # split/cross-wire), sem spammar a cada poll. Remover quando o bug de colisao estiver resolvido.
    _last_res: dict[str, str] = {}

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
        try:
            return sum(1 for p in tmux.list_panes_active() if p.get("cwd") == cwd) > 1
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
        """Branch atual do repo em cwd, lida direto de .git/HEAD (sem subprocess -> barato pra rodar
        por sessao na listagem). 'ref: refs/heads/<b>' -> <b>; detached/nao-repo/worktree -> None."""
        if not cwd:
            return None
        try:
            head = Path(cwd, ".git", "HEAD").read_text(encoding="utf-8", errors="replace").strip()
        except OSError:
            return None
        prefix = "ref: refs/heads/"
        return (head[len(prefix):] or None) if head.startswith(prefix) else None

    def list(self) -> list[SessionInfo]:
        # Resolucao de jsonl/tracked de todas as sessoes. Otimizado: UM mapa /proc + UMA chamada tmux
        # (pane_pid em lote) reusados por sessao -> O(P + S·descendentes) em vez de O(S·P). NAO calcula
        # state (sai 'idle' default): este caminho so resolve transcript; quem quer state usa
        # list_with_state(). Usado por varios endpoints que so precisam do jsonl por nome.
        children = _proc_children_map()
        out = []
        sids: dict[str, Optional[str]] = {}
        for p in tmux.list_panes_active():
            jsonl, tracked = self.resolve_tracked(p["name"], p["cwd"], p["pid"], children)
            link = ThenLink(p["name"]).get()
            pair = PairLink(p["name"]).get()
            out.append(SessionInfo(name=p["name"], cwd=p["cwd"], jsonl=jsonl, tracked=tracked,
                                   branch=self._branch_of(p["cwd"]),
                                   then_target=link.get("target") if link else None,
                                   paired_with=pair.get("peer") if pair else None))
            sids[p["name"]] = self._repl_sid(p["pid"], children)
        # Guarda de colisao: 2+ sessoes no mesmo jsonl -> so a dona mantem (mata a duplicata/cross-wire).
        self._dedupe_collisions(out, sids)
        # Sessoes Codex: nao vivem em tmux -> vem dos sidecars duraveis (sobrevivem a restart do
        # backend; o historico esta no rollout). jsonl = rollout_path; tracked=True (identidade
        # deterministica via thread_id). O client vivo e reaberto sob demanda (ensure_running).
        for meta in codex_sessions.list_all():
            out.append(SessionInfo(
                name=meta["name"], cwd=meta.get("cwd"), jsonl=meta.get("rollout_path"),
                provider="codex", tracked=True,
                branch=self._branch_of(meta.get("cwd")),
                then_target=(ThenLink(meta["name"]).get() or {}).get("target"),
                paired_with=(PairLink(meta["name"]).get() or {}).get("peer"),
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
            if marker and marker[0] != "awaiting_input":
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
            for info, c, frame in zip(pending, classified, frames):
                info.state = c[0]
                info.label = c[1]
                info.question = c[2]
                info.options = c[3]
                info.last_activity = _jsonl_mtime(info.jsonl)
                # Rate-limit radar (feature #8): so pane-derivado, entao so nas infos raspadas aqui
                # (marker path fica com o default False/None, igual a label/question/options).
                info.limit_reset = rate_limit_reset(frame)
                info.limited = info.limit_reset is not None
        # Travada (feature #7): "working" ha mais de CP_STALL_SECONDS sem o transcript avancar. So o
        # bool derivado pra UI/sig — o push (1x, com dedupe) e responsabilidade do stall_watch, nao daqui.
        now = time.time()
        for info in infos:
            info.stalled = (
                info.state == "working"
                and info.last_activity is not None
                and (now - info.last_activity) > settings.stall_seconds
            )
        return infos

    def create(self, name: str, cwd: str, config_dir: str | None = None,
               resume_session_id: str | None = None, provider: str = "claude") -> SessionInfo:
        # Nome tmux nao aceita "."/":"/espaco -> sanitiza igual ao rename. Varias sessoes na MESMA
        # pasta sao permitidas: cada uma tem nome unico + --session-id proprio -> jsonl proprio.
        name = re.sub(r"[^A-Za-z0-9_-]", "-", name.strip()).strip("-")
        if not name:
            raise ValueError("nome invalido")
        # Codex nao e tmux: o caminho async (spawn do app-server, thread/start) roda no loop
        # principal via create_codex(); o create() sync spawnaria o AppServerClient num loop
        # descartavel (asyncio.run) que morre ao retornar -> orfanaria o subprocess/reader task.
        # Por isso o create() sync e Claude-only e recusa Codex alto (Task 6 fia o endpoint async).
        if provider == "codex":
            raise ValueError("sessoes Codex sao criadas via create_codex (async)")
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
        base = (Path(config_dir) / "projects") if config_dir else self.projects_dir
        jsonl = str(base / sanitize_cwd(cwd) / f"{sid}.jsonl")
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
        self._jsonl_cache[name] = jsonl
        return SessionInfo(name=name, cwd=cwd, jsonl=jsonl, provider=provider)

    async def create_codex(self, name: str, cwd: str) -> SessionInfo:
        # Caminho Codex do create (NAO-tmux): spawna o app-server, abre um thread, grava o sidecar
        # duravel e anexa o client vivo. async porque o AppServerClient precisa viver no loop
        # principal (o mesmo que serve SSE/send_prompt depois) -- ver nota no create() sync.
        name = re.sub(r"[^A-Za-z0-9_-]", "-", name.strip()).strip("-")
        if not name:
            raise ValueError("nome invalido")
        # Unicidade contra sessoes tmux (Claude) E sidecars Codex existentes.
        if tmux.has_session(name) or codex_sessions.exists(name):
            raise ValueError("ja existe uma sessao com esse nome")
        client = AppServerClient()
        try:
            await client.start()
            await client.request("initialize", {
                "clientInfo": codex_adapter._CLIENT_INFO, "capabilities": None})
            result = await client.request("thread/start", {
                "cwd": cwd,
                "sandbox": codex_adapter._SANDBOX,
                "approvalPolicy": codex_adapter._APPROVAL,
            })
        except Exception:
            # Falha no handshake: nao deixa o subprocess orfao.
            await client.close()
            raise
        thread = result.get("thread") or {}
        thread_id = thread.get("id")
        rollout_path = thread.get("path")
        if not thread_id or not rollout_path:
            await client.close()
            raise ValueError("thread/start nao devolveu id/path")
        # save() (mkdir+write_text -> pode dar OSError: disco cheio/permissao) e attach() rodam com o
        # app-server JA spawnado -> qualquer falha aqui tem que fechar o client, senao vira orfao. Se
        # save deu certo mas attach falhou, remove o sidecar recem-escrito (estado consistente: nao
        # fica sidecar apontando pra um client fechado).
        try:
            # Sidecar duravel: sobrevive ao restart do backend (identidade + ponteiro pro rollout).
            codex_sessions.save(name, thread_id, rollout_path, cwd)
            # Client vivo (efemero) anexado no adapter; limpa fila/then herdados de nome reusado.
            # default_model/effort: thread/start ja devolve o modelo default da thread (ex.
            # "gpt-5.6-sol") -- passa pro attach so pra DISPLAY (pill/statusline); a sessao nova
            # ainda nao tem escolha explicita (model=None acima).
            from app.adapters import get_adapter
            get_adapter("codex").attach(name, client, thread_id,
                                         default_model=result.get("model"),
                                         default_effort=result.get("effort"))
        except Exception:
            await client.close()
            codex_sessions.delete(name)  # idempotente; remove sidecar orfao se save ja tinha passado
            raise
        PromptQueue(name).clear()
        ThenLink(name).clear()
        return SessionInfo(name=name, cwd=cwd, jsonl=rollout_path, provider="codex")

    def rename(self, old: str, new: str) -> None:
        # Cache e keyed por NOME -> ao renomear, move a entrada pro nome novo e esquece o velho. Senao
        # o nome velho apontaria pro jsonl pra sempre (reuso futuro = transcript errado) e o nome novo
        # cairia no fallback newest-by-mtime ate um sinal confiavel reaparecer.
        j = self._jsonl_cache.pop(old, None)
        if j is not None:
            self._jsonl_cache[new] = j
        if old in self._fd_locked:           # move o fd-lock junto com o cache
            self._fd_locked.discard(old)
            self._fd_locked.add(new)
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

    def kill(self, name: str) -> None:
        # Sessao Codex (tem sidecar duravel): fecha o client vivo (SIGTERM sync via adapter -> o read
        # loop no loop principal ve o EOF), apaga o sidecar (nao reaparece na lista / nao resume) e
        # limpa fila/then. NAO toca tmux (nao ha pane).
        if codex_sessions.exists(name):
            from app.adapters import get_adapter
            get_adapter("codex").close_sync(name)
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
        tmux.kill_session(name)
        self._forget(name)  # cache invalido: nome pode ser reusado por outra sessao depois
        # Sessao morta nao deixa fila pra tras: senao acumula orfaos e uma futura sessao de mesmo
        # nome herdaria essas entradas como bubble-fantasma (mesmo motivo do clear no create()).
        PromptQueue(name).clear()
        ThenLink(name).clear()  # mesmo motivo, pro vinculo 'then' (feature #12)
        self._clear_pair(name)

    @staticmethod
    def _clear_pair(name: str) -> None:
        # Sessão morta desfaz o pareamento DOS DOIS lados (unpair_both: sob lock): o sidecar do par
        # apontaria pra um fantasma (badge preso). Best-effort, nunca bloqueia o kill — mas LOGA:
        # engolir calado deixava o badge-fantasma indiagnosticável.
        try:
            unpair_both(name)
        except Exception as e:
            _log.warning("kill(%s): falha ao limpar pareamento: %r", name, e)

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

    def resume_candidates(self, name: str) -> tuple[str, bool, list[dict]]:
        # (cwd, ambiguo, candidatos). ambiguo = ha OUTRA sessao tmux no mesmo cwd -> o "mais recente por
        # mtime" pode ser de outra sessao (a UI pede confirmacao). candidatos = ate 6 jsonls recentes do
        # cwd, cada um com preview + se ja esta em uso por outra sessao viva.
        pane = self._pane_of(name)
        if pane is None:
            raise ValueError("sessao nao encontrada")
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
        cwd = pane["cwd"]
        cdir = _config_dir_of(pane["pid"]) if pane.get("pid") else None
        proj = ((cdir / "projects") if cdir else self.projects_dir) / sanitize_cwd(cwd)
        jsonl = proj / f"{session_id}.jsonl"
        if not jsonl.exists():
            raise ValueError("transcript nao encontrado")
        tmux.kill_session(name)
        self._forget(name)
        if not tmux.new_session(name, cwd, f"claude --resume {session_id}",
                                str(cdir) if cdir else None):
            raise ValueError("falha ao relançar a sessao")
        # Fixa o transcript resumido no cache: resolve() ja o devolveria (o --resume esta no cmdline),
        # mas semear evita a janela onde o pane ainda esta subindo e cairia no fallback por mtime.
        self._jsonl_cache[name] = str(jsonl)
        return SessionInfo(name=name, cwd=cwd, jsonl=str(jsonl), tracked=True)
