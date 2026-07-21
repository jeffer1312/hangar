"""Escrita do projects.json: merge preserva campos, validação, lock."""
import json
import threading
from pathlib import Path

import pytest

from app import projects


@pytest.fixture
def cfg(tmp_path, monkeypatch):
    """Aponta o projects.py pra um projects.json isolado e um cwd válido."""
    p = tmp_path / "projects.json"
    monkeypatch.setattr(projects, "_CONFIG", p)
    d = tmp_path / "proj"
    d.mkdir()
    return {"path": p, "cwd": str(d)}


def test_upsert_cria_entry(cfg):
    projects.upsert("meu", cfg["cwd"], "pnpm dev", port=3000)
    data = json.loads(Path(cfg["path"]).read_text())
    assert data["meu"] == {"cwd": cfg["cwd"], "command": "pnpm dev", "port": 3000}


def test_upsert_merge_preserva_stop_command(cfg):
    # cadastro com stop_command, depois edita só a porta -> stop_command PERMANECE.
    projects.upsert("pss", cfg["cwd"], "bash start.sh", stop_command="bash stop.sh")
    projects.upsert("pss", cfg["cwd"], "bash start.sh", port=8116)
    data = json.loads(Path(cfg["path"]).read_text())
    assert data["pss"]["stop_command"] == "bash stop.sh"
    assert data["pss"]["port"] == 8116


def test_validate_cwd_inexistente(cfg):
    with pytest.raises(projects.ProjectError) as e:
        projects.upsert("x", "/nao/existe/mesmo", "pnpm dev")
    assert e.value.status == 400


def test_validate_command_vazio(cfg):
    with pytest.raises(projects.ProjectError) as e:
        projects.upsert("x", cfg["cwd"], "   ")
    assert e.value.status == 400


def test_validate_name_com_barra(cfg):
    with pytest.raises(projects.ProjectError) as e:
        projects.upsert("a/b", cfg["cwd"], "pnpm dev")
    assert e.value.status == 400


def test_upsert_concorrente_nao_perde_entry(cfg):
    # 10 upserts concorrentes de nomes distintos -> todos sobrevivem (lock serializa o RMW).
    def add(i):
        projects.upsert(f"p{i}", cfg["cwd"], "pnpm dev")
    ts = [threading.Thread(target=add, args=(i,)) for i in range(10)]
    [t.start() for t in ts]
    [t.join() for t in ts]
    data = json.loads(Path(cfg["path"]).read_text())
    assert len([k for k in data if k.startswith("p")]) == 10
