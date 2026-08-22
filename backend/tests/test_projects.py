import json
import os

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


def test_state_derivation(config, monkeypatch, tmp_path):
    # Caminho REAL (tmp_path) em vez de "/tmp/a" literal: o `_owns` compara o dono da porta com
    # `os.path.realpath(cwd)`, e no Windows um "/tmp/a" ganha a letra do drive so de um lado — o
    # caso media a normalizacao do os.path, nao a regra de dono. Com caminho real a regra e a mesma
    # nos dois sistemas.
    raiz = str(tmp_path / "a")
    config({"a": {"cwd": raiz, "command": "x", "port": 1234}})
    slug = runner._slug(raiz)

    def status(runs, port=(False, None)):
        monkeypatch.setattr(projects, "_port_info", lambda ports: {1234: port})
        monkeypatch.setattr(runner, "all_runs", lambda: runs)
        return projects.list_projects()[0]

    assert status({}).state == "stopped"
    assert status({slug: RunInfo(command="x", exited=True, exit_status=3)}).state == "failed"
    assert status({slug: RunInfo(command="x", exited=True, exit_status=3)}).exit_status == 3
    assert status({slug: RunInfo(command="x")}).state == "starting"
    assert status({slug: RunInfo(command="x")}, port=(True, raiz)).state == "running"
    # subpasta do projeto tambem e dele (PSS sobe de deploy/)
    assert status({slug: RunInfo(command="x")},
                  port=(True, str(tmp_path / "a" / "deploy"))).state == "running"
    # porta aberta por OUTRO projeto: pane vivo continua "starting", nunca "running" emprestado
    assert status({slug: RunInfo(command="x")},
                  port=(True, str(tmp_path / "outro"))).state == "starting"


def test_externo_exige_dono_no_cwd(config, monkeypatch, tmp_path):
    raiz = str(tmp_path / "a")     # caminho real, mesmo motivo do caso acima
    config({"a": {"cwd": raiz, "command": "x", "port": 9}})
    monkeypatch.setattr(runner, "all_runs", lambda: {})

    def with_port(port):
        monkeypatch.setattr(projects, "_port_info", lambda ports: {9: port})
        return projects.list_projects()[0].state

    assert with_port((True, raiz)) == "external"
    # porta 3000 aberta por outro front, ou dono nao identificavel: NAO atribui
    assert with_port((True, str(tmp_path / "outro" / "front"))) == "stopped"
    assert with_port((True, None)) == "stopped"
    # sem porta configurada nao ha como afirmar run externo -> stopped
    config({"a": {"cwd": raiz, "command": "x"}})
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


# --- stop_command que o cmd.exe nem chega a rodar (Windows) -------------------------------------
# O `/bin/sh` chumbado nunca rodava no Windows; agora roda pelo COMSPEC, e um stop_command com
# sintaxe POSIX (projeto vindo de uma maquina Linux) falha ali sem ninguem ver: o pane morre, a UI
# diz "parado" e o processo de verdade fica orfao. Os casos abaixo forcam `os.name == "nt"` pra
# valerem tambem no Linux — o ramo POSIX nao tem nem a chamada.

def _cp(rc: int, stderr: bytes = b""):
    import subprocess as sp
    return sp.CompletedProcess(["cmd"], rc, b"", stderr)


def test_stop_command_que_o_windows_nao_tem_vira_erro_visivel(config, monkeypatch, tmp_path):
    """Sem mock de `which`: o PATH aponta pra uma pasta vazia e o comando nao existe em lugar
    nenhum — o mesmo que o `pkill` e nesta maquina.

    De proposito nao se toca em `projects.shutil` aqui: assim o caso roda IGUAL contra o codigo
    velho, que nem importa shutil, e o que ele mede la e o defeito de verdade (stop nao levanta
    nada, a UI diz "parado", o processo fica orfao) — nao um AttributeError de simbolo novo.
    """
    raiz = str(tmp_path)
    vazio = tmp_path / "path-vazio"
    vazio.mkdir()
    monkeypatch.setenv("PATH", str(vazio))
    config({"a": {"cwd": raiz, "command": "x",
                  "stop_command": "pkill-que-nao-existe -f 'node server.js'"}})
    monkeypatch.setattr(projects.os, "name", "nt")
    monkeypatch.setattr(runner, "stop_run", lambda cwd: None)
    monkeypatch.setattr(projects.subprocess, "run",
                        lambda *a, **k: _cp(1, "nao e reconhecido".encode("cp850")))

    with pytest.raises(projects.ProjectError) as e:
        projects.stop("a")
    assert "pkill-que-nao-existe" in e.value.detail and "orfao" in e.value.detail
    assert e.value.status == 500


def test_rc_diferente_de_zero_com_o_comando_existindo_segue_calado(config, monkeypatch, tmp_path):
    """`taskkill /IM x` sem processo devolve **128** (medido) — o mesmo "nao havia o que matar" que
    faz o `pkill` devolver 1 no Linux. Acusar por rc faria toda parada de projeto ja parado virar
    erro na tela."""
    raiz = str(tmp_path)
    config({"a": {"cwd": raiz, "command": "x", "stop_command": "taskkill /F /IM node.exe"}})
    monkeypatch.setattr(projects.os, "name", "nt")
    monkeypatch.setattr(projects.shutil, "which", lambda n, path=None: r"C:\W\taskkill.exe")
    monkeypatch.setattr(runner, "stop_run", lambda cwd: None)
    monkeypatch.setattr(projects.subprocess, "run", lambda *a, **k: _cp(128, b"nao encontrado"))

    projects.stop("a")                       # nao levanta


def test_script_ao_lado_do_projeto_nao_e_acusado_de_inexistente(config, monkeypatch, tmp_path):
    """O cmd.exe procura no diretorio ATUAL antes do PATH, e o subprocess roda com cwd no projeto:
    sem o cwd na busca, um `stop.bat` do proprio projeto viraria "o Windows nao tem"."""
    (tmp_path / "stop.bat").write_text("@exit /b 1\r\n", encoding="ascii")
    vistos = {}

    def which(nome, path=None):
        vistos["path"] = path
        from pathlib import Path as P
        for raiz in (path or "").split(os.pathsep):
            if raiz and (P(raiz) / nome).is_file():
                return str(P(raiz) / nome)
        return None

    config({"a": {"cwd": str(tmp_path), "command": "x", "stop_command": "stop.bat"}})
    monkeypatch.setattr(projects.os, "name", "nt")
    monkeypatch.setattr(projects.shutil, "which", which)
    monkeypatch.setattr(runner, "stop_run", lambda cwd: None)
    monkeypatch.setattr(projects.subprocess, "run", lambda *a, **k: _cp(1))

    projects.stop("a")                       # nao levanta
    assert str(tmp_path) in vistos["path"]   # o cwd entrou na busca


def test_posix_nao_ganha_checagem_nenhuma(config, monkeypatch, tmp_path):
    """No Linux o caminho fica byte-identico: rc != 0 nem e olhado (o `pkill` devolve 1 quando ja
    nao ha o que matar, e isso sempre foi silencio aqui)."""
    config({"a": {"cwd": str(tmp_path), "command": "x", "stop_command": "pkill -f node"}})
    monkeypatch.setattr(projects.os, "name", "posix")
    monkeypatch.setattr(projects.shutil, "which",
                        lambda *a, **k: pytest.fail("POSIX nao pode consultar o PATH aqui"))
    monkeypatch.setattr(runner, "stop_run", lambda cwd: None)
    monkeypatch.setattr(projects.subprocess, "run", lambda *a, **k: _cp(1))

    projects.stop("a")                       # nao levanta


@pytest.mark.parametrize("linha, esperado", [
    ('"C:\\Program Files\\app\\stop.exe" --tudo', "C:\\Program Files\\app\\stop.exe"),
    ("taskkill /F /IM node.exe", "taskkill"),
    ("   ", ""),
])
def test_primeiro_token_respeita_aspas(linha, esperado):
    assert projects._primeiro_token(linha) == esperado


def test_gravar_com_o_arquivo_aberto_nao_diz_sem_permissao(config, monkeypatch, tmp_path):
    """A mensagem do `_write` vai INTEIRA pra tela (`HTTPException(e.status, e.detail)`), e no
    Windows um projects.json aberto por outro processo derruba o rename com "Acesso negado" —
    mandando a pessoa conferir permissao de um arquivo que ela pode escrever."""
    from app import atomico
    config({})
    monkeypatch.setattr(atomico, "_E_WINDOWS", True)
    monkeypatch.setattr(atomico, "substituir",
                        lambda o, d: (_ for _ in ()).throw(PermissionError(5, "Acesso negado")))

    with pytest.raises(projects.ProjectError) as e:
        projects.upsert("a", str(tmp_path), "npm run dev")
    assert "outro programa" in e.value.detail


def test_builtin_do_cmd_nao_e_procurado_no_path(config, monkeypatch, tmp_path):
    """`cd ... && taskkill ...` e stop_command legitimo, e `cd` nao e arquivo nenhum."""
    config({"a": {"cwd": str(tmp_path), "command": "x",
                  "stop_command": "cd . && taskkill /F /IM node.exe"}})
    monkeypatch.setattr(projects.os, "name", "nt")
    monkeypatch.setattr(projects.shutil, "which",
                        lambda *a, **k: pytest.fail("builtin nao se procura no PATH"))
    monkeypatch.setattr(runner, "stop_run", lambda cwd: None)
    monkeypatch.setattr(projects.subprocess, "run", lambda *a, **k: _cp(128))

    projects.stop("a")                       # nao levanta
