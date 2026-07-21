import json

import pytest

from app import projects, runner
from app.models import RunInfo


@pytest.fixture
def config(tmp_path, monkeypatch):
    # Patch INCONDICIONAL do caminho: teste que nem chama write() (ex.: config inválido
    # escrevendo direto em projects._CONFIG) não pode alcançar o projects.json real.
    path = tmp_path / "projects.json"
    monkeypatch.setattr(projects, "_CONFIG", path)

    def write(data):
        path.write_text(json.dumps(data), encoding="utf-8")
    return write


def test_state_derivation(config, monkeypatch):
    config({"a": {"cwd": "/tmp/a", "command": "x", "port": 1234}})
    slug = runner._slug("/tmp/a")

    def status(runs, port=(False, None)):
        monkeypatch.setattr(projects, "_port_info", lambda ports: {1234: port})
        monkeypatch.setattr(runner, "all_runs", lambda: runs)
        return projects.list_projects()[0]

    assert status({}).state == "stopped"
    assert status({slug: RunInfo(command="x", exited=True, exit_status=3)}).state == "failed"
    assert status({slug: RunInfo(command="x", exited=True, exit_status=3)}).exit_status == 3
    assert status({slug: RunInfo(command="x")}).state == "starting"
    assert status({slug: RunInfo(command="x")}, port=(True, "/tmp/a")).state == "running"
    # subpasta do projeto tambem e dele (PSS sobe de deploy/)
    assert status({slug: RunInfo(command="x")}, port=(True, "/tmp/a/deploy")).state == "running"
    # porta aberta por OUTRO projeto: pane vivo continua "starting", nunca "running" emprestado
    assert status({slug: RunInfo(command="x")}, port=(True, "/outro")).state == "starting"


def test_externo_exige_dono_no_cwd(config, monkeypatch):
    config({"a": {"cwd": "/tmp/a", "command": "x", "port": 9}})
    monkeypatch.setattr(runner, "all_runs", lambda: {})

    def with_port(port):
        monkeypatch.setattr(projects, "_port_info", lambda ports: {9: port})
        return projects.list_projects()[0].state

    assert with_port((True, "/tmp/a")) == "external"
    # porta 3000 aberta por outro front, ou dono nao identificavel: NAO atribui
    assert with_port((True, "/outro/front")) == "stopped"
    assert with_port((True, None)) == "stopped"
    # sem porta configurada nao ha como afirmar run externo -> stopped
    config({"a": {"cwd": "/tmp/a", "command": "x"}})
    assert projects.list_projects()[0].state == "stopped"


def test_sem_porta_vivo_e_running(config, monkeypatch):
    config({"a": {"cwd": "/tmp/a", "command": "x"}})
    monkeypatch.setattr(runner, "all_runs", lambda: {runner._slug("/tmp/a"): RunInfo(command="x")})
    assert projects.list_projects()[0].state == "running"


def test_config_invalido_e_erro_visivel(config):
    projects._CONFIG.write_text("{quebrado", encoding="utf-8")
    with pytest.raises(projects.ProjectError):
        projects.list_projects()


def test_projeto_desconhecido_404(config):
    config({})
    with pytest.raises(projects.ProjectError) as e:
        projects.start("nao-existe")
    assert e.value.status == 404


def test_stop_command_falho_mata_pane_e_sobe_erro(config, monkeypatch):
    config({"a": {"cwd": "/tmp/a", "command": "x", "stop_command": "y"}})
    killed = []
    monkeypatch.setattr(runner, "stop_run", lambda cwd: killed.append(cwd))

    def boom(*a, **k):
        raise OSError("sem shell")
    monkeypatch.setattr(projects.subprocess, "run", boom)

    with pytest.raises(projects.ProjectError):
        projects.stop("a")
    assert killed == ["/tmp/a"]  # pane morre MESMO com stop_command quebrado
