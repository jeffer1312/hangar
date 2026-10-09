"""Regras da junção de conversas: nada é sobrescrito e repetir não duplica."""
import os

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
