"""Painel do condutor (app/orq_conductor.py) e as rotas GET /api/orq e /api/orq/{id}/conductor."""
import importlib.util
import json
import os
import subprocess
import sys
import shlex
import tempfile
import time
from datetime import datetime, timedelta
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app.config import settings
from orq_contract_client import call
from orq_http_native import NativeClient


class NativeConductor:
    @staticmethod
    def watchdog(directory, units, now):
        return call("conductor.watchdog", directory=directory, units=units, now=now.timestamp())

    @staticmethod
    def units():
        return call("conductor.units")

    @property
    def JEV_VETO_P(self):
        return call("conductor.veto")


orq_conductor = NativeConductor()

ORQ_PY = Path(__file__).resolve().parents[2] / "skills" / "orquestrar" / "scripts" / "orq.py"
AGORA = datetime.now().astimezone()
H = {"Authorization": "Bearer secret"}


def _iso(dt: datetime) -> str:
    return dt.isoformat(timespec="seconds")


def _batimento(d: Path, idade_s: int, pid: int | None = None, intervalo: int = 60,
               agora: datetime = AGORA) -> None:
    (d / "vigia.json").write_text(json.dumps({
        "ts": _iso(agora - timedelta(seconds=idade_s)), "pid": pid or os.getpid(), "unit": "vigia-g1",
        "arbiter": "arb", "watching": ["rev1", "arb"], "states": {"rev1": "idle", "arb": "idle"},
        "interval_s": intervalo}), encoding="utf-8")


def _pid_morto() -> int:
    p = subprocess.Popen(["true"])
    p.wait()
    return p.pid


UNIDADE = {"Id": "vigia-g1.service", "ActiveState": "active",
           "ActiveEnterTimestamp": "@1790348806", "NRestarts": "2"}


# ── watchdog ────────────────────────────────────────────────────────────────

def test_batimento_novo_e_vivo(tmp_path):
    _batimento(tmp_path, idade_s=40)
    w = orq_conductor.watchdog(tmp_path, None, now=AGORA)
    assert w["alive"] is True and w["source"] == "heartbeat"
    assert w["arbiter"] == "arb" and w["watching"] == ["rev1", "arb"] and w["unit"] == "vigia-g1"
    assert w["since"] is None and w["restarts"] is None and w["unit_state"] is None


def test_batimento_velho_e_parado_desde_a_ultima_volta(tmp_path):
    _batimento(tmp_path, idade_s=151)   # 2 × 60 + 30 = 150 s
    w = orq_conductor.watchdog(tmp_path, None, now=AGORA)
    assert w["alive"] is False and w["last_cycle"] == _iso(AGORA - timedelta(seconds=151))


@pytest.mark.skipif(sys.platform != "linux", reason="a checagem do pid lê /proc")
def test_batimento_novo_de_processo_morto_e_parado(tmp_path):
    _batimento(tmp_path, idade_s=5, pid=_pid_morto())
    assert orq_conductor.watchdog(tmp_path, None, now=AGORA)["alive"] is False


def test_unidade_parada_derruba_batimento_novo(tmp_path):
    _batimento(tmp_path, idade_s=5)
    unidades = {os.path.normpath(str(tmp_path)): {**UNIDADE, "ActiveState": "inactive"}}
    w = orq_conductor.watchdog(tmp_path, unidades, now=AGORA)
    assert w["alive"] is False and w["unit_state"] == "inactive" and w["restarts"] == 2


def test_sem_batimento_vale_a_unidade(tmp_path):
    w = orq_conductor.watchdog(tmp_path, {os.path.normpath(str(tmp_path)): UNIDADE}, now=AGORA)
    assert w["alive"] is True and w["source"] == "systemd" and w["unit"] == "vigia-g1"
    assert w["since"] == datetime.fromtimestamp(1790348806).astimezone().isoformat(timespec="seconds")
    assert w["last_cycle"] is None and w["watching"] == []


def test_sem_nada_e_sem_condutor_e_sem_systemd_e_indisponivel(tmp_path):
    assert orq_conductor.watchdog(tmp_path, {}, now=AGORA)["source"] == "none"
    assert orq_conductor.watchdog(tmp_path, None, now=AGORA)["source"] == "unavailable"


# ── units (systemd) ─────────────────────────────────────────────────────────

SHOW = """Id=vigia-g1.service
ActiveState=active
ActiveEnterTimestamp=@1790348806
NRestarts=0
ExecStart={ path=/x/vigia.sh ; argv[]=/x/vigia.sh arb7 -e /home/u/.hangar/orq/2026-09-23-np -m 15 ; ignore_errors=no ; start_time=[@1790348806] ; stop_time=[n/a] ; pid=1 ; code=(null) ; status=0/0 }

Id=vigia-velho.service
ActiveState=failed
ActiveEnterTimestamp=
NRestarts=4
ExecStart={ path=/x/vigia.sh ; argv[]=/x/vigia.sh exec rev arb 5 ; ignore_errors=no }
"""


def _systemd(monkeypatch, tmp_path, stdout="", rc=0, exc=None):
    programs = Path(tempfile.mkdtemp(prefix="systemd-", dir=tmp_path))
    log = programs / "arguments"
    script = programs / "systemctl"
    body = ("exec /bin/sleep 4\n" if exc else
            f"printf '%s' {shlex.quote(stdout)}\nexit {rc}\n")
    script.write_text(f"#!/bin/sh\nprintf '%s\\n' \"$@\" > {shlex.quote(str(log))}\n" + body, encoding="utf-8")
    script.chmod(0o700)
    monkeypatch.setenv("PATH", str(programs))
    return log


@pytest.mark.skipif(sys.platform != "linux", reason="systemd é consultado somente no Linux")
def test_units_acha_a_unidade_pelo_dir_do_menos_e(monkeypatch, tmp_path):
    log = _systemd(monkeypatch, tmp_path, SHOW)
    u = orq_conductor.units()
    assert list(u) == ["/home/u/.hangar/orq/2026-09-23-np"]
    assert u["/home/u/.hangar/orq/2026-09-23-np"]["Id"] == "vigia-g1.service"
    cmd = ["systemctl", *log.read_text().splitlines()]
    assert cmd[:4] == ["systemctl", "--user", "show", "vigia-*"] and "--timestamp=unix" in cmd


@pytest.mark.skipif(sys.platform != "linux", reason="systemd é consultado somente no Linux")
def test_units_sem_systemd_travado_ou_com_erro_e_none(monkeypatch, tmp_path):
    _systemd(monkeypatch, tmp_path, exc=subprocess.TimeoutExpired("systemctl", 3))
    before = time.monotonic()
    assert orq_conductor.units() is None
    assert 2.5 <= time.monotonic() - before < 4.0
    _systemd(monkeypatch, tmp_path, rc=1)
    assert orq_conductor.units() is None
    missing = tmp_path / "missing"
    missing.mkdir()
    monkeypatch.setenv("PATH", str(missing))
    assert orq_conductor.units() is None


@pytest.mark.skipif(sys.platform == "linux", reason="prova da ausência de systemd fora do Linux")
def test_units_outside_linux_is_unavailable():
    assert orq_conductor.units() is None


def _orq_cli():
    spec = importlib.util.spec_from_file_location("orq_cli", ORQ_PY)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def test_limiar_do_veto_e_o_mesmo_do_orq():
    assert orq_conductor.JEV_VETO_P == _orq_cli().VETO_P


# ── rotas ───────────────────────────────────────────────────────────────────

@pytest.fixture
def cli(monkeypatch, tmp_path):
    monkeypatch.setattr(settings, "auth_token", "secret")
    from app.api import app
    return TestClient(app)


def test_rota_do_condutor_sem_rust_recusa(cli, monkeypatch):
    from app import account_bridge
    monkeypatch.setattr(account_bridge, "owner_mode", lambda: "python")
    response = cli.get("/api/orq/2026-09-25-g1/conductor", headers=H)
    assert response.status_code == 503
    assert response.json()["detail"]["code"] == "orq_conductor_need_rust_server"


def test_rota_do_condutor_exige_autenticacao(cli):
    assert cli.get("/api/orq/x/conductor").status_code == 401


def test_condutor_valida_caminho_sem_exigir_eventos(monkeypatch, tmp_path):
    client = NativeClient(tmp_path, monkeypatch)
    _systemd(monkeypatch, tmp_path)
    directory = client.home / ".hangar/orq/execução"
    directory.mkdir(parents=True)
    answer = client.get("/api/orq/execução/conductor")
    assert answer.status_code == 200
    assert call("conductor.exec-dir", home=client.home, id="execução") == str(directory)
    assert answer.json()["watchdog"]["source"] == "none"
    assert list(directory.iterdir()) == []
    for invalid in ("a:b", "a%5Cb", "a%00b", "ausente"):
        answer = client.get(f"/api/orq/{invalid}/conductor")
        assert answer.status_code == 404
        assert answer.json()["detail"]["code"] == "erro_nao_encontrado"


def test_fato_interno_recusa_sem_segredo():
    from fastapi import FastAPI
    from app import internal_api
    app = FastAPI()
    app.include_router(internal_api.router)
    assert TestClient(app).get("/internal/orq/x/conductor-facts").status_code == 404


def test_rota_do_condutor_connect_repassa_pedido(cli, monkeypatch):
    from app import account_bridge
    from fastapi.responses import JSONResponse
    monkeypatch.setattr(account_bridge, "owner_mode", lambda: "rust")
    calls = []

    async def forward(request, target, **kwargs):
        calls.append((request.method, request.url.path, request.url.query, target))
        return JSONResponse({"fixture": True})

    monkeypatch.setattr(account_bridge, "forward_public", forward)
    answer = cli.get("/api/orq/execução/conductor?limit=3", headers=H)
    assert answer.status_code == 200 and answer.json() == {"fixture": True}
    assert calls == [("GET", "/api/orq/execução/conductor", "limit=3", "/__hangar_server/forwarded")]


def test_rota_do_condutor_recusa_convidado_antes_da_ponte(cli, monkeypatch):
    from app import account_bridge, guest_users
    monkeypatch.setattr(guest_users, "lookup_token", lambda token: object())

    async def forbidden(*args, **kwargs):
        pytest.fail("o convidado não pode chegar à ponte do condutor")

    monkeypatch.setattr(account_bridge, "forward_public", forbidden)
    assert cli.get("/api/orq/x/conductor", headers={"Authorization": "Bearer guest"}).status_code == 403
