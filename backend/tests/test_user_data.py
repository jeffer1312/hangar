"""Dados do usuário em `~/.hangar/config/`, com a `backend/` como lugar antigo enquanto a máquina não
migrou. HOME é sempre temporário: nem o `~/.hangar` nem o `backend/.env` reais são gravados."""
import os
import subprocess
import sys
from pathlib import Path

from app import user_data

BACKEND = Path(__file__).resolve().parent.parent


def _home(monkeypatch, tmp_path) -> Path:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    return home


def test_the_new_place_wins_when_the_file_is_there(monkeypatch, tmp_path):
    home = _home(monkeypatch, tmp_path)
    (home / ".hangar" / "config").mkdir(parents=True)
    (home / ".hangar" / "config" / "peers.json").write_text("{}", encoding="utf-8")
    assert user_data.path("peers.json") == home / ".hangar" / "config" / "peers.json"
    # Arquivo a arquivo: o `.env` ainda não migrado continua na `backend/`.
    assert user_data.path(".env") == BACKEND / ".env"


def test_without_the_new_file_the_old_place_and_no_folder_is_created(monkeypatch, tmp_path):
    home = _home(monkeypatch, tmp_path)
    assert user_data.path("projects.json") == BACKEND / "projects.json"
    assert not (home / ".hangar").exists()


def _child(home: Path, code: str) -> subprocess.CompletedProcess:
    env = {**os.environ, "HOME": str(home), "USERPROFILE": str(home)}
    return subprocess.run([sys.executable, "-c", code], cwd=BACKEND, env=env, capture_output=True,
                          text=True, timeout=120)


_WIRING = ("from app import peers, peers_api, projects, update_channel, vapid_setup\n"
           "for p in (peers._PEERS_FILE, projects._CONFIG, peers_api._ENV_ARQUIVO, update_channel.ENV_FILE, vapid_setup.ENV):\n"
           "    print(p)\n")


def test_readers_and_writers_use_the_new_place(tmp_path):
    config = tmp_path / ".hangar" / "config"
    config.mkdir(parents=True)
    for name in (".env", "peers.json", "projects.json"):
        (config / name).write_text("{}" if name.endswith(".json") else "", encoding="utf-8")
    out = _child(tmp_path, _WIRING)
    assert out.returncode == 0, out.stderr
    assert [Path(line) for line in out.stdout.splitlines()] == [
        config / "peers.json", config / "projects.json", config / ".env", config / ".env", config / ".env"]


def test_unmigrated_machine_keeps_the_backend_files(tmp_path):
    out = _child(tmp_path, _WIRING)
    assert out.returncode == 0, out.stderr
    assert [Path(line) for line in out.stdout.splitlines()] == [
        BACKEND / "peers.json", BACKEND / "projects.json", BACKEND / ".env", BACKEND / ".env", BACKEND / ".env"]
    assert not (tmp_path / ".hangar").exists()


def test_vapid_keys_are_written_in_the_new_place(tmp_path):
    env = tmp_path / ".hangar" / "config" / ".env"
    env.parent.mkdir(parents=True)
    env.write_text("CP_AUTH_TOKEN=t\n", encoding="utf-8")
    out = _child(tmp_path, "from app import vapid_setup; raise SystemExit(vapid_setup.main())")
    assert out.returncode == 0, out.stderr
    text = env.read_text(encoding="utf-8")
    assert "CP_VAPID_PUBLIC=" in text and "CP_VAPID_PRIVATE=" in text
