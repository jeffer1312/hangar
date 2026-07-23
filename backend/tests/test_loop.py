import json

from app import loop as loop_mod
from app.loop import ACTIVE, LoopLink, new_loop


def _patch_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(loop_mod.settings, "projects_dir", tmp_path / "projects")


def test_new_loop_shape():
    d = new_loop("passar testes", "pytest -x", 10, True)
    assert d["status"] == "running"
    assert d["iter"] == 0
    assert d["history"] == []
    assert d["goal"] == "passar testes"
    assert d["check_cmd"] == "pytest -x"
    assert d["ended_ts"] is None and d["ended_reason"] is None
    assert d["goal_entry_id"] is None and d["goal_delivered_ts"] is None
    assert isinstance(d["started_ts"], float)


def test_link_roundtrip(tmp_path, monkeypatch):
    _patch_dir(tmp_path, monkeypatch)
    link = LoopLink("minha-sessao")
    assert link.get() is None
    link.set(new_loop("g", None, 5, False))
    got = link.get()
    assert got["goal"] == "g" and got["check_cmd"] is None
    link.update(status="done", ended_reason="check passou")
    assert link.get()["status"] == "done"
    link.clear()
    assert link.get() is None


def test_link_corrupt_file_is_none(tmp_path, monkeypatch):
    _patch_dir(tmp_path, monkeypatch)
    link = LoopLink("s")
    link.path.parent.mkdir(parents=True, exist_ok=True)
    link.path.write_text("{broken", encoding="utf-8")
    assert link.get() is None


def test_active_set():
    assert "running" in ACTIVE and "done_claimed" in ACTIVE and "done" not in ACTIVE


from app.git_ops import branch_of


def test_branch_of(tmp_path):
    git = tmp_path / ".git"
    git.mkdir()
    (git / "HEAD").write_text("ref: refs/heads/TICKET-0000\n", encoding="utf-8")
    assert branch_of(str(tmp_path)) == "TICKET-0000"
    (git / "HEAD").write_text("abc123def\n", encoding="utf-8")  # detached
    assert branch_of(str(tmp_path)) is None
    assert branch_of(str(tmp_path / "nao-existe")) is None


from app.transcript import last_assistant_text


def test_last_assistant_text(tmp_path):
    j = tmp_path / "t.jsonl"
    lines = [
        {"type": "user", "message": {"role": "user", "content": "oi"}},
        {"type": "assistant", "message": {"role": "assistant",
            "content": [{"type": "text", "text": "primeira"}]}},
        {"type": "assistant", "message": {"role": "assistant",
            "content": [{"type": "text", "text": "trabalho feito. LOOP_DONE"}]}},
    ]
    j.write_text("\n".join(json.dumps(x) for x in lines), encoding="utf-8")
    out = last_assistant_text(str(j))
    assert out is not None and "LOOP_DONE" in out
    assert last_assistant_text(str(tmp_path / "nada.jsonl")) is None


from app.loop import run_tick, TickCtx, suggest_checks


def _ctx(tmp_path, monkeypatch, *, check=("false", 1, "err"), automations=True,
         branch="TICKET-0000", last=None, deliver_ok=True, entry_delivered=True):
    _patch_dir(tmp_path, monkeypatch)
    sent, queued, pushed = [], [], []
    ctx = TickCtx(
        cwd=str(tmp_path), jsonl=str(tmp_path / "t.jsonl"),
        deliver=lambda p: (sent.append(p), deliver_ok)[1],
        enqueue=queued.append,
        notify=lambda n, b: pushed.append((n, b)),
        automations=lambda: automations,
        branch=lambda cwd: branch,
        last_assistant=lambda j: last,
        run_check=lambda cmd, cwd: (check[1], check[2]),
        entry_delivered=lambda eid: entry_delivered,
    )
    return ctx, sent, queued, pushed


def _mk(name, tmp_path, monkeypatch, **over):
    _patch_dir(tmp_path, monkeypatch)
    d = new_loop(over.pop("goal", "g"), over.pop("check_cmd", "false"),
                 over.pop("max_iters", 10), over.pop("require_branch", True))
    d["goal_entry_id"] = "e1"
    d["goal_delivered_ts"] = over.pop("delivered", 1.0)
    d.update(over)
    link = LoopLink(name)
    link.set(d)
    return link


def test_tick_check_pass_is_done(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch)
    ctx, sent, _, pushed = _ctx(tmp_path, monkeypatch, check=("x", 0, "ok"))
    run_tick("s", ctx)
    assert link.get()["status"] == "done"
    assert pushed and not sent


def test_tick_check_fail_reprompts(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch)
    ctx, sent, _, pushed = _ctx(tmp_path, monkeypatch, check=("x", 1, "AssertionError: boom"))
    run_tick("s", ctx)
    d = link.get()
    assert d["status"] == "running" and d["iter"] == 1
    assert len(sent) == 1 and "AssertionError: boom" in sent[0]
    assert "Não edite arquivos de teste" in sent[0]
    assert not pushed  # status nao mudou -> sem push


def test_tick_anchor_blocks_before_delivery(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch, delivered=None)
    ctx, sent, _, _ = _ctx(tmp_path, monkeypatch, entry_delivered=False)
    assert run_tick("s", ctx) is None
    assert link.get()["iter"] == 0 and not sent


def test_tick_anchor_sets_ts_when_entry_delivered(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch, delivered=None)
    ctx, sent, _, _ = _ctx(tmp_path, monkeypatch, entry_delivered=True, check=("x", 1, "e"))
    run_tick("s", ctx)
    d = link.get()
    assert d["goal_delivered_ts"] is not None and d["iter"] == 1 and sent


def test_tick_stale_status_after_check_is_not_overwritten(tmp_path, monkeypatch):
    # usuario deu DELETE durante o check de 600s -> tick nao pode sobrescrever o stopped
    link = _mk("s", tmp_path, monkeypatch)

    def _check_and_stop(cmd, cwd):
        link.update(status="stopped", ended_reason="parado pelo usuário")
        return (0, "ok")

    ctx, _, _, pushed = _ctx(tmp_path, monkeypatch)
    ctx.run_check = _check_and_stop
    run_tick("s", ctx)
    assert link.get()["status"] == "stopped"  # done NAO sobrescreveu


def test_tick_exhausted(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch, max_iters=1, iter=1)
    ctx, sent, _, pushed = _ctx(tmp_path, monkeypatch, check=("x", 1, "e2"))
    run_tick("s", ctx)
    assert link.get()["status"] == "exhausted" and pushed and not sent


def test_tick_stagnation(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch,
               history=[{"n": 1, "ts": 1.0, "check_exit": 1, "tail": "mesmo erro"}], iter=1)
    ctx, _, _, pushed = _ctx(tmp_path, monkeypatch, check=("x", 1, "mesmo erro"))
    run_tick("s", ctx)
    d = link.get()
    assert d["status"] == "stopped" and "estagnado" in d["ended_reason"]


def test_tick_automations_off_stops_loud(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch)
    ctx, _, _, pushed = _ctx(tmp_path, monkeypatch, automations=False)
    run_tick("s", ctx)
    assert link.get()["status"] == "stopped" and pushed


def test_tick_branch_guard_reverifies(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch)
    ctx, _, _, pushed = _ctx(tmp_path, monkeypatch, branch="main")
    run_tick("s", ctx)
    assert link.get()["status"] == "stopped" and "main" in link.get()["ended_reason"]


def test_tick_branch_none_allowed(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch)
    ctx, sent, _, _ = _ctx(tmp_path, monkeypatch, branch=None, check=("x", 1, "e"))
    run_tick("s", ctx)
    assert link.get()["status"] == "running" and sent


def test_tick_no_check_done_claimed(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch, check_cmd=None)
    ctx, sent, _, pushed = _ctx(tmp_path, monkeypatch, last="feito. LOOP_DONE")
    run_tick("s", ctx)
    assert link.get()["status"] == "done_claimed" and pushed and not sent


def test_tick_no_check_no_marker_reprompts(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch, check_cmd=None)
    ctx, sent, _, _ = _ctx(tmp_path, monkeypatch, last="ainda trabalhando")
    run_tick("s", ctx)
    assert link.get()["iter"] == 1 and len(sent) == 1


def test_tick_check_enoent_fails(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch)
    ctx, _, _, pushed = _ctx(tmp_path, monkeypatch, check=("x", -404, "No such file"))
    run_tick("s", ctx)
    assert link.get()["status"] == "failed" and pushed


def test_tick_deliver_fail_enqueues(tmp_path, monkeypatch):
    link = _mk("s", tmp_path, monkeypatch)
    ctx, sent, queued, _ = _ctx(tmp_path, monkeypatch, deliver_ok=False, check=("x", 1, "e"))
    run_tick("s", ctx)
    assert queued and link.get()["iter"] == 1


def test_tick_history_capped_20(tmp_path, monkeypatch):
    hist = [{"n": i, "ts": float(i), "check_exit": 1, "tail": f"e{i}"} for i in range(1, 21)]
    link = _mk("s", tmp_path, monkeypatch, history=hist, iter=20, max_iters=99)
    ctx, _, _, _ = _ctx(tmp_path, monkeypatch, check=("x", 1, "novo erro"))
    run_tick("s", ctx)
    d = link.get()
    assert len(d["history"]) == 20 and d["history"][-1]["tail"] == "novo erro"


def test_suggest_checks_node(tmp_path):
    (tmp_path / "package.json").write_text('{"scripts": {"check": "x", "test": "y"}}')
    assert suggest_checks(str(tmp_path)) == ["npm run check", "npm run test"]


def test_notify_loop_no_subs_is_noop(tmp_path, monkeypatch):
    from app import push
    monkeypatch.setattr(push.settings, "projects_dir", tmp_path / "projects")
    push.notify_loop("sessao-x", "loop terminou: check passou")  # nao deve levantar
