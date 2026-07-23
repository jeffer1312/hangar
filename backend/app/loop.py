"""Loop runner (harness bloco A): objetivo -> executa -> verifica -> re-prompta -> para.
Sidecar JSON por sessao FONTE em ".claude-pocket-loop", keyed pelo NOME (sobrevive ao /clear),
mesmo padrao do chain/pqueue. Um loop por sessao; loop novo sobrescreve o anterior.
Spec: docs/superpowers/specs/2026-07-22-loop-runner-design.md"""
import json
import shlex
import subprocess
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Optional

from app.config import settings
from app.pqueue import _sanitize

ACTIVE = {"running", "paused_awaiting", "done_claimed"}

# Push SO nesta transicao de status (regra 8). Re-prompt comum e normalizacao
# paused_awaiting->running NUNCA notificam (senao cada permission-prompt vira push duplicado).
_NOTIFY_STATUSES = {"done", "done_claimed", "stopped", "exhausted", "failed"}

_TAIL = 8 * 1024

# Todo read-modify-write do sidecar (run_tick E endpoints da Task 6) roda sob este lock.
# O check de 600s roda FORA do lock; depois re-adquire, re-le o sidecar e aborta se o status mudou.
_lock = threading.Lock()


def _loop_dir() -> Path:
    d = Path(settings.projects_dir).parent / ".claude-pocket-loop"
    d.mkdir(parents=True, exist_ok=True)
    return d


def new_loop(goal: str, check_cmd: str | None, max_iters: int, require_branch: bool) -> dict:
    return {
        "goal": goal, "check_cmd": check_cmd, "max_iters": max_iters,
        "require_branch": require_branch, "status": "running", "iter": 0,
        "goal_entry_id": None, "goal_delivered_ts": None, "history": [],
        "started_ts": time.time(), "ended_ts": None, "ended_reason": None,
    }


class LoopLink:
    """Sidecar de UMA sessao (<nome>.json). get/set/update/clear; escrita atomica."""

    def __init__(self, name: str):
        self.path = _loop_dir() / f"{_sanitize(name)}.json"

    def get(self) -> dict | None:
        try:
            return json.loads(self.path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError, ValueError):
            return None

    def set(self, data: dict) -> None:
        tmp = self.path.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, ensure_ascii=False), encoding="utf-8")
        tmp.replace(self.path)

    def update(self, **fields) -> dict | None:
        cur = self.get()
        if cur is None:
            return None
        cur.update(fields)
        self.set(cur)
        return cur

    def clear(self) -> None:
        try:
            self.path.unlink()
        except OSError:
            pass


@dataclass
class TickCtx:
    """Dependencias do tick, TODAS injetadas -> teste nao toca disco/tmux de verdade."""
    cwd: str                                          # cwd da sessao (info.cwd do registry)
    jsonl: str                                        # transcript corrente (resolvido AGORA)
    deliver: Callable[[str], bool]                    # entrega prompt na TUI; True = entregue
    enqueue: Callable[[str], None]                    # fallback pqueue.append
    notify: Callable[[str, str], None]                # push (session_name, corpo)
    automations: Callable[[], bool]                   # config.automations_enabled
    branch: Callable[[str], Optional[str]]            # git_ops.branch_of
    last_assistant: Callable[[str], Optional[str]]    # transcript.last_assistant_text
    run_check: Callable[[str, str], tuple[int, str]]  # (cmd, cwd) -> (exit, tail)
    entry_delivered: Callable[[str], Optional[bool]]  # pqueue: entry_id -> delivered? (None = sumiu)


def _body(status: str, reason: str | None) -> str:
    return {
        "done": f"loop concluído: {reason}",
        "done_claimed": "Claude declarou pronto — confirmar?",
        "stopped": f"loop parado: {reason}",
        "exhausted": f"loop esgotou as iterações: {reason}",
        "failed": f"loop falhou: {reason}",
    }.get(status, reason or status)


def _end(link: "LoopLink", name: str, status: str, reason: str,
         notify: Callable[[str, str], None]) -> dict | None:
    """Transiciona pra estado terminal/atencao (grava status/ended_ts/ended_reason + push com corpo
    canonico do _body). Notify SO se o status entra na whitelist (regra 8). Caller segura _lock.
    Fonte unica pros endpoints (resolve/stop) e o run_tick -> zero string de push duplicada."""
    fields = {"status": status, "ended_reason": reason}
    if status != "done_claimed":   # done_claimed NAO e terminal (aguarda confirmacao humana)
        fields["ended_ts"] = time.time()
    d = link.update(**fields)
    if status in _NOTIFY_STATUSES:
        notify(name, _body(status, reason))
    return d


def _reprompt(link: "LoopLink", d: dict, tail: str, check_exit: int | None,
              deliver: Callable[[str], bool], enqueue: Callable[[str], None]) -> None:
    """Append no history, iter++, monta e entrega o re-prompt. Caller segura _lock e ja validou
    iter+1 <= max_iters. REUSADO pelo /resolve (reject) — zero copia divergente."""
    now = time.time()
    d["history"].append({"n": d["iter"] + 1, "ts": now, "check_exit": check_exit, "tail": tail})
    d["history"] = d["history"][-20:]
    d["iter"] += 1
    link.set(d)
    prompt = (
        f"[loop {d['iter']}/{d['max_iters']}] Objetivo: {d['goal']}\n"
        + (f"O comando de verificação falhou (exit {check_exit}). Saída (cauda):\n{tail}\n"
           if d["check_cmd"] else "A tarefa ainda não foi confirmada como concluída. Continue.\n")
        + "Não edite arquivos de teste nem o comando de verificação; corrija o código."
    )
    if not deliver(prompt):
        enqueue(prompt)


def run_tick(name: str, ctx: TickCtx) -> dict | None:
    """Um tick do loop. Sincrono/testavel. Ver regras numeradas no plano/spec."""
    link = LoopLink(name)
    with _lock:
        d = link.get()
        # 1: sem sidecar ou status inativo -> no-op
        if d is None or d["status"] not in ("running", "paused_awaiting"):
            return None
        # 1: normaliza paused_awaiting -> running (SILENCIOSO, nunca notifica)
        if d["status"] == "paused_awaiting":
            d["status"] = "running"
            link.set(d)
        # 2: ancora concreta — so tica depois do goal constar entregue na pqueue
        if d["goal_delivered_ts"] is None:
            delivered = ctx.entry_delivered(d["goal_entry_id"])
            if delivered is False:
                return None                       # goal ainda nao digitado na TUI
            d["goal_delivered_ts"] = time.time()  # True/None -> segue
            link.set(d)
        # 3: kill-switch mestre -> parada NOTIFICADA (nunca silenciosa)
        if not ctx.automations():
            return _end(link, name, "stopped", "automations_disabled", ctx.notify)
        # 4: guardrail de branch, re-verificado a cada tick
        br = ctx.branch(ctx.cwd)
        if d["require_branch"] and br in ("main", "master"):
            return _end(link, name, "stopped", f"branch {br}", ctx.notify)
        check_cmd = d["check_cmd"]

    # FORA do lock: o check pode segurar 600s.
    exit_code: int | None = None
    tail = ""
    txt: str | None = None
    if check_cmd:
        exit_code, tail = ctx.run_check(check_cmd, ctx.cwd)
    else:
        txt = ctx.last_assistant(ctx.jsonl)

    with _lock:
        d = link.get()
        # usuario deu DELETE/resolve durante o check -> nao sobrescreve
        if d is None or d["status"] not in ("running", "paused_awaiting"):
            return None
        if check_cmd:
            # 5
            if exit_code == 0:
                return _end(link, name, "done", "check passou", ctx.notify)
            if exit_code == -404:
                return _end(link, name, "failed", f"check_cmd não executável: {tail}", ctx.notify)
            if d["history"] and d["history"][-1]["tail"] == tail:
                return _end(link, name, "stopped", "estagnado — mesmo erro 2x", ctx.notify)
            # 7
            if d["iter"] + 1 > d["max_iters"]:
                return _end(link, name, "exhausted", f"esgotou {d['max_iters']} iterações", ctx.notify)
            _reprompt(link, d, tail, exit_code, ctx.deliver, ctx.enqueue)
            return link.get()
        # 6: sem check_cmd
        if "LOOP_DONE" in (txt or ""):
            return _end(link, name, "done_claimed", "declarou pronto — confirmar?", ctx.notify)
        if d["iter"] + 1 > d["max_iters"]:
            return _end(link, name, "exhausted", f"esgotou {d['max_iters']} iterações", ctx.notify)
        _reprompt(link, d, "(sem check — iteração de continuação)", None, ctx.deliver, ctx.enqueue)
        return link.get()


def _run_check(cmd: str, cwd: str) -> tuple[int, str]:
    """Check determinístico real. argv (nunca shell=True); timeout 600s; cauda 8KB."""
    try:
        p = subprocess.run(shlex.split(cmd), cwd=cwd, capture_output=True,
                           text=True, timeout=600)
    except (ValueError, OSError) as e:
        # ValueError = shlex.split com aspas desbalanceadas (ANTES do subprocess); OSError cobre
        # FileNotFoundError (comando inexistente) e afins. Sem capturar, a thread do tick morreria
        # e o loop travaria em running pra sempre.
        return (-404, str(e))
    except subprocess.TimeoutExpired:
        return (124, "timeout 600s")
    out = ((p.stdout or "") + "\n" + (p.stderr or "")).strip()
    return (p.returncode, out[-_TAIL:])


_REFINE_TIMEOUT = 60
_REFINE_MAX = 2000

_REFINE_SYSTEM = (
    "Você reescreve OBJETIVOS pra um loop autônomo de código. Boas práticas:\n"
    "- Objetivo é UMA coisa só e VERIFICÁVEL: descreve o RESULTADO final, não os passos.\n"
    "- Pequeno e concreto (ex: \"migre utils/date.ts pra date-fns mantendo o check verde\"), "
    "nunca amplo (\"refatore o projeto\").\n"
    "- Peça EVIDÊNCIA, não promessa: inclua \"rode o comando de verificação e mostre a saída\".\n"
    "- Deixe explícito: não editar arquivos de teste nem o comando de verificação; corrigir o código.\n"
    "Responda SÓ com o objetivo reescrito em pt-BR, texto puro, sem preâmbulo, sem aspas, sem markdown."
)


class RefineError(Exception):
    """claude -p indisponivel/timeout/exit≠0/vazio no refinador de objetivo."""


def _refine_prompt(goal: str, check_cmd: str | None) -> str:
    parts = [_REFINE_SYSTEM, "", "Objetivo do usuário:", goal]
    if check_cmd:
        parts += ["", f"Comando de verificação (check): {check_cmd}"]
    return "\n".join(parts)


def refine_goal(goal: str, check_cmd: str | None = None) -> str:
    """Refina o objetivo via claude -p efemero (haiku), cwd neutro (nao o da sessao), argv sem shell,
    timeout 60s. Levanta RefineError em qualquer falha (o endpoint mapeia pra 502)."""
    import tempfile
    prompt = _refine_prompt(goal, check_cmd)
    try:
        p = subprocess.run(
            ["claude", "-p", "--model", "haiku", prompt],
            cwd=tempfile.gettempdir(), capture_output=True, text=True, timeout=_REFINE_TIMEOUT,
        )
    except FileNotFoundError:
        raise RefineError("claude CLI não encontrado")
    except (subprocess.TimeoutExpired, OSError):
        raise RefineError("refinamento excedeu o tempo ou falhou ao iniciar")
    if p.returncode != 0:
        raise RefineError(f"claude -p falhou (exit {p.returncode})")
    out = (p.stdout or "").strip()
    if not out:
        raise RefineError("refinamento vazio")
    return out[:_REFINE_MAX]


def suggest_checks(cwd: str) -> list[str]:
    c = Path(cwd)
    out: list[str] = []
    try:
        if (c / "package.json").is_file():
            scripts = json.loads((c / "package.json").read_text(encoding="utf-8")).get("scripts", {})
            for s in ("check", "test", "lint"):
                if s in scripts:
                    out.append(f"npm run {s}")
        if (c / "pyproject.toml").is_file() or (c / "pytest.ini").is_file():
            out.append("uv run pytest -x -q")
        # Comando UNICO: _run_check faz shlex.split sem shell -> '&&' viraria argv literal e o check
        # nunca passaria (a estagnacao mataria o loop com motivo confuso). 'cargo test'/'go test'
        # ja compilam antes de rodar, entao cobrem o build.
        if (c / "Cargo.toml").is_file():
            out.append("cargo test")
        if (c / "go.mod").is_file():
            out.append("go test ./...")
    except (OSError, json.JSONDecodeError, ValueError):
        pass
    return out[:4]


_inflight: set[str] = set()
_inflight_lock = threading.Lock()


def schedule_tick(name: str, ctx_factory: Callable[[], Optional[TickCtx]]) -> None:
    """Dispara run_tick em thread propria com dedupe em memoria (um tick em voo por sessao)."""
    with _inflight_lock:
        if name in _inflight:
            return
        _inflight.add(name)

    def _work():
        try:
            ctx = ctx_factory()
            if ctx is not None:
                run_tick(name, ctx)
        finally:
            with _inflight_lock:
                _inflight.discard(name)

    threading.Thread(target=_work, daemon=True, name=f"loop-tick-{name}").start()
