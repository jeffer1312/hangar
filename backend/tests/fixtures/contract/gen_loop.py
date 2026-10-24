"""Golden do loop runner e do vínculo `then` pelo Python (`app.loop`, `app.chain`, rotas da `app.api`).

Cada caso roda o código de verdade com o relógio fixo em 1000.0 e as dependências de fora (lista,
fila, branch, push, `claude -p`) trocadas por dublês: o tique, o comando de conferência (processo
real, só Linux), o `shlex.split`, as sugestões, os corpos de push, as rotas e o `claude -p` com a
ordem de contas e a troca no limite. O Rust repete os casos em `crates/hangar-server/src/loops/`.

Uso, de backend/: uv run python tests/fixtures/contract/gen_loop.py
"""
import json
import os
import shlex
import subprocess
import sys
import tempfile
from pathlib import Path
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from fastapi.testclient import TestClient  # noqa: E402
from app import account_bridge, api, chain, cotas, loop as loop_mod  # noqa: E402
from app.config import settings  # noqa: E402
from app.models import SessionInfo  # noqa: E402

NOW = 1000.0


class _Clock:
    @staticmethod
    def time():
        return NOW


def _base_loop(**over) -> dict:
    with patch.object(loop_mod, "time", _Clock):
        d = loop_mod.new_loop(over.pop("goal", "g"), over.pop("check_cmd", "false"),
                              over.pop("max_iters", 10), over.pop("require_branch", True))
    d["goal_entry_id"] = "e1"
    d["goal_delivered_ts"] = 1.0
    d["started_ts"] = 1.0
    d.update(over)
    return d


TICKS = [
    ("check_pass_is_done", {}, {"check": [0, "ok"]}),
    ("check_fail_reprompts", {}, {"check": [1, "AssertionError: boom"]}),
    ("anchor_blocks_before_delivery", {"goal_delivered_ts": None}, {"entry_delivered": False}),
    ("anchor_sets_ts_when_delivered", {"goal_delivered_ts": None}, {"entry_delivered": True, "check": [1, "e"]}),
    ("anchor_missing_entry_proceeds", {"goal_delivered_ts": None}, {"entry_delivered": None, "check": [1, "e"]}),
    ("anchor_without_entry_id", {"goal_delivered_ts": None, "goal_entry_id": None}, {"entry_delivered": None, "check": [1, "e"]}),
    ("stagnation", {"iter": 1, "history": [{"n": 1, "ts": 1.0, "check_exit": 1, "tail": "mesmo erro"}]},
     {"check": [1, "mesmo erro"]}),
    ("exhausted", {"max_iters": 1, "iter": 1}, {"check": [1, "e2"]}),
    ("automations_off", {}, {"automations": False}),
    ("branch_main", {}, {"branch": "main"}),
    ("branch_master", {}, {"branch": "master"}),
    ("branch_main_not_required", {"require_branch": False}, {"branch": "main", "check": [1, "e"]}),
    ("branch_none_allowed", {}, {"branch": None, "check": [1, "e"]}),
    ("no_check_done_claimed", {"check_cmd": None}, {"last": "feito. LOOP_DONE"}),
    ("no_check_reprompts", {"check_cmd": None}, {"last": "ainda trabalhando"}),
    ("no_check_no_text", {"check_cmd": None}, {"last": None}),
    ("no_check_exhausted", {"check_cmd": None, "iter": 3, "max_iters": 3}, {"last": "x"}),
    ("empty_check_is_no_check", {"check_cmd": ""}, {"last": "LOOP_DONE"}),
    ("check_not_executable_fails", {}, {"check": [-404, "No closing quotation"]}),
    ("check_timeout_reprompts", {}, {"check": [124, "timeout 600s"]}),
    ("history_capped_20", {"iter": 20, "max_iters": 99,
                           "history": [{"n": i, "ts": float(i), "check_exit": 1, "tail": f"e{i}"} for i in range(1, 21)]},
     {"check": [1, "novo erro"]}),
    ("paused_resumes", {"status": "paused_awaiting"}, {"check": [1, "e"]}),
    ("paused_then_done", {"status": "paused_awaiting"}, {"check": [0, ""]}),
    ("done_is_noop", {"status": "done"}, {"check": [0, ""]}),
    ("done_claimed_is_noop", {"status": "done_claimed"}, {"check": [0, ""]}),
    ("stagnation_only_against_last", {"iter": 2, "history": [{"n": 1, "ts": 1.0, "check_exit": 1, "tail": "x"},
                                                             {"n": 2, "ts": 2.0, "check_exit": 1, "tail": "y"}]},
     {"check": [1, "x"]}),
    ("unicode_goal", {"goal": "corrigir ação — ✓"}, {"check": [2, "falhou ç"]}),
]


def run_tick_case(loop_over: dict, ctx_in: dict) -> dict:
    with tempfile.TemporaryDirectory() as d, \
         patch.object(loop_mod.settings, "projects_dir", Path(d) / "projects"), \
         patch.object(loop_mod, "time", _Clock):
        link = loop_mod.LoopLink("s")
        link.set(_base_loop(**loop_over))
        sent, pushed = [], []
        check = ctx_in.get("check", [1, "err"])
        ctx = loop_mod.TickCtx(
            cwd=d, jsonl=str(Path(d) / "t.jsonl"),
            deliver=lambda p: (sent.append(p), True)[1],
            enqueue=lambda p: sent.append(p),
            notify=lambda n, b: pushed.append([n, b]),
            automations=lambda: ctx_in.get("automations", True),
            branch=lambda _cwd: ctx_in.get("branch", "TICKET-1"),
            last_assistant=lambda _j: ctx_in.get("last"),
            run_check=lambda _c, _w: (check[0], check[1]),
            entry_delivered=lambda _e: ctx_in.get("entry_delivered", True),
        )
        returned = loop_mod.run_tick("s", ctx)
        return {"loop": link.get(), "returned": returned is not None, "pushed": pushed, "sent": sent}


def run_checks() -> list:
    cases = []
    with tempfile.TemporaryDirectory() as d:
        noexec = Path(d) / "noexec.sh"
        noexec.write_text("#!/bin/sh\necho x\n")
        cmds = [
            "true", "false",
            "sh -c 'echo out; echo err >&2; exit 3'",
            "sh -c 'printf \"a\\r\\nb\\rc\"'",
            "sh -c 'head -c 9000 /dev/zero | tr \"\\000\" x; echo; echo fim'",
            "echo \"sem fechar",
            "echo a\\",
            "programa-que-nao-existe-xyz arg",
            "sh -c \"printf '%s|' \\\"a b\\\" c\\\\ d 'e\\\\f'\"",
            "printf '\\377\\376ok'",
            "sh -c 'kill -TERM $$'",
            "./noexec.sh",
            "pwd",
            "sh -c 'printf \"  \\t pad \\n\\n\"'",
            "echo '#nao é comentário' # nem isto",
        ]
        for cmd in cmds:
            try:
                code, tail = loop_mod._run_check(cmd, d)
                cases.append({"cmd": cmd, "exit": code, "tail": tail.replace(d, "<cwd>")})
            except Exception as e:  # noqa: BLE001 - o Python que derruba a thread também é o golden
                cases.append({"cmd": cmd, "raises": type(e).__name__})
        for cmd in ["   ", ""]:
            try:
                cases.append({"cmd": cmd, "exit": loop_mod._run_check(cmd, d)[0]})
            except Exception as e:  # noqa: BLE001
                cases.append({"cmd": cmd, "raises": type(e).__name__})
    return cases


SPLITS = [
    "npm run check", "  a   b  ", "a\tb\nc", "'a b' c", "\"a b\" c", "a\\ b", "\"a\\\"b\"", "'a\\b'",
    "\"a\\b\"", "\"a\\\\b\"", "a\"b c\"d", "''", "\"\"", "a '' b", "C:\\Users\\x\\npm.cmd run", "#x y",
    "a#b", "\"sem fechar", "'sem fechar", "fim\\", "ç ã", "a\\\nb", "\"$HOME\"", "\"a`b\"",
]


def run_splits() -> list:
    out = []
    for s in SPLITS:
        try:
            out.append({"in": s, "argv": shlex.split(s)})
        except ValueError as e:
            out.append({"in": s, "error": str(e)})
    return out


SUGGEST_DIRS = [
    {"package.json": '{"scripts": {"check": "x", "test": "y", "lint": "z", "build": "b"}}'},
    {"package.json": '{"scripts": {"test": "y"}}', "pyproject.toml": "", "Cargo.toml": "", "go.mod": ""},
    {"package.json": '{"scripts": {"check": "x", "test": "y", "lint": "z"}}', "pytest.ini": "", "Cargo.toml": ""},
    {"package.json": "{quebrado", "Cargo.toml": ""},
    {"package.json": '{"name": "sem scripts"}', "go.mod": ""},
    {"package.json": '{"scripts": ["check"]}'},
    {"package.json": "[1]"},
    {},
]


def run_suggestions() -> list:
    out = []
    for files in SUGGEST_DIRS:
        with tempfile.TemporaryDirectory() as d:
            for name, body in files.items():
                (Path(d) / name).write_text(body, encoding="utf-8")
            try:
                got = loop_mod.suggest_checks(d)
            except Exception as e:  # noqa: BLE001
                got = {"raises": type(e).__name__}
            out.append({"files": files, "suggestions": got})
    return out


class _FakeQueue:
    rows: dict = {}

    def __init__(self, name):
        self.name = name

    def append(self, text, delivered=False, ts=None, pre_transcript=False):
        entry = {"id": f"q-{self.name}-{len(_FakeQueue.rows.setdefault(self.name, []))}", "text": text, "delivered": delivered}
        _FakeQueue.rows[self.name].append(entry)
        return entry

    def entry_delivered(self, entry_id):
        return None


def run_routes() -> list:
    """(rótulo, método, caminho, corpo, sessões vivas, automações, branch, sidecar inicial do loop)."""
    infos = {
        "cc": SessionInfo(name="cc", cwd="/repo", jsonl="/repo/t.jsonl", git_cwd="/repo-feat"),
        "cx": SessionInfo(name="cx", cwd="/repo", jsonl="/repo/x.jsonl", provider="codex"),
        "b": SessionInfo(name="b", cwd="/b", jsonl="/b/t.jsonl"),
    }
    running = _base_loop(check_cmd=None)
    claimed = {**_base_loop(check_cmd=None), "status": "done_claimed"}
    claimed_last = {**claimed, "iter": 10}
    stopped = {**_base_loop(), "status": "stopped", "ended_reason": "x", "ended_ts": 5.0}
    cases = [
        ("then_ok", "PUT", "/api/sessions/cc/then", {"target": "b", "text": "roda os testes"}, None),
        ("then_self", "PUT", "/api/sessions/cc/then", {"target": "cc", "text": "x"}, None),
        ("then_missing_target", "PUT", "/api/sessions/cc/then", {"target": "ghost", "text": "x"}, None),
        ("then_empty_text", "PUT", "/api/sessions/cc/then", {"target": "b", "text": ""}, None),
        ("then_extra_field", "PUT", "/api/sessions/cc/then", {"target": "b", "text": "x", "y": 1}, None),
        ("then_delete", "DELETE", "/api/sessions/cc/then", None, None),
        ("create_ok", "POST", "/api/sessions/cc/loop", {"goal": "criar ok.txt", "check_cmd": "test -f ok.txt"}, None),
        ("create_defaults", "POST", "/api/sessions/cc/loop", {"goal": "g"}, None),
        ("create_missing", "POST", "/api/sessions/ghost/loop", {"goal": "g"}, None),
        ("create_codex", "POST", "/api/sessions/cx/loop", {"goal": "g"}, None),
        ("create_automations_off", "POST", "/api/sessions/cc/loop", {"goal": "g"}, None, {"automations": False}),
        ("create_already_active", "POST", "/api/sessions/cc/loop", {"goal": "g"}, running),
        ("create_after_stopped", "POST", "/api/sessions/cc/loop", {"goal": "g", "max_iters": 3}, stopped),
        ("create_main_branch", "POST", "/api/sessions/cc/loop", {"goal": "g"}, None, {"branch": "main"}),
        ("create_main_not_required", "POST", "/api/sessions/cc/loop", {"goal": "g", "require_branch": False}, None, {"branch": "main"}),
        ("create_max_too_high", "POST", "/api/sessions/cc/loop", {"goal": "g", "max_iters": 500}, None),
        ("create_max_zero", "POST", "/api/sessions/cc/loop", {"goal": "g", "max_iters": 0}, None),
        ("create_empty_goal", "POST", "/api/sessions/cc/loop", {"goal": ""}, None),
        ("create_null_check", "POST", "/api/sessions/cc/loop", {"goal": "g", "check_cmd": None}, None),
        ("get_none", "GET", "/api/sessions/cc/loop", None, None),
        ("get_running", "GET", "/api/sessions/cc/loop", None, running),
        ("get_missing_session", "GET", "/api/sessions/ghost/loop", None, None),
        ("stop_missing", "DELETE", "/api/sessions/cc/loop", None, None),
        ("stop_running", "DELETE", "/api/sessions/cc/loop", None, running),
        ("stop_already_stopped", "DELETE", "/api/sessions/cc/loop", None, stopped),
        ("resolve_wrong_state", "POST", "/api/sessions/cc/loop/resolve", {"accept": True}, running),
        ("resolve_no_loop", "POST", "/api/sessions/cc/loop/resolve", {"accept": True}, None),
        ("resolve_accept", "POST", "/api/sessions/cc/loop/resolve", {"accept": True}, claimed),
        ("resolve_reject", "POST", "/api/sessions/cc/loop/resolve", {"accept": False}, claimed),
        ("resolve_reject_exhausted", "POST", "/api/sessions/cc/loop/resolve", {"accept": False}, claimed_last),
        ("resolve_reject_dead_session", "POST", "/api/sessions/ghost/loop/resolve", {"accept": False}, claimed),
        ("refine_automations_off", "POST", "/api/sessions/cc/loop/refine", {"goal": "g"}, None, {"automations": False}),
        ("refine_too_long", "POST", "/api/sessions/cc/loop/refine", {"goal": "x" * 2001}, None),
    ]
    out = []
    for case in cases:
        label, method, path, body, initial = case[:5]
        opts = case[5] if len(case) > 5 else {}
        sess = path.split("/")[3]
        with tempfile.TemporaryDirectory() as d, \
             patch.object(account_bridge, "owner_mode", lambda: "python"), \
             patch.object(settings, "projects_dir", Path(d) / "projects"), \
             patch.object(settings, "auth_token", "secret"), \
             patch.object(loop_mod, "time", _Clock), \
             patch.object(api.registry, "list", lambda: list(infos.values())), \
             patch.object(api, "_session_exists", lambda n: n in infos), \
             patch.object(api, "automations_enabled", lambda: opts.get("automations", True)), \
             patch.object(api, "branch_of", lambda _d: opts.get("branch", "TICKET-1")), \
             patch.object(api, "PromptQueue", _FakeQueue), \
             patch.object(api, "_drenar", lambda *a, **k: 0), \
             patch.object(api.push, "notify_loop", lambda n, b: pushed.append([n, b])):
            _FakeQueue.rows = {}
            pushed: list = []
            if initial is not None:
                loop_mod.LoopLink(sess).set(json.loads(json.dumps(initial)))
            r = TestClient(api.app).request(method, path, json=body, headers={"Authorization": "Bearer secret"})
            try:
                resp = r.json()
            except ValueError:
                resp = r.text
            out.append({"label": label, "method": method, "path": path, "body": body, "initial": initial,
                        "automations": opts.get("automations", True), "branch": opts.get("branch", "TICKET-1"),
                        "status": r.status_code, "response": resp, "pushed": pushed, "queued": _FakeQueue.rows,
                        "loop_after": loop_mod.LoopLink(sess).get(), "chain_after": chain.ThenLink(sess).get()})
    return out


def _row(path, ativa=False, estado="lida", janelas=(("5h", 10.0, False), ("7d", 20.0, False))):
    return {"id": f"claude:{path}", "label": path, "provedor": "claude", "ativa": ativa, "estado": estado,
            "janelas": [{"rotulo": r, "pct": p, "por_modelo": m} for r, p, m in janelas], "ts": NOW}


ORDERS = [
    [_row("/a", ativa=True), _row("/b", janelas=(("7d", 50.0, False),)), _row("/c", janelas=(("7d", 5.0, False),))],
    [_row("/a", ativa=True, janelas=(("5h", 100.0, False),)), _row("/b")],
    [_row("/a", ativa=True, janelas=(("7d", 100.0, True),)), _row("/b")],
    [_row("/a", ativa=True, janelas=(("5h", 100.0, False),))],
    [_row("/b", estado="expirada"), _row("/c", janelas=()), _row("/d", janelas=(("7d", 100.0, False),))],
    [_row("/b", janelas=(("5h", 3.0, False),)), _row("/c", janelas=(("7d", 1.0, False),))],
    [],
]

LIMITS = [
    (1, "You've hit your weekly limit · resets 3pm", ""), (1, "", "Rate limit exceeded"), (1, "erro qualquer", ""),
    (0, "You've hit your weekly limit · resets 3pm", ""), (0, "  usage limit reached", ""),
    (0, "Claude AI usage limit reached|1700000000", ""), (0, "A resposta cita usage limit no meio", ""),
    (0, "You've hit your limit" + "x" * 300, ""), (2, "HIT YOUR monthly LIMIT", ""), (1, "hit your\nlimit", ""),
]

# Respostas do `claude -p` por conta, na ordem em que são chamadas.
FLOWS = [
    ("ok_default", ORDERS[0], [(0, "  objetivo reescrito \n", "")], None),
    ("limit_switches_once", ORDERS[0], [(1, "You've hit your weekly limit", ""), (0, "outro", "")], ORDERS[0]),
    ("limit_second_fails", ORDERS[0], [(1, "You've hit your weekly limit", ""), (1, "", "boom")], ORDERS[0]),
    ("limit_second_limited", ORDERS[0], [(1, "usage limit", ""), (0, "usage limit reached", "")], ORDERS[0]),
    ("limit_no_other", [_row("/a", ativa=True)], [(1, "rate limit", "")], [_row("/a", ativa=True)]),
    ("exit_nonzero", ORDERS[0], [(3, "x", "falhou feio")], None),
    ("empty_output", ORDERS[0], [(0, "   ", "")], None),
    ("default_exhausted_starts_on_other", ORDERS[1], [(0, "ok", "")], None),
    ("refused_question", ORDERS[0], [(0, "Qual arquivo?", "")], None),
    ("refused_prefix", ORDERS[0], [(0, "Desculpe, não dá", "")], None),
    ("truncated", ORDERS[0], [(0, "y" * 2500, "")], None),
]


def run_flows() -> list:
    out = []
    for label, lidas, answers, relidas in FLOWS:
        calls = []

        def fake_run(exe, prompt, conta, _answers=iter(answers)):
            code, so, se = next(_answers)
            calls.append(conta)
            return subprocess.CompletedProcess([exe], code, so, se)

        def fake_cotas(atualizar=False, _l=lidas, _r=relidas):
            rows = _r if atualizar else _l
            return [cotas.CotaConta.model_validate(r) for r in rows]

        with patch.object(loop_mod, "_exe_claude", lambda: "/bin/claude"), \
             patch.object(loop_mod, "_rodar_claude_p", fake_run), \
             patch.object(cotas, "cotas_claude", fake_cotas):
            try:
                res = {"goal": loop_mod.refine_goal("g", "make test")}
            except loop_mod.ClaudePError as e:
                res = {"error": str(e)}
        out.append({"label": label, "lidas": lidas, "relidas": relidas, "answers": answers, "calls": calls, **res})
    return out


def run_push_bodies() -> list:
    """O corpo do `_end` pelo `push.notify_loop`, em cada idioma da inscrição."""
    from app import push
    out = []
    for status, reason in [("done", "check passou"), ("done_claimed", "declarou pronto — confirmar?"),
                           ("stopped", "branch main"), ("exhausted", "esgotou 3 iterações"),
                           ("failed", "check_cmd não executável: x: y")]:
        got = {}
        with patch.object(push, "_suppressed", lambda _n: False), \
             patch.object(push, "_broadcast", lambda _n, fn, **_k: got.update(pt=fn("pt"), en=fn("en"))):
            push.notify_loop("s", loop_mod._body(status, reason))
        out.append({"status": status, "reason": reason, **got})
    return out


def main() -> None:
    ticks = [{"label": label, "loop": _base_loop(**over), "ctx": ctx, "out": run_tick_case(over, ctx)}
             for label, over, ctx in TICKS]
    orders = [{"rows": rows, "order": loop_mod._ordem_contas([cotas.CotaConta.model_validate(r) for r in rows])}
              for rows in ORDERS]
    limits = [{"exit": c, "stdout": so, "stderr": se,
               "limited": loop_mod._bateu_limite(subprocess.CompletedProcess(["x"], c, so, se))} for c, so, se in LIMITS]
    bodies = [{"status": s, "reason": r, "body": loop_mod._body(s, r)}
              for s in ("done", "done_claimed", "stopped", "exhausted", "failed", "running", "outro")
              for r in ("check passou", None)]
    commit = subprocess.run(["git", "rev-parse", "HEAD"], capture_output=True, text=True, cwd=HERE).stdout.strip()
    out = {"origin_commit": commit, "command": "uv run python tests/fixtures/contract/gen_loop.py", "now": NOW,
           "prompt": loop_mod._refine_prompt("g", "make test"),
           "ticks": ticks, "checks": run_checks(), "splits": run_splits(), "suggestions": run_suggestions(),
           "bodies": bodies, "push_bodies": run_push_bodies(), "routes": run_routes(), "orders": orders, "limits": limits, "flows": run_flows()}
    dest = HERE / "loop" / "cases.json"
    dest.parent.mkdir(exist_ok=True)
    dest.write_text(json.dumps(out, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
    print(f"{len(ticks)} tiques, {len(out['routes'])} rotas, {len(out['checks'])} comandos -> {dest}")


if __name__ == "__main__":
    os.environ.setdefault("LC_ALL", "C.UTF-8")
    main()
