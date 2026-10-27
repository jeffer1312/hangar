"""orq advance (skills/orquestrar/scripts/orq.py): the orchestrator's pass, on a real git repo
with a fake hangar-send."""
import fcntl
import http.server
import importlib.util
import json
import os
import subprocess
import sys
import threading
import time
from datetime import datetime
from pathlib import Path

import pytest

pytestmark = pytest.mark.usefixtures("hangar_server_bin")

ORQ = Path(__file__).resolve().parents[2] / "skills" / "orquestrar" / "scripts" / "orq.py"

# One line per hangar-send call ("$*"); `--new` also writes the sidecar the backend would, with
# the model asked for, or FAKE_BORN_MODEL to simulate a session born on another one.
FAKE = r"""#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
a = sys.argv[1:]
with open(os.environ["FAKE_LOG"], "a") as f:
    f.write(" ".join(a) + "\n")
if a[:1] == [os.environ.get("FAKE_FAIL")]:   # a send to this session fails
    sys.exit(1)
if os.environ.get("HANGAR_SEND_PAINEL") == "1":
    with open(os.environ["FAKE_LOG"] + ".painel", "a") as f:
        f.write(a[0] + "\n")
if a[:1] == ["--new"]:
    opt = lambda k, dflt=None: a[a.index(k) + 1] if k in a else dflt
    side = Path.home() / ".hangar" / "claude-headless"
    side.mkdir(parents=True, exist_ok=True)
    (side / (a[1] + ".json")).write_text(json.dumps({
        "name": a[1], "provider": opt("--provider", "claude"),
        "key": "fake-" + a[1],
        "model": os.environ.get("FAKE_BORN_MODEL") or opt("--model")}))
"""

ROWS = ("| 1 | first | §1 | `a.txt` · `messages/pt.json` | `true` | 1 | low | — |\n"
        "| 2 | second | §2 | `a.txt` · `messages/pt.json` | `true` | 1 | high | — |\n")

PLAN = """# Orchestration plan — t

## Projeto
Checagens: —
Integração: {integ}
Prova: {prova}
Paralelo: {par}
Revisão: {revisao}
Correção pelo revisor: até 0 linhas
Aditivos: `messages/*.json`

## Tasks
| # | What it is | Where in their plan | Files | Verification | Wave | Risk | Roteiro |
|---|---|---|---|---|---|---|---|
{rows}"""


def run(e, *args, check=True):
    r = subprocess.run([sys.executable, str(ORQ), *args], env=e, capture_output=True, text=True)
    if check:
        assert r.returncode == 0, r.stdout + r.stderr
    return r


def git_in(r):
    def g(*a):
        return subprocess.run(["git", "-C", str(r), *a], check=True, capture_output=True,
                              text=True).stdout.strip()
    return g


def start(tmp_path, integ="`test -f a.txt`", par="até 2", rows=ROWS, revisao="sessão",
          prova="por-task", contract=""):
    """Repo on `main`, an auto run in <date>-g1 with the stamped plan, execucao_inicio recorded."""
    r = tmp_path / "repo"
    r.mkdir()
    g = git_in(r)
    g("init", "-q", "-b", "main")
    g("config", "user.email", "t@t")
    g("config", "user.name", "t")
    (r / "a.txt").write_text("1\n2\n3\n")
    (r / "messages").mkdir()
    (r / "messages" / "pt.json").write_text('{\n  "a": "A",\n  "z": "Z"\n}\n')
    g("add", "a.txt", "messages/pt.json")
    g("commit", "-qm", "base")
    d = tmp_path / "2026-09-28-g1"
    d.mkdir()
    log = tmp_path / "sent.log"
    fake = tmp_path / "fake-send"
    fake.write_text(FAKE)
    fake.chmod(0o755)
    # CLAUDE_CONFIG_DIR isolated: jev_config() would otherwise read the real runtime-config.json.
    nt = tmp_path / "nt-bin"   # tmux falso: o fallback de _session_ids nunca ve as sessoes reais
    nt.mkdir()
    (nt / "tmux").write_text("#!/bin/sh\nexit 1\n")
    (nt / "tmux").chmod(0o755)
    e = {**os.environ, "PATH": f"{nt}{os.pathsep}{os.environ['PATH']}",
         "ORQ_DIR": str(d), "ORQ_SEND": str(fake), "FAKE_LOG": str(log),
         "ORQ_JEV": "off", "HOME": str(tmp_path), "CLAUDE_CONFIG_DIR": str(tmp_path / "cfg"),
         "ORQ_WHOAMI": "false",
         "TYPESAFE_API_KEY": "", "ORQ_JEV_URL": "", "JEV_ENDPOINT": "", "JEV_MODEL": ""}
    (tmp_path / "r1.md").write_text("roteiro\n")
    plan = tmp_path / "plan.orq.md"
    plan.write_text(PLAN.format(integ=integ, par=par, revisao=revisao, prova=prova, rows=rows))
    run(e, "plan-check", str(plan), "--repo", str(r), "--stamp")
    c = tmp_path / "regras.md"
    c.write_text(contract)
    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", str(c), "--plan", str(plan),
        "--auto")
    run(e, "event", "execucao_inicio", "--plano", str(plan), "--branch", "main", "--gid", "g1")
    return d, r, g, e, log


def started(e, tasks=(1, 2)):
    """The Tasks already opened (by hand here), so advance only integrates."""
    for n in tasks:
        run(e, "event", "task_inicio", "--task", str(n), "--titulo", f"t{n}", "--executor", f"ex{n}",
            "--par", f"rev{n}")


def close(d, task, h):
    """What `orq commit` writes once the commit is checked."""
    ts = datetime.now().astimezone().isoformat(timespec="seconds")
    with (d / "closed.jsonl").open("a") as f:
        f.write(json.dumps({"ts": ts, "task": task, "hash": h}) + "\n")


def test_done_includes_closed_task_reviewer_before_execution_end(tmp_path):
    d, _, _, e, _ = start(tmp_path)
    started(e, tasks=(1,))
    close(d, 1, "a")

    assert dict(orq_mod().done(d)) == {
        "ex1": "Task 1 closed", "rev1": "Task 1 closed",
    }


def test_done_preserves_reviewer_shared_with_open_task(tmp_path):
    d, _, _, e, _ = start(tmp_path)
    started(e, tasks=(1,))
    run(e, "event", "task_inicio", "--task", "2", "--titulo", "t2",
        "--executor", "ex2", "--par", "rev1")
    close(d, 1, "a")

    assert dict(orq_mod().done(d)) == {"ex1": "Task 1 closed"}


def test_done_preserves_reopened_task_owners(tmp_path):
    d, _, _, e, _ = start(tmp_path)
    started(e, tasks=(1,))
    close(d, 1, "a")
    with (d / "eventos.jsonl").open("a") as f:
        f.write(json.dumps({"ts": "2999-01-01T00:00:00+00:00", "tipo": "task_inicio",
                            "task": 1, "executor": "ex1", "par": "rev1"}) + "\n")

    assert orq_mod().done(d) == []


def test_done_preserves_arbiter_and_subagent_after_close(tmp_path):
    d, _, _, e, _ = start(tmp_path)
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t1",
        "--executor", "arb", "--par", "subagente")
    close(d, 1, "a")

    assert orq_mod().done(d) == []


def test_done_keeps_replaced_reviewers_in_cleanup(tmp_path):
    d, _, _, e, _ = start(tmp_path)
    started(e, tasks=(1,))
    run(e, "event", "sessao_trocada", "--de", "rev1", "--para", "rev1b")
    close(d, 1, "a")

    assert dict(orq_mod().done(d)) == {
        "ex1": "Task 1 closed", "rev1b": "Task 1 closed", "rev1": "replaced by rev1b",
    }


def test_task_start_records_both_session_identities(tmp_path, monkeypatch):
    d, _, _, _, _ = start(tmp_path)
    m = orq_mod()
    monkeypatch.setattr(m, "session_identity", lambda name: f"claude:key-{name}")

    ev = m.event_append(d, {"tipo": "task_inicio", "task": 1, "titulo": "t1",
                            "executor": "ex1", "par": "rev1"})

    assert ev["session_identities"] == {"ex1": "claude:key-ex1", "rev1": "claude:key-rev1"}
    assert events(d)[-1]["session_identities"] == ev["session_identities"]


def test_session_swap_records_destination_identity(tmp_path, monkeypatch):
    d, _, _, _, _ = start(tmp_path)
    m = orq_mod()
    monkeypatch.setattr(m, "session_identity", lambda name: f"claude:key-{name}")

    ev = m.event_append(d, {"tipo": "sessao_trocada", "de": "arb", "para": "arb2"})

    assert ev["session_identity"] == "claude:key-arb2"


def test_identity_failure_records_problem_without_using_name(tmp_path, monkeypatch):
    d, _, _, _, _ = start(tmp_path)
    m = orq_mod()
    def unavailable(name):
        raise ValueError("missing")
    monkeypatch.setattr(m, "session_identity", unavailable)

    ev = m.event_append(d, {"tipo": "task_inicio", "task": 1, "titulo": "t1",
                            "executor": "ex1", "par": "subagente"})

    assert ev["session_identities"] == {}
    assert "session identity unavailable: ex1: ValueError" in (d / "registro.md").read_text()
    assert "session identity unavailable: subagente" not in (d / "registro.md").read_text()


@pytest.mark.parametrize("swap", [False, True])
def test_reinit_preserves_original_arbiter_identity(tmp_path, swap):
    sidecars = tmp_path / ".hangar" / "claude-headless"
    sidecars.mkdir(parents=True)
    (sidecars / "arb.json").write_text(json.dumps({"key": "original"}))
    (sidecars / "arb2.json").write_text(json.dumps({"key": "successor"}))
    d, r, _, e, _ = start(tmp_path)
    cfg = json.loads((d / "orq.json").read_text())
    assert cfg["arbiter_identity"] == "claude:original"
    if swap:
        run(e, "event", "sessao_trocada", "--de", "arb", "--para", "arb2")
    (sidecars / "arb.json").write_text(json.dumps({"key": "unrelated-new-session"}))

    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", cfg["contract"],
        "--plan", cfg["plan"], "--auto")

    assert json.loads((d / "orq.json").read_text())["arbiter_identity"] == "claude:original"


def test_reinit_does_not_bind_legacy_run_to_recreated_arbiter(tmp_path):
    d, r, _, e, _ = start(tmp_path)
    cfg = json.loads((d / "orq.json").read_text())
    cfg.pop("arbiter_identity", None)
    (d / "orq.json").write_text(json.dumps(cfg))
    sidecars = tmp_path / ".hangar" / "claude-headless"
    sidecars.mkdir(parents=True)
    (sidecars / "arb.json").write_text(json.dumps({"key": "unrelated-new-session"}))

    run(e, "init", "--arbiter", "arb", "--repo", str(r), "--contract", cfg["contract"],
        "--plan", cfg["plan"], "--auto")

    assert "arbiter_identity" not in json.loads((d / "orq.json").read_text())
    assert "previous init has no recorded identity" in (d / "registro.md").read_text()


def branch_commit(g, r, name, path, content):
    """A worktree Task's commit: its own branch off main, main checked out again."""
    g("checkout", "-q", "-b", name, "main")
    (r / path).write_text(content)
    g("add", path)
    g("commit", "-qm", name)
    h = g("rev-parse", "HEAD")
    g("checkout", "-q", "main")
    return h


def events(d):
    return [json.loads(l) for l in (d / "eventos.jsonl").read_text().splitlines() if l.strip()]


def timeline_lines(d):
    p = d / f"timeline-{d.name}.jsonl"
    return [json.loads(l) for l in p.read_text().splitlines()] if p.exists() else []


def sent(log):
    return log.read_text().splitlines() if log.exists() else []


def orq_mod():
    spec = importlib.util.spec_from_file_location("orq_advance_mod", ORQ)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


@pytest.fixture
def jev():
    """Fake Jev: answers what the test put in `resp`, keeps the request body."""
    ctl = {"resp": {}, "body": None}

    class H(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            ctl["body"] = json.loads(self.rfile.read(int(self.headers["content-length"])))
            out = json.dumps({"answers": ctl["resp"]}).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.end_headers()
            self.wfile.write(out)

        def log_message(self, *a):
            pass

    srv = http.server.HTTPServer(("127.0.0.1", 0), H)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    ctl["url"] = f"http://127.0.0.1:{srv.server_port}/v1/systemone"
    yield ctl
    srv.shutdown()


def test_advance_does_nothing_in_a_run_without_auto(tmp_path):
    d, r, g, e, log = start(tmp_path)
    cfg = json.loads((d / "orq.json").read_text())
    cfg.pop("auto")
    (d / "orq.json").write_text(json.dumps(cfg))
    started(e)
    close(d, 1, g("rev-parse", "HEAD"))
    assert run(e, "advance").stdout == ""
    assert not any(x["tipo"] == "integrada" for x in events(d))
    assert not (d / "advance.lock").exists() and not (d / "advance.again").exists()


def test_worktree_task_is_merged_no_ff_then_integrated_once(tmp_path):
    d, r, g, e, log = start(tmp_path, integ="`test -f b.txt`")
    started(e)
    h = branch_commit(g, r, "main-t1", "b.txt", "b\n")
    close(d, 1, h)
    out = run(e, "advance").stdout.splitlines()
    head = g("rev-parse", "HEAD")
    assert out[0] == f"integrated T1 {head[:12]}"
    # A --no-ff merge of exactly the verified commit; the check ran on the merged tree.
    assert g("rev-list", "--parents", "-n1", "HEAD").split()[2] == h
    integ = [x for x in events(d) if x["tipo"] == "integrada"]
    assert [(x["task"], x["commit"]) for x in integ] == [(1, head)]
    assert any(x["kind"] == "advance" and x["task"] == 1 and "merge" in x["text"]
               for x in timeline_lines(d))
    run(e, "advance")
    assert g("rev-parse", "HEAD") == head
    assert len([x for x in events(d) if x["tipo"] == "integrada"]) == 1


def test_commit_already_on_the_main_line_is_not_merged(tmp_path):
    # Sequential allows one Task per wave.
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ROWS.replace("| 1 | high |", "| 2 | high |"))
    started(e)
    (r / "a.txt").write_text("x\n")
    g("commit", "-qam", "t1")
    h = g("rev-parse", "HEAD")
    close(d, 1, h)
    run(e, "advance")
    assert g("rev-parse", "HEAD") == h
    assert [(x["task"], x["commit"]) for x in events(d) if x["tipo"] == "integrada"] == [(1, h)]


def test_red_integration_without_jev_goes_back_to_the_executor_and_never_retries(tmp_path):
    counter = tmp_path / "ran"
    d, r, g, e, log = start(tmp_path, integ=f"`echo x >> {counter}; exit 1`")
    started(e)
    close(d, 1, branch_commit(g, r, "main-t1", "b.txt", "b\n"))
    assert "red T1: back to its executor" in run(e, "advance").stdout
    assert counter.read_text() == "x\n"
    tipos = [x["tipo"] for x in events(d)]
    assert tipos[-2:] == ["integracao_vermelha", "task_inicio"]
    assert "ex1" in run(e, "ball").stdout.split()   # the Task is open again, the ball with its executor
    assert any(m.startswith("ex1 [painel: orquestrador g1] [decisao] Integration red after Task 1")
               for m in sent(log))
    before = sent(log)
    run(e, "advance")   # the main line waits for the fix: no rerun, nobody woken again
    assert counter.read_text() == "x\n" and sent(log) == before


@pytest.mark.parametrize("mode,green", [("on", True), ("shadow", False)])
def test_red_integration_jev_retry_applies_only_when_on(tmp_path, jev, mode, green):
    flaky = tmp_path / "flaky"
    d, r, g, e, log = start(tmp_path, integ=f"`mkdir {flaky} 2>/dev/null && exit 1 || true`")
    cfg = json.loads((d / "orq.json").read_text())
    cfg["jev"] = mode
    (d / "orq.json").write_text(json.dumps(cfg))
    jev["resp"] = {"red": {"choice": "retry", "probabilities": {"retry": 0.95}}}
    e = {**e, "TYPESAFE_API_KEY": "k", "ORQ_JEV_URL": jev["url"]}
    started(e)
    close(d, 1, branch_commit(g, r, "main-t1", "b.txt", "b\n"))
    run(e, "advance")
    assert set(jev["body"]["questions"]["red"]["criteria"]) >= {"back", "retry", "wake"}
    tipos = [x["tipo"] for x in events(d)]
    assert ("integrada" in tipos) is green
    assert ("integracao_vermelha" in tipos) is not green
    if mode == "shadow":
        assert any(x["kind"] == "notice" and "rodar de novo" in x["text"] for x in timeline_lines(d))


def test_positional_conflict_in_an_additive_file_is_resolved_by_union(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    h1 = branch_commit(g, r, "main-t1", "messages/pt.json", '{\n  "a": "A",\n  "b": "B",\n  "z": "Z"\n}\n')
    h2 = branch_commit(g, r, "main-t2", "messages/pt.json", '{\n  "a": "A",\n  "c": "C",\n  "z": "Z"\n}\n')
    close(d, 1, h1)
    close(d, 2, h2)
    out = run(e, "advance").stdout
    assert "integrated T1" in out and "integrated T2" in out
    assert json.loads((r / "messages" / "pt.json").read_text()) == {"a": "A", "b": "B", "c": "C", "z": "Z"}
    assert g("status", "--porcelain") == ""


def test_other_conflict_aborts_the_merge_and_wakes_the_arbiter_once(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    close(d, 1, branch_commit(g, r, "main-t1", "a.txt", "one\n2\n3\n"))
    close(d, 2, branch_commit(g, r, "main-t2", "a.txt", "two\n2\n3\n"))
    run(e, "advance")
    head = g("rev-parse", "HEAD")
    assert not (r / ".git" / "MERGE_HEAD").exists()
    assert (r / "a.txt").read_text() == "one\n2\n3\n"
    woke = [m for m in sent(log) if "conflicted" in m]
    assert woke == ["arb [painel: orquestrador g1] [decisao] T2 conflicted with T1 in a.txt. "
                    "Merge aborted: the main line is as before."]
    assert [x["motivo"] for x in events(d) if x["tipo"] == "conflito"] == ["a.txt with T1"]
    run(e, "advance")
    assert g("rev-parse", "HEAD") == head
    assert [m for m in sent(log) if "conflicted" in m] == woke


def test_a_failed_step_wakes_the_arbiter_once_and_is_not_retried(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    close(d, 1, "deadbeef" * 5)   # not a commit: git merge refuses without a conflict
    assert run(e, "advance").stdout.startswith("failed integrate T1: git merge")
    run(e, "advance")
    fails = [x for x in events(d) if x["tipo"] == "advance_falhou"]
    assert [(x["passo"], x["task"]) for x in fails] == [("integrate", 1)]
    assert len([m for m in sent(log) if "orq advance failed at integrate T1" in m]) == 1
    assert [x["kind"] for x in timeline_lines(d) if x["task"] == 1] == ["failed"]


def test_a_held_lock_leaves_a_mark_and_does_nothing(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    close(d, 1, g("rev-parse", "HEAD"))
    with (d / "advance.lock").open("w") as f:
        fcntl.flock(f, fcntl.LOCK_EX | fcntl.LOCK_NB)
        assert run(e, "advance").stdout == ""
    assert not any(x["tipo"] == "integrada" for x in events(d))
    assert (d / "advance.again").exists()   # the holder would have run once more for it
    assert "integrated T1" in run(e, "advance").stdout


def test_union_proof_refuses_a_changed_value_and_a_lost_line():
    m = orq_mod()
    assert m._prove_additive("m.json", b"", b'{"a": "1"}', b'{"a": "2"}', b'{"a": "1", "a": "2"}')
    assert m._prove_additive("m.txt", b"x\n", b"x\ny\n", b"x\nz\n", b"x\ny\n")
    assert m._prove_additive("m.json", b"{}", b'{"a": "1"}', b'{"b": "2"}', b'{"a": "1", "b": "2"}') is None
    assert m._prove_additive("m.txt", b"a\nb\n", b"a\nX\nb\n", b"a\nY\nb\n", b"a\nX\nY\nb\n") is None


def test_union_proof_refuses_an_old_line_kept_beside_its_edit():
    # ours edited X into X2, theirs inserted Y after X: the union keeps X, which ours removed.
    m = orq_mod()
    assert m._prove_additive("m.txt", b"a\nX\nb\n", b"a\nX2\nb\n", b"a\nX\nY\nb\n",
                             b"a\nX2\nX\nY\nb\n")
    # A key one side deleted does not come back through the other side's copy.
    assert m._prove_additive("m.json", b'{"a": 1, "z": 1}', b'{"a": 1, "b": 1}',
                             b'{"a": 1, "c": 1, "z": 1}', b'{"a": 1, "b": 1, "c": 1, "z": 1}')


def test_an_edit_beside_an_insertion_in_an_additive_file_is_a_conflict(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    base = '{\n  "a": "A",\n  "z": "Z"\n}\n'
    h1 = branch_commit(g, r, "main-t1", "messages/pt.json", base.replace('"A"', '"A2"'))
    h2 = branch_commit(g, r, "main-t2", "messages/pt.json", base.replace('"A",', '"A",\n  "c": "C",'))
    close(d, 1, h1)
    close(d, 2, h2)
    run(e, "advance")
    assert [x["task"] for x in events(d) if x["tipo"] == "conflito"] == [2]
    assert json.loads((r / "messages" / "pt.json").read_text()) == {"a": "A2", "z": "Z"}
    assert g("status", "--porcelain") == ""


def hook(g, r, name):
    """A repo hook that refuses; the path is explicit so a global core.hooksPath cannot hide it."""
    hooks = r / ".git" / "hooks"
    hooks.mkdir(exist_ok=True)
    (hooks / name).write_text("#!/bin/sh\nexit 1\n")
    (hooks / name).chmod(0o755)
    g("config", "core.hooksPath", str(hooks))


def test_a_merge_refused_by_a_hook_is_aborted(tmp_path):
    d, r, g, e, log = start(tmp_path, integ="`test -f b.txt`")
    started(e)
    close(d, 1, branch_commit(g, r, "main-t1", "b.txt", "b\n"))
    before = g("rev-parse", "HEAD")
    hook(g, r, "commit-msg")
    assert run(e, "advance").stdout.startswith("failed integrate T1")
    assert not (r / ".git" / "MERGE_HEAD").exists()
    assert g("status", "--porcelain") == "" and g("rev-parse", "HEAD") == before


def test_a_union_commit_refused_by_pre_commit_is_aborted(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    h1 = branch_commit(g, r, "main-t1", "messages/pt.json", '{\n  "a": "A",\n  "b": "B",\n  "z": "Z"\n}\n')
    h2 = branch_commit(g, r, "main-t2", "messages/pt.json", '{\n  "a": "A",\n  "c": "C",\n  "z": "Z"\n}\n')
    close(d, 1, h1)
    close(d, 2, h2)
    hook(g, r, "pre-commit")   # git merge runs no pre-commit: only the union's commit meets it
    out = run(e, "advance").stdout
    assert "integrated T1" in out and "failed integrate T2" in out
    assert not (r / ".git" / "MERGE_HEAD").exists()
    assert g("status", "--porcelain") == ""


@pytest.mark.parametrize("staged", [False, True])
def test_a_dirty_main_line_waits_without_event_or_wake(tmp_path, staged):
    counter = tmp_path / "ran"
    d, r, g, e, log = start(tmp_path, integ=f"`echo x >> {counter}`")
    started(e)
    close(d, 1, branch_commit(g, r, "main-t1", "b.txt", "b\n"))
    before, n_events = g("rev-parse", "HEAD"), len(events(d))
    (r / "messages" / "pt.json").write_text("{}\n")   # a file the Task never touched
    if staged:
        g("add", "messages/pt.json")
    run(e, "advance")
    run(e, "advance")
    assert g("rev-parse", "HEAD") == before and not counter.exists()
    assert len(events(d)) == n_events and sent(log) == []
    waits = [x for x in timeline_lines(d) if x["kind"] == "notice"]
    assert len(waits) == 1 and "árvore limpa" in waits[0]["text"]   # once per dirty episode
    g("checkout", "HEAD", "--", "messages/pt.json")
    assert "integrated T1" in run(e, "advance").stdout


def test_a_modify_delete_conflict_in_an_additive_file_is_not_a_union(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    h1 = branch_commit(g, r, "main-t1", "messages/pt.json", '{\n  "a": "A",\n  "b": "B",\n  "z": "Z"\n}\n')
    g("checkout", "-q", "-b", "main-t2", "main")
    g("rm", "-q", "messages/pt.json")
    g("commit", "-qm", "t2")
    h2 = g("rev-parse", "HEAD")
    g("checkout", "-q", "main")
    close(d, 1, h1)
    close(d, 2, h2)
    run(e, "advance")
    assert [x["task"] for x in events(d) if x["tipo"] == "conflito"] == [2]
    assert (r / "messages" / "pt.json").exists() and g("status", "--porcelain") == ""


def test_a_conflict_whose_wake_fails_is_not_a_failed_step(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    close(d, 1, branch_commit(g, r, "main-t1", "a.txt", "one\n2\n3\n"))
    close(d, 2, branch_commit(g, r, "main-t2", "a.txt", "two\n2\n3\n"))
    e = {**e, "FAKE_LOG": str(tmp_path / "missing" / "sent.log")}   # the fake send exits non-zero
    run(e, "advance")
    tipos = [x["tipo"] for x in events(d)]
    assert "conflito" in tipos and "advance_falhou" not in tipos


CONTRACT = """# Regras

## Quem é quem

| papel | vez | sessão | provider | conta | modelo | esforço | abertura |
|---|---|---|---|---|---|---|---|
| árbitro | - | arb | claude | padrao | opus[1m] | high | - |
| executor | low | w-t* | claude | 200-01 | opus[1m] | medium | - |
| executor | high | w-t* | codex | openai-codex | gpt-6-sol | high | --headless |
| revisor | - | w-rev-* | claude | padrao | opus[1m] | high | - |
"""
ROWS3 = ROWS + "| 3 | third | §3 | `c.txt` | `true` | 2 | low | — |\n"
ONE = "| 1 | first | §1 | `a.txt` | `true` | 1 | low | {rot} |\n"


def with_molds(tmp_path, e):
    """Every placeholder shows up in the text sent; the untouchables list is the only multi-line one,
    so it goes last, on its own lines."""
    m = tmp_path / "molds"
    m.mkdir(exist_ok=True)
    for role in ("executor", "revisor"):
        (m / f"kickoff-{role}.md").write_text(
            role.upper() + " T{task} {title} wt={worktree} br={branch} dir={run_dir} plan={plan} "
            "c={contract} ex={executor} rev={reviewer} arb={arbiter} base={base}\n{untouchables}")
    return {**e, "ORQ_KICKOFF_DIR": str(m)}


def test_opens_the_wave_up_to_paralelo_with_worktrees_rows_and_kickoffs(tmp_path):
    root = tmp_path.resolve()
    d, r, g, e, log = start(tmp_path, rows=ROWS3, contract=CONTRACT)
    e = with_molds(tmp_path, e)
    cfg = json.loads((d / "orq.json").read_text())
    cfg["untouchables"] = ["CLAUDE.md", "docs/*"]
    (d / "orq.json").write_text(json.dumps(cfg))
    base = g("rev-parse", "HEAD")
    assert run(e, "advance").stdout.splitlines() == ["opened T1: w-t1 + w-rev-1",
                                                     "opened T2: w-t2 + w-rev-2"]
    # The run's gid in the names: a later run on the same branch never meets these.
    wt1, wt2 = root / "repo-g1-t1", root / "repo-g1-t2"
    assert git_in(wt1)("branch", "--show-current") == "main-g1-t1"
    assert git_in(wt2)("rev-parse", "HEAD") == g("rev-parse", "HEAD")
    assert [m for m in sent(log) if m.startswith("--new ")] == [
        f"--new w-t1 {wt1} --provider claude --conta 200-01 --model opus[1m] --effort medium",
        f"--new w-rev-1 {wt1} --provider claude --model opus[1m] --effort high --read-only --terminal",
        f"--new w-t2 {wt2} --provider codex --conta openai-codex --model gpt-6-sol --effort high --headless",
        f"--new w-rev-2 {wt2} --provider claude --model opus[1m] --effort high --read-only --terminal",
    ]
    assert [(x["task"], x["titulo"], x["executor"], x["par"]) for x in events(d)
            if x["tipo"] == "task_inicio"] == [(1, "first", "w-t1", "w-rev-1"), (2, "second", "w-t2", "w-rev-2")]
    head = (f"EXECUTOR T1 first wt={wt1} br=main-g1-t1 dir={d} plan={root / 'plan.orq.md'} "
            f"c={root / 'regras.md'} ex=w-t1 rev=w-rev-1 arb=arb base={base}")
    assert [m for m in sent(log) if m.startswith("w-t1 ")] == [f"w-t1 [painel: orquestrador g1] {head}"]
    assert (d / "kickoffs" / "task1-executor.md").read_text() == f"{head}\n- CLAUDE.md\n- docs/*"
    assert (d / "kickoffs" / "task1-revisor.md").read_text().startswith("REVISOR T1 first")
    assert any(x["kind"] == "advance" and x["task"] == 1 and "abriu w-t1" in x["text"]
               for x in timeline_lines(d))
    seen = [json.loads(l) for l in (d / "sessions.jsonl").read_text().splitlines()]
    assert [(x["name"], x["role"], x["task"], x["provider"]) for x in seen if x["task"] is not None] == [
        ("w-t1", "executor", 1, "claude"), ("w-rev-1", "revisor", 1, "claude"),
        ("w-t2", "executor", 2, "codex"), ("w-rev-2", "revisor", 2, "claude")]
    assert (seen[0]["name"], seen[0]["role"], seen[0]["task"]) == ("arb", "arbitro", None)
    assert run(e, "advance").stdout == ""   # the wave is full, T3 waits for wave 1


def test_sequential_opens_on_the_main_line_after_the_previous_is_integrated(tmp_path):
    root = tmp_path.resolve()
    rows = ONE.format(rot="—") + "| 2 | second | §2 | `b.txt` | `true` | 2 | low | — |\n"
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=rows, contract=CONTRACT)
    e = with_molds(tmp_path, e)
    assert run(e, "advance").stdout.splitlines() == ["opened T1: w-t1 + w-rev-1"]
    assert f"--new w-t1 {root / 'repo'} --provider claude" in "\n".join(sent(log))
    assert run(e, "advance").stdout == ""
    (r / "a.txt").write_text("x\n")
    g("commit", "-qam", "t1")
    close(d, 1, g("rev-parse", "HEAD"))
    assert run(e, "advance").stdout.splitlines() == [f"integrated T1 {g('rev-parse', 'HEAD')[:12]}",
                                                     "opened T2: w-t2 + w-rev-2"]


def test_subagent_review_opens_only_the_executor(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"),
                            revisao="subagente", contract=CONTRACT)
    e = with_molds(tmp_path, e)
    assert run(e, "advance").stdout.splitlines() == ["opened T1: w-t1 + subagente"]
    assert [m.split()[1] for m in sent(log) if m.startswith("--new ")] == ["w-t1"]
    assert [x["par"] for x in events(d) if x["tipo"] == "task_inicio"] == ["subagente"]
    assert not (d / "kickoffs" / "task1-revisor.md").exists()
    kick = (d / "kickoffs" / "task1-executor.md").read_text()
    assert "rev=subagente" in kick and "br=main " in kick
    # On the main line the base is the checkout's HEAD; no untouchables → "- none".
    assert kick.endswith(f"base={g('rev-parse', 'HEAD')}\n- none")


def test_born_on_another_model_fails_once_and_is_not_retried(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"), contract=CONTRACT)
    e = {**with_molds(tmp_path, e), "FAKE_BORN_MODEL": "sonnet"}
    root = tmp_path.resolve()
    # What exists already is named, so the arbiter can finish or clean by hand.
    assert run(e, "advance").stdout.strip() == (
        "failed open T1: born wrong: w-t1: born on model sonnet, row says opus[1m]. "
        f"Left behind (finish or clean by hand): session w-t1 in {root / 'repo'}; "
        "task_inicio NOT recorded; no kick-off written")
    run(e, "advance")
    assert len([m for m in sent(log) if m.startswith("--new ")]) == 1
    assert [(x["passo"], x["task"]) for x in events(d) if x["tipo"] == "advance_falhou"] == [("open", 1)]
    assert not any(x["tipo"] == "task_inicio" for x in events(d))
    assert len([m for m in sent(log) if "orq advance failed at open T1" in m]) == 1


def test_last_task_integrated_wakes_the_arbiter_for_the_final_review_once(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    started(e, tasks=(1,))
    close(d, 1, g("rev-parse", "HEAD"))
    assert run(e, "advance").stdout.splitlines()[-1] == "all integrated: arbiter woken for the final review"
    run(e, "advance")
    assert len([x for x in events(d) if x["tipo"] == "tudo_integrado"]) == 1
    assert len([m for m in sent(log) if "Every Task of the plan is integrated" in m]) == 1


def test_a_later_integration_wakes_the_arbiter_for_the_final_review_again(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    started(e, tasks=(1,))
    close(d, 1, g("rev-parse", "HEAD"))
    run(e, "advance")
    time.sleep(1.1)   # encerramentos e eventos têm precisão de segundos
    (r / "a.txt").write_text("fixed\n")
    g("commit", "-qam", "fix after the final review")
    close(d, 1, g("rev-parse", "HEAD"))
    assert run(e, "advance").stdout.splitlines()[-1] == "all integrated: arbiter woken for the final review"
    run(e, "advance")
    assert [x["tipo"] for x in events(d) if x["tipo"] in ("integrada", "tudo_integrado")] == [
        "integrada", "tudo_integrado", "integrada", "tudo_integrado"]
    assert len([m for m in sent(log) if "Every Task of the plan is integrated" in m]) == 2


def test_full_proof_batch_is_announced_once_its_tasks_are_integrated(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="r1.md"), prova="lote(1)")
    started(e, tasks=(1,))
    h = g("rev-parse", "HEAD")
    close(d, 1, h)
    with (d / "prova-fila.jsonl").open("w") as f:   # what `orq commit` queues
        f.write(json.dumps({"ts": "2026-09-28T10:00:00-03:00", "task": 1, "roteiro": "r1.md", "hash": h}) + "\n")
    out = run(e, "advance").stdout.splitlines()
    assert out[1] == f"batch: lote 1: T1 {tmp_path.resolve() / 'r1.md'} {h[:12]}"
    assert [json.loads(l)["tasks"] for l in (d / "prova-lotes.jsonl").read_text().splitlines()] == [[1]]
    assert any("[painel: orquestrador g1] [decisao] Proof batch ready — lote 1: T1" in m for m in sent(log))


CONTRACT6 = """## Quem é quem

| papel | sessão | provider | conta | modelo | esforço |
|---|---|---|---|---|---|
| **árbitro** | arb | claude | padrao | opus[1m] | high |
| **executor** | `w-t*` | Claude | **200-01** | - | — |
| revisor | w-rev-* | claude | padrao | sonnet | high |
"""
CONTRACT7 = """## Quem é quem

| papel | vez | sessão | provider | conta | modelo | esforço |
|---|---|---|---|---|---|---|
| árbitro | - | arb | claude | padrao | opus[1m] | high |
| **executor** | 1 | **w-a-t*** | claude | 200-01 | opus[1m] | medium |
| executor | 2 | w-b-t* | codex | `openai-codex` | gpt-6-sol | high |
"""


@pytest.mark.parametrize("contract", [CONTRACT, CONTRACT6, CONTRACT7])
def test_team_table_reads_like_the_backend(contract):
    from orq_contract_client import roles as orq_papeis
    m = orq_mod()
    ours = [(x["papel"], x["vez"], x["sessao"], x["provider"].lower(), x["conta"], x["modelo"], x["esforco"])
            for x in m.team_rows(contract)]
    theirs = [(p.papel, p.vez, p.sessao, p.provider, p.conta, p.modelo, p.esforco)
              for p in orq_papeis.ler(contract)]
    assert ours and ours == theirs


def test_role_row_rotation_risk_names_and_flags(monkeypatch):
    m = orq_mod()
    monkeypatch.setattr(m, "jev_config", lambda auto=False: {})   # in-process: the machine's key would add --jev
    rows = [{"papel": "executor", "vez": "1", "sessao": "a"}, {"papel": "executor", "vez": "2", "sessao": "b"}]
    assert [m.role_row(rows, "executor", n, "")["sessao"] for n in (1, 2, 3)] == ["a", "b", "a"]
    rows = [{"papel": "executor", "vez": "low", "sessao": "a"}, {"papel": "executor", "vez": "high", "sessao": "b"}]
    assert m.role_row(rows, "executor", 5, "high")["sessao"] == "b"
    with pytest.raises(m.OrqError):
        m.role_row(rows, "executor", 5, "")
    assert [m.session_name("w-t*", 4), m.session_name("w-review", 4)] == ["w-t4", "w-review-t4"]
    # The name the backend gives the session (app/names.py), not the one asked for.
    assert [m.session_name("revisão-t*", 4), m.session_name("rev x", 4)] == ["revisao-t4", "rev-x-t4"]
    assert m.open_flags({"provider": "claude"}, False) == ["--provider", "claude"]
    assert m.open_flags({"provider": "claude"}, True) == ["--provider", "claude", "--read-only", "--terminal"]
    assert m.open_flags({"provider": "claude", "abertura": "--terminal"}, False) == ["--provider", "claude", "--terminal"]
    assert m.open_flags({"provider": "claude", "abertura": "--headless"}, False) == ["--provider", "claude", "--headless"]
    # A combinação recusada pelo backend deve falhar sem descartar a proteção.
    for abertura, read_only in (("--headless", True), ("--headless --read-only", False)):
        with pytest.raises(m.OrqError, match="read-only session cannot open with --headless"):
            m.open_flags({"provider": "claude", "sessao": "rev-t*", "abertura": abertura}, read_only)


def test_kickoff_mold_with_an_unknown_placeholder_is_an_error(tmp_path, monkeypatch):
    m = orq_mod()
    (tmp_path / "kickoff-executor.md").write_text("T{task} {nope}")
    monkeypatch.setattr(m, "KICKOFF_DIR", tmp_path)
    with pytest.raises(m.OrqError, match="nope"):
        m.render_kickoff("executor", {"task": 1})


@pytest.mark.parametrize("role", ["executor", "revisor"])
def test_the_real_molds_render_with_the_twelve_keys(role, monkeypatch):
    monkeypatch.delenv("ORQ_KICKOFF_DIR", raising=False)
    m = orq_mod()
    keys = ("task", "title", "worktree", "branch", "run_dir", "plan", "contract", "executor",
            "reviewer", "arbiter", "base", "untouchables")
    text = m.render_kickoff(role, {k: f"<{k}>" for k in keys})
    assert "{" not in text and "}" not in text
    assert "<task>" in text and "<untouchables>" in text


def test_accented_session_pattern_opens_under_the_backend_name(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"),
                            contract=CONTRACT.replace("w-rev-*", "revisão-t*"))
    e = with_molds(tmp_path, e)
    assert run(e, "advance").stdout.splitlines() == ["opened T1: w-t1 + revisao-t1"]
    assert [m.split()[1] for m in sent(log) if m.startswith("--new ")] == ["w-t1", "revisao-t1"]
    assert [x["par"] for x in events(d) if x["tipo"] == "task_inicio"] == ["revisao-t1"]
    assert any(m.startswith("revisao-t1 [painel: orquestrador g1] REVISOR T1") for m in sent(log))


def test_a_task_whose_opening_failed_still_holds_its_paralelo_slot(tmp_path):
    d, r, g, e, log = start(tmp_path, contract=CONTRACT)
    e = with_molds(tmp_path, e)
    # plan-check keeps a wave within Paralelo; a plan edited after the stamp is what gets past it.
    plan = tmp_path / "plan.orq.md"
    plan.write_text(plan.read_text() + "| 3 | third | §3 | `c.txt` | `true` | 1 | low | — |\n")
    with (d / "eventos.jsonl").open("a") as f:   # the arbiter may open T1 by hand at any time
        f.write(json.dumps({"ts": "2026-09-28T10:00:00-03:00", "tipo": "advance_falhou",
                            "passo": "open", "task": 1, "motivo": "x"}) + "\n")
    assert run(e, "advance").stdout.splitlines() == ["opened T2: w-t2 + w-rev-2"]


def test_prove_born_reads_the_sidecar_then_the_pane(tmp_path, monkeypatch):
    m = orq_mod()
    monkeypatch.setenv("HOME", str(tmp_path))
    row = {"provider": "claude", "modelo": "opus[1m]"}
    side = tmp_path / ".hangar" / "claude-headless"
    side.mkdir(parents=True)
    (side / "s1.json").write_text(json.dumps({"provider": "codex", "model": "gpt-6-sol"}))
    assert m.prove_born("s1", row) == "s1: born on codex, row says claude"
    pane = {"rc": 0, "out": "claude --session-id u --model claude-opus-5-5\n"}
    calls = []

    def fake_run(cmd, **kw):
        calls.append(cmd)
        return subprocess.CompletedProcess(cmd, pane["rc"], pane["out"], "")

    monkeypatch.setattr(m.subprocess, "run", fake_run)
    assert m.prove_born("s2", row) is None
    assert calls[-1][:2] == ["tmux", "display"] and "=s2:" in calls[-1]
    assert m.prove_born("s2", {"provider": "codex", "modelo": "gpt-6-sol"}).startswith(
        "s2: pane started `claude --session-id u")
    pane["rc"] = 1
    assert m.prove_born("s2", row) == "s2: no sidecar and no tmux pane to prove what was born"


def wait_for(pred, timeout=15):
    end = time.time() + timeout
    while time.time() < end:
        if pred():
            return True
        time.sleep(0.1)
    return False


def _commit_task_1(d, r, g, e):
    """Round 1 of Task 1 delivered, approved and committed through `orq commit`."""
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev1")
    (r / "a.txt").write_text("x\n")
    g("add", "a.txt")
    h = g("stash", "create")
    g("stash", "store", "-m", "task-1 round 1", h)
    run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--commit", h)
    run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova", "--sessao", "rev1")
    g("commit", "-qm", "t1")
    run(e, "commit", "--task", "1", "--hash", g("rev-parse", "HEAD"))


def test_auto_commit_starts_advance_instead_of_waking_the_arbiter(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    _commit_task_1(d, r, g, e)
    assert wait_for(lambda: any(x["tipo"] == "integrada" for x in events(d))), \
        (d / "advance.log").read_text()
    assert not any("Release the next ready Task" in m for m in sent(log))
    assert "orq advance started" in (d / "registro.md").read_text()


def test_non_auto_commit_still_wakes_the_arbiter_and_starts_nothing(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    cfg = json.loads((d / "orq.json").read_text())
    cfg.pop("auto")
    (d / "orq.json").write_text(json.dumps(cfg))
    _commit_task_1(d, r, g, e)
    assert any(m.startswith("arb [decisao] Task 1 closed and checked") and
               m.endswith("Release the next ready Task(s).") for m in sent(log))
    assert run(e, "advance", "--detach").stdout == ""
    time.sleep(0.5)
    assert not (d / "advance.log").exists() and not (d / "advance.lock").exists()


def test_a_spawn_that_fails_is_journaled_and_never_a_traceback(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    (d / "advance.log").mkdir()   # opening the log fails: spawn_advance raises OSError
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev1")
    (r / "a.txt").write_text("x\n")
    g("add", "a.txt")
    h = g("stash", "create")
    g("stash", "store", "-m", "task-1 round 1", h)
    ev = run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--commit", h)
    assert ev.stdout == "ok\n" and "Traceback" not in ev.stderr and "orq advance" in ev.stderr
    v = run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova",
            "--sessao", "rev1")
    assert v.stdout == "ok\n" and "Traceback" not in v.stderr
    assert any(m.startswith("ex1 APROVA Task 1") for m in sent(log))   # the notice still went out
    g("commit", "-qm", "t1")
    c = run(e, "commit", "--task", "1", "--hash", g("rev-parse", "HEAD"))
    assert c.stdout == "ok\n" and "Traceback" not in c.stderr and "orq advance" in c.stderr
    reg = (d / "registro.md").read_text()
    assert "orq advance started" not in reg and "commit T1: orq advance not started" in reg
    # The arbiter wakes each time: nobody else would integrate or release the next Task.
    woke = [m for m in sent(log) if m.startswith("arb [painel: orquestrador g1] [decisao]")
            and "orq advance did not start" in m]
    assert len(woke) == 3
    assert len([x for x in timeline_lines(d) if x["kind"] == "failed" and "orq advance" in x["text"]]) == 3


def test_a_crashing_pass_is_a_failed_step_reported_once(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()
    calls = []

    def crash(*a):
        calls.append(1)
        if len(calls) == 1:
            raise KeyError("boom")
    monkeypatch.setattr(m, "_pass", crash)
    assert m.advance(d) == ["failed advance: KeyError: 'boom'"]
    assert m.advance(d) == [] and len(calls) == 1   # held until a new close, as the wake promised
    assert [(x["passo"], x.get("task")) for x in events(d) if x["tipo"] == "advance_falhou"] == \
        [("advance", None)]
    assert len([s for s in sent(log) if "orq advance failed at advance" in s]) == 1
    later = datetime.fromtimestamp(time.time() + 5).astimezone().isoformat(timespec="seconds")
    with (d / "closed.jsonl").open("a") as f:
        f.write(json.dumps({"ts": later, "task": 1, "hash": g("rev-parse", "HEAD")}) + "\n")
    m.advance(d)
    assert len(calls) == 2


def test_only_the_orchestrator_sends_as_a_panel_notice(tmp_path):
    d, r, g, e, log = start(tmp_path)
    started(e)
    run(e, "notify", "[decisao] waiting for the arbiter")   # a session's message: keeps its [de: …]
    close(d, 1, "deadbeef" * 5)                              # the orchestrator's wake
    run(e, "advance")
    assert sum(m.startswith("arb ") for m in sent(log)) == 2
    assert (tmp_path / "sent.log.painel").read_text().splitlines() == ["arb"]


@pytest.mark.parametrize("move", ["branch", "detached"])
def test_a_main_checkout_off_the_run_branch_waits(tmp_path, move):
    d, r, g, e, log = start(tmp_path, integ="`test -f b.txt`", contract=CONTRACT)
    e = with_molds(tmp_path, e)
    started(e, tasks=(1,))
    close(d, 1, branch_commit(g, r, "main-t1", "b.txt", "b\n"))
    g("checkout", "-q", *(["-b", "other"] if move == "branch" else ["--detach"]))
    before, n_events = g("rev-parse", "HEAD"), len(events(d))
    assert run(e, "advance").stdout == ""
    run(e, "advance")
    # Neither merged into the wrong branch nor opened T2 on it.
    assert g("rev-parse", "HEAD") == before and len(events(d)) == n_events and sent(log) == []
    waits = [x for x in timeline_lines(d) if x["kind"] == "notice"]
    assert len(waits) == 1 and "main" in waits[0]["text"]   # once per episode
    g("checkout", "-q", "main")
    out = run(e, "advance").stdout
    assert "integrated T1" in out and "opened T2" in out


def test_init_without_auto_keeps_an_auto_run_auto(tmp_path):
    d, r, g, e, log = start(tmp_path)
    args = ["init", "--arbiter", "arb2", "--repo", str(r), "--contract", str(tmp_path / "regras.md"),
            "--plan", str(tmp_path / "plan.orq.md")]
    run(e, *args, "--auto", "--jev", "shadow", "--regex", "on")
    run(e, *args)   # the arbiter changing only the untouchables forgot --auto
    cfg = json.loads((d / "orq.json").read_text())
    assert (cfg["auto"], cfg["jev"], cfg["regex"], cfg["arbiter"]) == (True, "shadow", "on", "arb2")
    run(e, *args, "--regex", "shadow")
    cfg = json.loads((d / "orq.json").read_text())
    assert (cfg["auto"], cfg["jev"], cfg["regex"]) == (True, "shadow", "shadow")


def test_auto_commit_whose_proof_queue_fails_wakes_the_arbiter(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    (d / "advance.log").mkdir()   # no background pass racing the assertions
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev1")
    (r / "a.txt").write_text("x\n")
    g("add", "a.txt")
    h = g("stash", "create")
    g("stash", "store", "-m", "task-1 round 1", h)
    run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--commit", h)
    run(e, "event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova", "--sessao", "rev1")
    g("commit", "-qm", "t1")
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()
    monkeypatch.setattr(m, "spawn_advance", lambda d: None)

    def boom(*a):
        raise m.OrqError("plan unreadable")
    monkeypatch.setattr(m, "_queue_proof", boom)
    assert m.main(["commit", "--task", "1", "--hash", g("rev-parse", "HEAD")]) == 0
    assert any(x["kind"] == "failed" and x["task"] == 1 and "plan unreadable" in x["text"]
               for x in timeline_lines(d))
    assert any(s.startswith("arb [painel: orquestrador g1] [decisao]") and "plan unreadable" in s
               for s in sent(log))


def test_a_crash_outside_the_pass_itself_is_still_a_failed_step(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()

    def bad(*a):
        raise ValueError("closed.jsonl unreadable")
    monkeypatch.setattr(m, "_closes", bad)
    assert m.advance(d) == ["failed advance: ValueError: closed.jsonl unreadable"]
    assert [x["passo"] for x in events(d) if x["tipo"] == "advance_falhou"] == ["advance"]
    assert len([s for s in sent(log) if "orq advance failed at advance" in s]) == 1


def test_cmd_advance_reports_a_crash_to_the_arbiter(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()

    def crash(d):
        raise OSError("advance.lock: disk full")
    monkeypatch.setattr(m, "advance", crash)
    assert m.main(["advance"]) == 1
    assert [x["passo"] for x in events(d) if x["tipo"] == "advance_falhou"] == ["advance"]
    assert any("orq advance failed at advance" in s and "disk full" in s for s in sent(log))


def test_a_merge_that_hangs_is_killed_and_is_a_failed_step(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path, integ="`test -f b.txt`")
    started(e)
    h = branch_commit(g, r, "main-t1", "b.txt", "b\n")
    hook = r / ".git" / "hooks" / "pre-merge-commit"
    hook.write_text("#!/bin/sh\nsleep 60\n")
    hook.chmod(0o755)
    close(d, 1, h)
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()
    monkeypatch.setattr(m, "MERGE_TIMEOUT_S", 1)
    t0 = time.monotonic()
    out = m.advance(d)
    assert time.monotonic() - t0 < 20   # the hook's child holding the pipe is killed too
    assert out[0].startswith("failed integrate T1") and "timed out" in out[0]
    assert subprocess.run(["git", "-C", str(r), "rev-parse", "-q", "--verify", "MERGE_HEAD"],
                          capture_output=True).returncode != 0   # never left mid-merge
    assert not any(x["tipo"] == "integrada" for x in events(d))


def test_open_flags_of_an_auto_run_read_the_jev_key_the_settings_screen_wrote(tmp_path, monkeypatch):
    cfg = tmp_path / "cfg"
    cfg.mkdir()
    (cfg / "runtime-config.json").write_text(json.dumps({"jev_api_key": "k"}))
    for k, v in {"CLAUDE_CONFIG_DIR": str(cfg), "HOME": str(tmp_path), "TYPESAFE_API_KEY": ""}.items():
        monkeypatch.setenv(k, v)
    assert "--jev" in orq_mod().open_flags({"provider": "claude"}, False)


def test_a_failed_final_review_wake_does_not_mark_the_plan_integrated(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    started(e, tasks=(1,))
    close(d, 1, g("rev-parse", "HEAD"))
    run({**e, "FAKE_FAIL": "arb"}, "advance")
    assert not any(x["tipo"] == "tudo_integrado" for x in events(d))
    assert not any(x["tipo"] == "advance_falhou" for x in events(d))   # a send failure retries


def test_a_final_review_wake_that_fails_twice_is_retried_and_recorded_once(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    started(e, tasks=(1,))
    close(d, 1, g("rev-parse", "HEAD"))
    for _ in range(2):
        run({**e, "FAKE_FAIL": "arb"}, "advance")
    assert run(e, "advance").stdout.splitlines()[-1] == "all integrated: arbiter woken for the final review"
    run(e, "advance")
    assert len([x for x in events(d) if x["tipo"] == "tudo_integrado"]) == 1
    assert not any(x["tipo"] == "advance_falhou" for x in events(d))
    assert (tmp_path / "sent.log.painel").read_text().splitlines() == ["arb"]   # woken once


def test_a_final_review_wake_that_fails_three_times_is_a_failed_step(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    started(e, tasks=(1,))
    close(d, 1, g("rev-parse", "HEAD"))
    for _ in range(4):
        run({**e, "FAKE_FAIL": "arb"}, "advance")
    assert [x["passo"] for x in events(d) if x["tipo"] == "advance_falhou"] == ["final"]
    assert not any(x["tipo"] == "tudo_integrado" for x in events(d))


def test_a_failed_batch_wake_leaves_the_proofs_pending(tmp_path):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="r1.md"), prova="lote(1)")
    started(e, tasks=(1,))
    h = g("rev-parse", "HEAD")
    close(d, 1, h)
    with (d / "prova-fila.jsonl").open("w") as f:
        f.write(json.dumps({"ts": "2026-09-28T10:00:00-03:00", "task": 1, "roteiro": "r1.md", "hash": h}) + "\n")
    run({**e, "FAKE_FAIL": "arb"}, "advance")
    assert not (d / "prova-lotes.jsonl").exists()
    assert [p["task"] for p in orq_mod().pending_proofs(d)] == [1]


def test_a_merge_killed_before_merge_head_leaves_no_dirty_tree(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path)
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()

    def half_merge(cmd, timeout):   # killed after writing files, before MERGE_HEAD
        (r / "a.txt").write_text("half\n")
        g("add", "a.txt")
        raise subprocess.TimeoutExpired(cmd, timeout)
    monkeypatch.setattr(m, "_run_bounded", half_merge)
    with pytest.raises(m.OrqError, match="timed out"):
        m._merge(d, str(r), 1, "deadbeef" * 5)
    assert g("status", "--porcelain") == "" and (r / "a.txt").read_text() == "1\n2\n3\n"


def test_a_killed_merge_whose_reset_fails_names_the_dirty_files(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path)
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()

    def half_merge(cmd, timeout):   # the killed git left its index.lock behind
        (r / "a.txt").write_text("half\n")
        (r / ".git" / "index.lock").write_text("")
        raise subprocess.TimeoutExpired(cmd, timeout)
    monkeypatch.setattr(m, "_run_bounded", half_merge)
    with pytest.raises(m.OrqError, match="reset --merge") as ex:
        m._merge(d, str(r), 1, "deadbeef" * 5)
    assert "a.txt" in str(ex.value)


def test_run_bounded_never_waits_on_a_pipe_held_outside_the_group(tmp_path):
    m = orq_mod()
    t0 = time.monotonic()
    # setsid: the grandchild escapes killpg and keeps stdout open.
    with pytest.raises(subprocess.TimeoutExpired):
        m._run_bounded(["sh", "-c", "setsid sleep 20 & sleep 20"], 0.5)
    assert time.monotonic() - t0 < 12


def test_a_wake_that_cannot_even_be_journaled_never_raises(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path)
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()
    for exc in (KeyError("arbiter"), OSError("disk full"), ValueError("bad"), m.OrqError("send")):
        def boom(*a, exc=exc, **k):
            raise exc
        monkeypatch.setattr(m, "_wake", boom)
        monkeypatch.setattr(m, "journal_append", lambda *a: (_ for _ in ()).throw(OSError("ro")))
        m._wake_or_journal(d, "text", "line", 1)


def test_an_event_whose_advance_and_wake_both_fail_still_sends_its_notice(tmp_path, monkeypatch):
    d, r, g, e, log = start(tmp_path, par="sequencial", rows=ONE.format(rot="—"))
    (d / "advance.log").mkdir()   # no background pass racing the assertions
    run(e, "event", "task_inicio", "--task", "1", "--titulo", "t", "--executor", "ex1", "--par", "rev1")
    (r / "a.txt").write_text("x\n")
    g("add", "a.txt")
    h = g("stash", "create")
    g("stash", "store", "-m", "task-1 round 1", h)
    for k, v in e.items():
        monkeypatch.setenv(k, v)
    m = orq_mod()
    monkeypatch.setattr(m, "spawn_advance", lambda d: "no fork")

    def boom(*a, **k):
        raise KeyError("arbiter")
    monkeypatch.setattr(m, "_wake", boom)
    real = m.journal_append

    def ro(d, text):   # only the "not started" line fails to be written
        if "not started" in text or "not woken" in text:
            raise OSError("ro")
        real(d, text)
    monkeypatch.setattr(m, "journal_append", ro)
    run(e, "event", "entrega", "--task", "1", "--rodada", "1", "--commit", h)
    assert m.main(["event", "veredito", "--task", "1", "--rodada", "1", "--resultado", "aprova",
                   "--sessao", "rev1"]) == 0
    assert any(s.startswith("ex1 APROVA Task 1") for s in sent(log))
