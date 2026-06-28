import os
import re
import uuid
from pathlib import Path
from typing import Optional
from app import tmux
from app.config import settings
from app.models import SessionInfo
from app.pqueue import PromptQueue


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
        if target.endswith(".jsonl") and target.startswith(base + os.sep):
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

    def resolve(self, name: str, cwd: str) -> Optional[str]:
        return self.resolve_tracked(name, cwd)[0]

    def resolve_tracked(self, name: str, cwd: str) -> tuple[Optional[str], bool]:
        # Mapeia uma sessao tmux -> o jsonl CERTO + se o vinculo e CONFIAVEL (tracked).
        # tracked=True so com sinal DETERMINISTICO: --session-id do cmdline, fd aberto, ou cache
        # (semeado por um desses / pelo create()). tracked=False = chute newest-by-mtime, que COLIDE
        # com varias sessoes bare no mesmo cwd -> a UI marca "sem id" e desliga o chat (evita mostrar
        # /trocar transcript errado). Determinismo so com --session-id: o "+" do app, ou o wrapper
        # `claude --session-id <uuid>` no terminal.
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
                    cdir = _config_dir_of(p)
                    proj = (cdir / "projects") if cdir else self.projects_dir
                    j = str(proj / sanitize_cwd(cwd) / f"{sid}.jsonl")
                    self._jsonl_cache[name] = j
                    return j, True
            # 2. fd aberto (confiavel quando presente; raro, o claude nao segura em idle).
            for p in pids:
                cdir = _config_dir_of(p)
                j = _open_jsonl(p, (cdir / "projects") if cdir else self.projects_dir)
                if j:
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
        out = []
        for s in tmux.list_sessions():
            jsonl, tracked = self.resolve_tracked(s["name"], s["cwd"])
            out.append(SessionInfo(name=s["name"], cwd=s["cwd"], jsonl=jsonl, tracked=tracked))
        return out

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
        tmux.kill_session(name)
        self._forget(name)  # cache invalido: nome pode ser reusado por outra sessao depois
        # Sessao morta nao deixa fila pra tras: senao acumula orfaos e uma futura sessao de mesmo
        # nome herdaria essas entradas como bubble-fantasma (mesmo motivo do clear no create()).
        PromptQueue(name).clear()
