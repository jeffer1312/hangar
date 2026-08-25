import json
import os

import pytest

from app import tmux
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
        # `has-session` rc=1 = "nao existe". rc=0 pra TUDO (como era antes) diz que a sessao
        # sobreviveu ao kill, e o start passa a recusar — com razao.
        if "has-session" in args:
            return MagicMock(returncode=1, stdout="")
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
    calls = []

    def fake_run(args, **k):
        calls.append(list(args))
        return MagicMock(returncode=1 if "has-session" in args else 0, stdout="")

    monkeypatch.setattr(runner, "RUN", fake_run)
    runner.stop_run("/home/u/myproj")
    kill = next(a for a in calls if "kill-session" in a)
    assert kill[:4] == ["tmux", "-L", "cppkt-run", "kill-session"]
    assert tmux.alvo_de_kill(runner._slug("/home/u/myproj")) in kill


# ── o kill deixou de ser "mandei" e virou "saiu?" ────────────────────────────────────────────────
# Medido nesta VM (psmux 3.3.7, 23/08/2026), com um run de verdade no pane e o alvo `=<nome>`:
#   kill-session  rc=1, 5,1s, "session ... still present after 5s"  -> a sessao VIVE
#   new-session   rc=1, "duplicate session: <nome>"                 -> engolido pelo except: pass
#   list-sessions -> a sessao VELHA, que o run_status devolvia como se fosse a nova
# Dai os dois consertos: conferir depois de matar, e parar de ignorar o rc do new-session.

def _fake_tmux(calls, viva, spawn_rc=0):
    """`RUN` falso: `viva` decide o que o `has-session` responde (uma lista consome uma resposta
    por chamada — e como se a sessao morresse no meio)."""
    respostas = list(viva) if isinstance(viva, list) else None

    def fake(args, **k):
        calls.append(list(args))
        if "has-session" in args:
            v = respostas.pop(0) if respostas else viva
            return MagicMock(returncode=0 if v else 1, stdout="")
        if "list-panes" in args:
            return MagicMock(returncode=0, stdout="4242\n")
        if "new-session" in args:
            return MagicMock(returncode=spawn_rc, stdout="",
                             stderr="duplicate session: x" if spawn_rc else "")
        return MagicMock(returncode=0, stdout="")
    return fake


def test_start_run_recusa_quando_a_sessao_velha_sobrevive(monkeypatch, tmp_path):
    """O caso do usuario: aperta reiniciar, a velha nao morre, a nova nunca sobe — e a tela
    mostrava o estado da VELHA como se fosse a nova, sem erro nenhum."""
    from app import runner
    monkeypatch.setattr(runner, "_prefs_path", lambda: tmp_path / "prefs.json")
    calls = []
    monkeypatch.setattr(runner, "RUN", _fake_tmux(calls, viva=True))
    monkeypatch.setattr(runner.os, "kill", lambda *a: None)
    with pytest.raises(runner.RunnerError) as e:
        runner.start_run(str(tmp_path / "myproj"), "pnpm run dev")
    assert e.value.status == 409
    assert not any("new-session" in a for a in calls)   # nao tenta subir por cima da velha
    # o comando fica lembrado mesmo assim: a intencao e da pessoa, e nao pode custar redigitar
    assert runner.remembered(str(tmp_path / "myproj")) == "pnpm run dev"


def test_sobrevivente_e_morta_pelo_pane_pid(monkeypatch, tmp_path):
    """Contorno que o instalador ja usa: a sessao e NOSSA e o `pane_pid` sai do proprio tmux.
    Nunca `kill-server`, que levaria junto os runs dos OUTROS projetos deste mesmo socket."""
    from app import runner
    monkeypatch.setattr(runner, "_prefs_path", lambda: tmp_path / "prefs.json")
    calls, mortos = [], []
    # viva -> viva (o kill-session nao matou) -> morta (o pane_pid matou)
    monkeypatch.setattr(runner, "RUN", _fake_tmux(calls, viva=[True, False]))
    monkeypatch.setattr(runner.os, "kill", lambda pid, sig: mortos.append(pid))
    runner.start_run(str(tmp_path / "myproj"), "pnpm run dev")
    assert mortos == [4242]
    assert not any("kill-server" in a for a in calls)
    panes = next(a for a in calls if "list-panes" in a)
    slug = runner._slug(str(tmp_path / "myproj"))
    assert f"={slug}:" in panes        # alvo de pane precisa do `=` E do `:`
    assert any("new-session" in a for a in calls)   # morta a velha, a nova sobe


def test_new_session_que_falha_nao_pode_mais_sumir(monkeypatch, tmp_path):
    """Vale nos DOIS sistemas: o rc do new-session era ignorado dentro de um `except: pass`, e o
    "duplicate session" — a unica pista de que o play nao aconteceu — nao chegava a lugar nenhum."""
    from app import runner
    monkeypatch.setattr(runner, "_prefs_path", lambda: tmp_path / "prefs.json")
    monkeypatch.setattr(runner, "RUN", _fake_tmux([], viva=False, spawn_rc=1))
    with pytest.raises(runner.RunnerError) as e:
        runner.start_run(str(tmp_path / "myproj"), "pnpm run dev")
    assert e.value.status == 502
    assert "duplicate session" in e.value.detail


def test_new_session_que_nem_roda_tambem_aparece(monkeypatch, tmp_path):
    """O `except (TimeoutExpired, OSError): pass` engolia ate o tmux ausente."""
    from app import runner
    monkeypatch.setattr(runner, "_prefs_path", lambda: tmp_path / "prefs.json")

    def fake(args, **k):
        if "has-session" in args:
            return MagicMock(returncode=1, stdout="")
        if "new-session" in args:
            raise OSError("nao achei o tmux")
        return MagicMock(returncode=0, stdout="")

    monkeypatch.setattr(runner, "RUN", fake)
    with pytest.raises(runner.RunnerError, match="nao achei o tmux"):
        runner.start_run(str(tmp_path / "myproj"), "pnpm run dev")


def test_stop_run_que_nao_para_levanta(monkeypatch, tmp_path):
    """`{"ok": True}` com o processo vivo era mentira — a mesma regra que o `projects.stop` ja
    escreve pro `stop_command`: orfao invisivel e pior que erro na tela."""
    from app import runner
    monkeypatch.setattr(runner, "RUN", _fake_tmux([], viva=True))
    monkeypatch.setattr(runner.os, "kill", lambda *a: None)
    with pytest.raises(runner.RunnerError) as e:
        runner.stop_run("/home/u/myproj")
    assert e.value.status == 409


def test_alvo_do_kill_e_o_da_plataforma(monkeypatch):
    """O alvo sai do `alvo_de_kill`, a MESMA funcao da producao — no Windows o nome CRU (o `=` faz
    o psmux esperar 5s e nao matar), no POSIX com o `=` (defesa contra o resolve por prefixo).
    Chumbar `=` aqui "pra ficar igual ao resto" INTRODUZIRIA o defeito neste arquivo."""
    from app import runner
    calls = []
    monkeypatch.setattr(runner, "RUN", _fake_tmux(calls, viva=False))
    runner.stop_run("/home/u/myproj")
    kill = next(a for a in calls if "kill-session" in a)
    slug = runner._slug("/home/u/myproj")
    assert kill[-1] == tmux.alvo_de_kill(slug)
    assert kill[-1] == (f"={slug}" if os.name == "posix" else slug)
