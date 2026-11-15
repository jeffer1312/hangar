"""O que dos grupos fica no Python depois da migração: leitores da lista e o fechamento que o Rust
já fez. Fecha linhas da `conferencia.md` da Task 6."""
import json

import pytest

from app import pair


@pytest.fixture(autouse=True)
def _tmp_pair_dir(tmp_path, monkeypatch):
    monkeypatch.setattr(pair.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_python_mode_list_reads_the_group_and_a_broken_outside_registry_is_empty(tmp_path, monkeypatch):
    """BRG-29: a lista do modo `python` lê o arquivo de grupo; `external_pairs.json` torto vale vazio."""
    from app import registry as R
    from app.adapters.claude_headless import sessions as S
    monkeypatch.setattr(S, "_dir", lambda: tmp_path / "hl")
    monkeypatch.setattr(R.tmux, "list_panes_all", lambda: {})
    monkeypatch.setattr(R.codex_sessions, "list_all", lambda include_incomplete=False: [])
    S.save("s", str(tmp_path), "22222222-2222-2222-2222-222222222222")
    (pair._pair_dir() / "s.json").write_text(json.dumps(
        {"peers": ["b", "ana::y"], "task": "T-1", "gid": "ab12cd34", "harness": {}}), encoding="utf-8")
    registry_file = pair._pair_dir() / "external_pairs.json"
    registry_file.write_text("{", encoding="utf-8")
    reg = R.SessionRegistry(str(tmp_path / "projects"))
    [row] = [r for r in reg.list() if r.name == "s"]
    assert (row.pair_peers, row.pair_gid, row.pair_task, row.pair_external) == (["b", "ana::y"], "ab12cd34", "T-1", None)
    registry_file.write_text(json.dumps([{"local_session": "s", "alias": "ana", "peer_session": "y", "peer_owner": "Ana"}]))
    [row] = [r for r in reg.list() if r.name == "s"]
    assert row.pair_external == {"alias": "ana", "owner": "Ana", "session": "y"}
