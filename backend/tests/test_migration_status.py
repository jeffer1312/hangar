"""Tela de migração: o Python sozinho na porta diz isso, com o motivo, e mede o próprio processo."""
import os

from fastapi.testclient import TestClient

from app import migration_status
from app.api import app
from app.config import settings

TOKEN = "test-migration-status"


def test_python_alone_answers_with_reason_and_own_usage(monkeypatch, tmp_path):
    monkeypatch.setattr(settings, "auth_token", TOKEN)
    monkeypatch.setattr(settings, "reload", False)
    monkeypatch.setattr(settings, "rust_server", False)
    monkeypatch.setattr(migration_status, "_reason", None)
    monkeypatch.setattr(migration_status, "_canos", (0.0, []))
    monkeypatch.setenv("HOME", str(tmp_path))
    client = TestClient(app)
    assert client.get("/api/migration/status").status_code == 401
    body = client.get("/api/migration/status", headers={"Authorization": f"Bearer {TOKEN}"}).json()
    assert body["served_by"] == "python" and body["areas"] is None
    facts = body["python"]
    assert facts["reason"] == "desligado"
    assert facts["processes"]["python"]["pid"] == os.getpid() and facts["processes"]["python"]["rss_bytes"] > 0
    assert facts["processes"]["rust"] is None and facts["processes"]["cano"]["count"] == 0


def test_reason_recorded_by_the_supervisor_wins(monkeypatch):
    monkeypatch.setattr(settings, "reload", True)
    monkeypatch.setattr(migration_status, "_reason", "quedas")
    assert migration_status.reason() == "quedas"
    monkeypatch.setattr(migration_status, "_reason", None)
    assert migration_status.reason() == "reload"


STATUS = "Name:\thangar-server\nVmRSS:\t   85000 kB\nRssAnon:\t   70000 kB\nVmSwap:\t   62000 kB\nThreads:\t12\n"


def test_status_gives_rss_and_swap_in_bytes():
    assert migration_status._status_memory(STATUS) == (85000 * 1024, 62000 * 1024)
    assert migration_status._status_memory("Name:\tx\nVmRSS:\t10 kB\n") == (10 * 1024, 0), "kernel sem VmSwap"


def test_canos_sum_rss_and_swap(monkeypatch):
    monkeypatch.setattr(migration_status, "_cano_pids", lambda: [11, 12])
    monkeypatch.setattr(migration_status, "_read_proc", lambda pid: {11: (100, 40, 1.0), 12: (200, 0, 2.0)}[pid])
    total = migration_status._canos_usage()
    assert (total["count"], total["rss_bytes"], total["swap_bytes"]) == (2, 300, 40)


def test_installed_package_takes_the_branch_from_the_manifest(monkeypatch, tmp_path):
    import json

    from app import diag, git_ops
    (tmp_path / "manifest.json").write_text(json.dumps(
        {"version": "0.1.0.8207", "commit": "abc", "channel": "beta"}), encoding="utf-8")
    monkeypatch.setattr(diag, "_ROOT", tmp_path)
    monkeypatch.setattr(migration_status, "_branch", None)
    monkeypatch.setattr(migration_status, "_branch_failed_at", 0.0)

    def sem_git(*args, **kwargs):
        raise AssertionError("pacote instalado não chama git")
    monkeypatch.setattr(git_ops, "_run", sem_git)
    assert migration_status._checkout_branch() == "beta"
