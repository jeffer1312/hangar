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
