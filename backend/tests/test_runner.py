import json

from app.runner import detect_runners


def _by_label(runners):
    return {r.label: r for r in runners}


def test_package_json_uses_lockfile_pm(tmp_path):
    (tmp_path / "package.json").write_text(
        json.dumps({"scripts": {"dev": "vite", "build": "vite build"}}), encoding="utf-8")
    (tmp_path / "pnpm-lock.yaml").write_text("", encoding="utf-8")
    by = _by_label(detect_runners(str(tmp_path)))
    assert by["dev"].command == "pnpm run dev"
    assert by["dev"].source == "npm"
    assert by["build"].command == "pnpm run build"


def test_package_json_defaults_to_npm(tmp_path):
    (tmp_path / "package.json").write_text(
        json.dumps({"scripts": {"start": "node ."}}), encoding="utf-8")
    by = _by_label(detect_runners(str(tmp_path)))
    assert by["start"].command == "npm run start"


def test_makefile_targets(tmp_path):
    (tmp_path / "Makefile").write_text("dev:\n\tvite\n\n.PHONY: dev\nbuild:\n\tvite build\n", encoding="utf-8")
    by = _by_label(detect_runners(str(tmp_path)))
    assert by["dev"].command == "make dev"
    assert by["build"].command == "make build"
    assert ".PHONY" not in by  # linhas com ponto sao ignoradas


def test_dev_guess_ranking(tmp_path):
    (tmp_path / "package.json").write_text(
        json.dumps({"scripts": {"build": "x", "serve": "y", "dev": "z"}}), encoding="utf-8")
    guesses = [r.label for r in detect_runners(str(tmp_path)) if r.is_dev_guess]
    assert guesses == ["dev"]  # so um, e o de maior rank


def test_missing_files_no_raise(tmp_path):
    assert detect_runners(str(tmp_path)) == []


def test_cargo_stack(tmp_path):
    (tmp_path / "Cargo.toml").write_text("[package]\nname='x'\n", encoding="utf-8")
    by = _by_label(detect_runners(str(tmp_path)))
    assert by["cargo run"].command == "cargo run"
    assert by["cargo run"].source == "stack"


def test_pyproject_scripts_stack(tmp_path):
    (tmp_path / "pyproject.toml").write_text(
        "[project]\nname='x'\n[project.scripts]\napp='pkg:main'\n", encoding="utf-8")
    by = _by_label(detect_runners(str(tmp_path)))
    assert by["app"].command == "uv run app"
    assert by["app"].source == "stack"


def test_malformed_pyproject_no_raise(tmp_path):
    (tmp_path / "pyproject.toml").write_text("project = 'not-a-table'\n", encoding="utf-8")
    assert detect_runners(str(tmp_path)) == []  # nao levanta, nao emite lixo


def test_remember_roundtrip(tmp_path, monkeypatch):
    from app import runner
    from app.config import settings
    monkeypatch.setattr(settings, "projects_dir", str(tmp_path / "projects"))
    assert runner.remembered("/proj/a") is None
    runner.remember("/proj/a", "pnpm run dev")
    assert runner.remembered("/proj/a") == "pnpm run dev"
    runner.remember("/proj/a", "make serve")  # sobrescreve
    assert runner.remembered("/proj/a") == "make serve"
    assert runner.remembered("/proj/b") is None


from unittest.mock import MagicMock


def test_start_run_builds_isolated_socket_command(tmp_path, monkeypatch):
    from app import runner
    from app.config import settings
    monkeypatch.setattr(settings, "projects_dir", str(tmp_path / "projects"))
    calls = []

    def fake_run(args, **k):
        calls.append(args)
        if "list-sessions" in args:  # status devolve a sessao viva
            return MagicMock(returncode=0, stdout="myproj\t1700000000\n")
        return MagicMock(returncode=0, stdout="")

    monkeypatch.setattr(runner, "RUN", fake_run)
    info = runner.start_run(str(tmp_path / "myproj"), "pnpm run dev")

    spawn = next(a for a in calls if "new-session" in a)
    assert "-L" in spawn and "cppkt-run" in spawn          # socket dedicado
    assert "pnpm run dev" in spawn[-1]                       # comando no exec
    assert info.command == "pnpm run dev"
    assert runner.remembered(str(tmp_path / "myproj")) == "pnpm run dev"  # gravou


def test_run_status_none_when_no_session(monkeypatch):
    from app import runner
    monkeypatch.setattr(runner, "RUN",
                        lambda args, **k: MagicMock(returncode=1, stdout=""))
    assert runner.run_status("/proj/x") is None


def test_stop_run_kills_session(monkeypatch):
    from app import runner
    seen = {}
    monkeypatch.setattr(runner, "RUN",
                        lambda args, **k: seen.update(args=args) or MagicMock(returncode=0, stdout=""))
    runner.stop_run("/home/u/myproj")
    assert seen["args"][:4] == ["tmux", "-L", "cppkt-run", "kill-session"]
    assert "myproj" in seen["args"]
