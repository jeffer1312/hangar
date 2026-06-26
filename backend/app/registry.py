import os
import re
import uuid
from pathlib import Path
from typing import Optional
from app import tmux
from app.config import settings
from app.models import SessionInfo


def sanitize_cwd(cwd: str) -> str:
    return re.sub(r"[^A-Za-z0-9]", "-", cwd)


def _descendant_pids(root: int) -> list[int]:
    # root + todos os descendentes, via mapa ppid->filhos do /proc/*/stat. O claude pode ser filho do
    # shell do pane (sessao manual) ou o proprio pane (app-criada com `claude` como comando).
    children: dict[int, list[int]] = {}
    try:
        entries = os.listdir("/proc")
    except OSError:
        return [root]
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
        if target.endswith(".jsonl") and target.startswith(base):
            return target
    return None


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


def _cmdline(pid: int) -> str:
    # cmdline crua do processo (args separados por NUL -> espaco).
    try:
        with open(f"/proc/{pid}/cmdline", "rb") as fh:
            return fh.read().replace(b"\x00", b" ").decode(errors="replace")
    except OSError:
        return ""


class SessionRegistry:
    # Cache name -> ultimo jsonl resolvido por sinal CONFIAVEL (cmdline --session-id / fd). De classe
    # (compartilhado entre instancias: api.registry e sse._registry). Estabiliza a resolucao quando o
    # processo que carrega o --session-id SOME transitoriamente (a sessao dirigida por job/harness
    # spawna claude por turno) -> sem isto a resolucao oscilava pro mtime e o watcher do SSE limpava o
    # chat. Atualizado quando um sinal confiavel reaparece (ex: /clear -> session-id novo).
    _jsonl_cache: dict[str, str] = {}

    def __init__(self, projects_dir: Path | None = None):
        self.projects_dir = Path(projects_dir or settings.projects_dir)

    def resolve_jsonl(self, cwd: str) -> Optional[str]:
        # FALLBACK por cwd: jsonl mais recente do dir do projeto. So usado quando nao ha --session-id
        # nem fd aberto. NAO confiavel com varias sessoes no mesmo cwd (colide) -> por isso o
        # cmdline --session-id (em resolve()) vem primeiro.
        proj = self.projects_dir / sanitize_cwd(cwd)
        if not proj.is_dir():
            return None
        files = sorted(proj.glob("*.jsonl"), key=lambda f: f.stat().st_mtime, reverse=True)
        return str(files[0]) if files else None

    def resolve(self, name: str, cwd: str) -> Optional[str]:
        # Mapeia uma sessao tmux -> o jsonl CERTO. So sinais CONFIAVEIS: o --session-id do cmdline e o fd
        # aberto. Sem eles (manual `claude` puro), cai no newest-by-mtime, que PODE colidir com varias
        # sessoes no mesmo cwd -> mostra a conversa ATIVA pras duas (ambiguo, mas conteudo real). NAO
        # usar heuristica de btime: jsonls de sessoes transitorias caem na mesma janela de tempo e a
        # atribuicao sai ERRADA (apontava pra transcript vazio/de outra sessao). Determinismo so com
        # --session-id -> usar o "+" do app, ou `claude --session-id <uuid>` no manual.
        pid = tmux.pane_pid(name)
        if pid is not None:
            pids = _descendant_pids(pid)
            # 1. cmdline --session-id (DETERMINISTICO; app-created sempre, manual com flag). Vale mesmo
            #    sem o arquivo existir ainda (sessao recem-criada) -> o tailer segue quando aparecer.
            #    PULA os processos auxiliares da arvore do claude, que carregam um --session-id PROPRIO
            #    (transitorio) != o do REPL principal -> sem isto resolvia pro jsonl errado/inexistente:
            #      - `claude daemon` + bg-pty-host/spare (sockets em /tmp/cc-daemon-*): contem "daemon"/"--bg-"
            #      - SUB-AGENTES (`--agent`): cada Task/subagent roda seu proprio session-id.
            for p in pids:
                cmd = _cmdline(p)
                if "daemon" in cmd or "--bg-" in cmd or "--agent" in cmd:
                    continue
                sid = _session_id_from_cmdline(cmd)
                if sid:
                    j = str(self.projects_dir / sanitize_cwd(cwd) / f"{sid}.jsonl")
                    self._jsonl_cache[name] = j
                    return j
            # 2. fd aberto (confiavel quando presente; raro, o claude nao segura em idle).
            for p in pids:
                j = _open_jsonl(p, self.projects_dir)
                if j:
                    self._jsonl_cache[name] = j
                    return j
        # 3. cache: ultimo sinal confiavel. Estabiliza quando o processo com --session-id some
        #    transitoriamente (senao a resolucao oscilava pro mtime e o watcher limpava o chat).
        cached = self._jsonl_cache.get(name)
        if cached:
            return cached
        # 4. fallback: mais recente por mtime (ambiguo com varias sessoes bare no mesmo cwd).
        return self.resolve_jsonl(cwd)

    def _forget(self, name: str) -> None:
        self._jsonl_cache.pop(name, None)

    def list(self) -> list[SessionInfo]:
        out = []
        for s in tmux.list_sessions():
            jsonl = self.resolve(s["name"], s["cwd"])
            out.append(SessionInfo(name=s["name"], cwd=s["cwd"], jsonl=jsonl))
        return out

    def create(self, name: str, cwd: str) -> SessionInfo:
        sid = str(uuid.uuid4())
        tmux.new_session(name, cwd, f"claude --session-id {sid}")
        return SessionInfo(name=name, cwd=cwd, jsonl=str(self.projects_dir / sanitize_cwd(cwd) / f"{sid}.jsonl"))

    def kill(self, name: str) -> None:
        tmux.kill_session(name)
        self._forget(name)  # cache invalido: nome pode ser reusado por outra sessao depois
