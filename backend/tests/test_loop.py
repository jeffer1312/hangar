import json

from app import loop as loop_mod
from app.loop import ACTIVE, LoopLink


def _patch_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(loop_mod.settings, "projects_dir", tmp_path / "projects")






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


def test_decorate_loop(tmp_path, monkeypatch):
    _patch_dir(tmp_path, monkeypatch)
    LoopLink("sessao-a").path.write_text(json.dumps({"status": "running", "iter": 3, "max_iters": 10}), encoding="utf-8")
    from app.registry import _decorate_loop

    class Info:  # objeto minimo com os atributos usados
        name = "sessao-a"; loop_status = None; loop_iter = None; loop_max = None

    i = Info()
    _decorate_loop(i)
    assert i.loop_status == "running" and i.loop_iter == 3 and i.loop_max == 10


def test_decorate_loop_no_sidecar(tmp_path, monkeypatch):
    _patch_dir(tmp_path, monkeypatch)
    from app.registry import _decorate_loop

    class Info:
        name = "sem-loop"; loop_status = None; loop_iter = None; loop_max = None

    i = Info()
    _decorate_loop(i)
    assert i.loop_status is None and i.loop_iter is None
