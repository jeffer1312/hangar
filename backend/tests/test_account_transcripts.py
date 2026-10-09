"""Regras da junção de conversas: nada é sobrescrito e repetir não duplica."""
import os
import stat

import pytest

from app import account_transcripts


def _write(path, body):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


def test_merge_skips_identical_and_renames_conflicts(tmp_path):
    home, default = tmp_path / "conta", tmp_path / "padrao"
    _write(home / "projects" / "-p" / "new.jsonl", "new")
    _write(home / "projects" / "-p" / "same.jsonl", "same")
    _write(home / "projects" / "-p" / "diff.jsonl", "mine")
    _write(default / "projects" / "-p" / "same.jsonl", "same")
    _write(default / "projects" / "-p" / "diff.jsonl", "theirs")
    _write(default / "projects" / "-p" / "diff.from-work.jsonl", "older")
    os.utime(home / "projects" / "-p" / "new.jsonl", (1_000_000, 1_000_000))

    count = account_transcripts.keep(home, default, ("projects",), "work")

    assert count == {"merged": 1, "skipped": 1, "renamed": 1}
    kept = default / "projects" / "-p"
    assert (kept / "new.jsonl").stat().st_mtime == 1_000_000
    assert (kept / "diff.jsonl").read_text(encoding="utf-8") == "theirs"
    assert (kept / "diff.from-work-2.jsonl").read_text(encoding="utf-8") == "mine"
    assert account_transcripts.keep(home, default, ("projects",), "work") == {
        "merged": 0, "skipped": 3, "renamed": 0}


@pytest.mark.skipif(os.name == "nt", reason="modo POSIX")
def test_copy_keeps_file_mode_and_new_dirs_are_private(tmp_path):
    home, default = tmp_path / "conta", tmp_path / "padrao"
    _write(home / "projects" / "-p" / "s" / "a.jsonl", "x")
    os.chmod(home / "projects" / "-p" / "s" / "a.jsonl", 0o600)

    account_transcripts.keep(home, default, ("projects",), "work")

    def mode(p):
        return stat.S_IMODE(p.stat().st_mode)
    assert mode(default / "projects" / "-p" / "s" / "a.jsonl") == 0o600
    assert mode(default) == 0o700
    assert mode(default / "projects" / "-p" / "s") == 0o700


def test_file_created_meanwhile_is_reevaluated(tmp_path, monkeypatch):
    home, default = tmp_path / "conta", tmp_path / "padrao"
    _write(home / "projects" / "-p" / "a.jsonl", "same")
    _write(default / "projects" / "-p" / "a.jsonl", "same")
    real = os.path.lexists
    calls = []

    def first_miss(path):
        # Simula a sessão da conta padrão gravando o arquivo entre a checagem e o "xb".
        calls.append(path)
        return False if len(calls) == 1 else real(path)
    monkeypatch.setattr(account_transcripts.os.path, "lexists", first_miss)

    assert account_transcripts.keep(home, default, ("projects",), "work") == {
        "merged": 0, "skipped": 1, "renamed": 0}


def test_failure_names_source_and_target(tmp_path):
    home, default = tmp_path / "conta", tmp_path / "padrao"
    _write(home / "projects" / "-p" / "a.jsonl", "x")
    _write(default / "projects" / "-p", "um arquivo no lugar da pasta")

    with pytest.raises(account_transcripts.MergeError) as e:
        account_transcripts.keep(home, default, ("projects",), "work")

    assert e.value.params()["source"] == str(home / "projects" / "-p" / "a.jsonl")
    assert e.value.params()["target"] == str(default / "projects" / "-p")
