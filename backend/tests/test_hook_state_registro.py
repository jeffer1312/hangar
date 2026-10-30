import json
import os
import time
from pathlib import Path

import pytest

from app import hook_state

PID_MORTO = 2 ** 22 - 7  # acima de qualquer pid_max real desta suite


def _marcador(base: Path, sid: str, state: str) -> None:
    d = base / ".hangar-state"
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{sid}.json").write_text(json.dumps({"state": state, "ts": time.time()}))


def _registro(base: Path, pid: int, sid: str, status: str) -> Path:
    d = base / "sessions"
    d.mkdir(parents=True, exist_ok=True)
    f = d / f"{pid}.json"
    f.write_text(json.dumps({"pid": pid, "sessionId": sid, "status": status,
                             "statusUpdatedAt": int(time.time() * 1000)}))
    return f


def test_registro_vence_marcador_com_pid_vivo(tmp_path):
    _marcador(tmp_path, "aaa", "working")
    _registro(tmp_path, os.getpid(), "aaa", "waiting")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "awaiting_input"


def test_registro_mapeia_busy_e_idle(tmp_path):
    _registro(tmp_path, os.getpid(), "aaa", "busy")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "working"
    f = _registro(tmp_path, os.getpid(), "aaa", "idle")
    hs._apply_registro(f)
    assert hs.get_state("aaa")[0] == "idle"


def test_registro_de_pid_morto_cai_no_marcador(tmp_path):
    _marcador(tmp_path, "aaa", "working")
    _registro(tmp_path, PID_MORTO, "aaa", "idle")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "working"


def test_registro_shell_e_sessao_ociosa_com_comando_vivo(tmp_path):
    # Turno encerrado com um comando de background de pe: a TUI diz `shell`. O agente aceita
    # mensagem, entao vale `idle` — antes o status ficava de fora do mapa, o registro inteiro era
    # descartado e o fallback do pane dizia "working" numa sessao parada.
    _marcador(tmp_path, "aaa", "working")
    _registro(tmp_path, os.getpid(), "aaa", "shell")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "idle"


def test_demote_awaiting_nao_quebra_shells(tmp_path, monkeypatch):
    # O rebaixamento reescreve a entrada do registro: com uma tupla curta, a proxima consulta de
    # shells estourava IndexError DENTRO do gerador do estado e matava o stream daquela sessao.
    monkeypatch.setattr(hook_state, "shells_de", lambda pid: [])
    hs = hook_state.HookState()
    hs._apply_registro(_registro(tmp_path, os.getpid(), "aaa", "waiting"))
    hs.demote_awaiting("aaa")
    assert hs.get_state("aaa")[0] == "idle"
    assert hs.shells("aaa") == []


def test_shells_so_no_status_shell(tmp_path, monkeypatch):
    monkeypatch.setattr(hook_state, "shells_de", lambda pid: [{"pid": 4242, "cmd": "sleep 900", "desde": None}])
    hs = hook_state.HookState()

    hs._apply_registro(_registro(tmp_path, os.getpid(), "aaa", "shell"))
    assert [s["cmd"] for s in hs.shells("aaa")] == ["sleep 900"]

    # Trabalhando, o filho direto e o comando em PRIMEIRO plano: o turno acontecendo, nao resto.
    hs._apply_registro(_registro(tmp_path, os.getpid(), "aaa", "busy"))
    assert hs.shells("aaa") == []

    hs._apply_registro(_registro(tmp_path, PID_MORTO, "aaa", "shell"))
    assert hs.shells("aaa") == [], "pid morto nao tem filho pra mostrar"


def test_registro_status_desconhecido_nao_vale(tmp_path):
    _marcador(tmp_path, "aaa", "working")
    _registro(tmp_path, os.getpid(), "aaa", "blocked")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "working"


@pytest.mark.parametrize("failure", ["partial", "unknown", "missing"])
def test_invalid_native_update_releases_cached_state(tmp_path, failure):
    _marcador(tmp_path, "aaa", "working")
    f = _registro(tmp_path, os.getpid(), "aaa", "idle")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    transitions = []
    hs.on_transition = lambda sid, state: transitions.append((sid, state))
    assert hs.get_state("aaa")[0] == "idle"

    if failure == "partial":
        f.write_text("{", encoding="utf-8")
    elif failure == "unknown":
        _registro(tmp_path, os.getpid(), "aaa", "unknown-test-status")
    else:
        f.unlink()
    hs._apply_registro(f, notify=True)

    assert hs.get_state("aaa")[0] == "working"
    assert transitions == [("aaa", "working")]
    assert hs.shells("aaa") == []

    _registro(tmp_path, os.getpid(), "aaa", "waiting")
    hs._apply_registro(f, notify=True)
    assert hs.get_state("aaa")[0] == "awaiting_input"
    assert transitions[-1] == ("aaa", "awaiting_input")


def test_registro_removido_volta_ao_marcador(tmp_path):
    _marcador(tmp_path, "aaa", "idle")
    f = _registro(tmp_path, os.getpid(), "aaa", "busy")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    assert hs.get_state("aaa")[0] == "working"
    f.unlink()
    hs._remover_registro(f)
    assert hs.get_state("aaa")[0] == "idle"


def test_registro_dispara_transicao(tmp_path):
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    vistos = []
    hs.on_transition = lambda sid, st: vistos.append((sid, st))
    f = _registro(tmp_path, os.getpid(), "aaa", "busy")
    hs._apply_registro(f, notify=True)
    _registro(tmp_path, os.getpid(), "aaa", "waiting")
    hs._apply_registro(f, notify=True)
    hs._apply_registro(f, notify=True)  # mesmo status regravado: nao re-dispara
    assert vistos == [("aaa", "working"), ("aaa", "awaiting_input")]


def test_marcador_nao_gera_transicao_enquanto_registro_manda(tmp_path):
    _registro(tmp_path, os.getpid(), "aaa", "busy")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    vistos = []
    hs.on_transition = lambda sid, st: vistos.append((sid, st))
    _marcador(tmp_path, "aaa", "idle")
    hs._apply(tmp_path / ".hangar-state" / "aaa.json", notify=True)
    assert hs.get_state("aaa")[0] == "working"
    assert vistos == []


def test_demote_awaiting_rebaixa_registro_em_memoria(tmp_path):
    f = _registro(tmp_path, os.getpid(), "aaa", "waiting")
    hs = hook_state.HookState()
    hs.load_existing([tmp_path])
    hs.demote_awaiting("aaa")
    assert hs.get_state("aaa")[0] == "idle"
    assert json.loads(f.read_text())["status"] == "waiting"  # o arquivo e do Claude


def test_registro_dirs_symlink_conta_uma_vez(tmp_path):
    principal = tmp_path / "principal"
    (principal / "sessions").mkdir(parents=True)
    conta = tmp_path / "conta"
    conta.mkdir()
    (conta / "sessions").symlink_to(principal / "sessions")
    assert hook_state.HookState._registro_dirs([principal, conta]) == [(principal / "sessions").resolve()]
