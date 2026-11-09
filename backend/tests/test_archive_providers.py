"""Conversa morta de Pi, Kimi e Codex: cada um guarda de um jeito e nenhum guarda como o Claude.

Os fixtures montam o layout REAL de cada um (medido no disco), porque e justamente o layout que o
codigo tem que acertar -- um fake "como a doc sugere" passaria sem provar nada.
"""
import json
from pathlib import Path

import pytest

from app import archive_providers as ap
from app import codex_contas
import codex_contas_apoio


PI_SID = "602c251a-5233-42be-9b55-585c88e072f2"
CODEX_SID = "019f99de-9572-7221-89c0-80c62f883d44"


@pytest.fixture
def codex_home(tmp_path, monkeypatch):
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    home = tmp_path / "codex"
    d = home / "sessions" / "2026" / "07" / "25"
    d.mkdir(parents=True)
    j = d / f"rollout-2026-07-25T12-22-09-{CODEX_SID}.jsonl"
    j.write_text(json.dumps({
        "timestamp": "2026-07-25T15:23:08.027Z", "type": "session_meta",
        "payload": {"session_id": CODEX_SID, "cwd": "/home/u/proj"},
    }) + "\n", encoding="utf-8")
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", home)
    return j


def test_codex_le_o_cwd_do_session_meta(codex_home):
    convs = ap._codex_conversas()
    assert [(c.session_id, c.cwd) for c in convs] == [(CODEX_SID, "/home/u/proj")]
    assert ap.jsonl_de("codex", CODEX_SID) == codex_home


def test_session_id_de_outro_provider_e_recusado():
    with pytest.raises(ValueError):
        ap.jsonl_de("nao-existe", PI_SID)


@pytest.fixture
def duas_contas_codex(tmp_path, monkeypatch):
    monkeypatch.setattr(__import__("pathlib").Path, "home",
                        classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", tmp_path / ".codex")
    default = codex_contas.Account("default", tmp_path / ".codex", True)
    work = codex_contas_apoio.create_account("work")
    sid = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
    paths = {}
    for account, text in ((default, "default"), (work, "work")):
        path = account.home / "sessions" / "2026" / "09" / "09"
        path.mkdir(parents=True)
        rollout = path / f"rollout-2026-09-09T10-00-00-{sid}.jsonl"
        rollout.write_text(json.dumps({
            "type": "session_meta", "payload": {"session_id": sid, "cwd": "/repo"},
        }) + "\n" + text + "\n", encoding="utf-8")
        paths[account.id] = rollout
    return default, work, paths, sid


def test_codex_lista_origem_de_cada_conta(duas_contas_codex):
    default, work, paths, _ = duas_contas_codex
    convs = ap._codex_conversas()
    assert {c.codex_home for c in convs} == {
        str(default.home.resolve()), str(work.home.resolve()),
    }


def test_codex_id_duplicado_exige_conta(duas_contas_codex):
    _default, work, paths, sid = duas_contas_codex
    with pytest.raises(codex_contas.AccountError) as error:
        ap.jsonl_de("codex", sid)
    assert error.value.status == 409
    assert error.value.code == "codex_account_ambiguous_rollout"
    assert ap.jsonl_de("codex", sid, codex_account=work.id) == paths[work.id]
