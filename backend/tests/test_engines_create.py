"""Criar/retomar sessão com motor.

O motor é aplicado prefixando o comando com `cp-engine --exec`, não com `tmux -e`: assim a key não
aparece em /proc/<pid>/cmdline (legível por qualquer usuário) e o tmux.py não muda.
E o motor tem que sobreviver aos DOIS resumes — senão uma sessão Kimi ressuscita na conta Anthropic
continuando um transcript de Kimi, calado.
"""
import pytest

from app import engines as eng
from app import registry as reg


@pytest.fixture(autouse=True)
def _isola(tmp_path, monkeypatch):
    monkeypatch.setattr(eng, "caminho", lambda: tmp_path / "engines.json")
    yield


def _motor():
    eng.salvar("kimi", {"base_url": "https://api.kimi.com/coding",
                        "api_key": "sk-kimi-1234", "model": "k3"})


def _reg(tmp_path, monkeypatch, visto):
    def _fake_new(name, cwd, command, config_dir=None):
        visto["command"] = command
        return True

    monkeypatch.setattr(reg.tmux, "new_session", _fake_new)
    monkeypatch.setattr(reg.tmux, "has_session", lambda n: False)
    monkeypatch.setattr(reg, "_pretrust_cwd", lambda cwd, cfg: None)
    return reg.SessionRegistry(projects_dir=tmp_path)


def test_create_com_motor_prefixa_o_comando(tmp_path, monkeypatch):
    _motor()
    visto = {}
    info = _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path), engine="kimi")
    assert visto["command"].startswith("cp-engine --exec kimi -- claude --session-id ")
    assert info.engine == "kimi"


def test_create_com_motor_nao_poe_a_key_no_comando(tmp_path, monkeypatch):
    _motor()
    visto = {}
    _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path), engine="kimi")
    assert "sk-kimi" not in visto["command"]


def test_create_sem_motor_nao_muda_o_comando(tmp_path, monkeypatch):
    visto = {}
    info = _reg(tmp_path, monkeypatch, visto).create("s", str(tmp_path))
    assert visto["command"].startswith("claude --session-id ")
    assert info.engine is None


def test_create_com_motor_inexistente_estoura(tmp_path, monkeypatch):
    visto = {}
    r = _reg(tmp_path, monkeypatch, visto)
    with pytest.raises(ValueError, match="motor"):
        r.create("s", str(tmp_path), engine="fantasma")
    assert "command" not in visto


def test_engine_of_le_o_cp_engine_do_proc(tmp_path, monkeypatch):
    # Mesmo truque do _config_dir_of: o env do processo VIVO é o registro autoritativo — um sidecar
    # em disco pode divergir do que está de fato rodando no pane.
    environ = tmp_path / "environ"
    environ.write_bytes(b"PATH=/usr/bin\x00CP_ENGINE=kimi\x00HOME=/home/x\x00")
    monkeypatch.setattr(reg, "_proc_environ_path", lambda pid: str(environ))
    assert reg._engine_of(1234) == "kimi"


def test_engine_of_sem_a_marca_e_none(tmp_path, monkeypatch):
    environ = tmp_path / "environ"
    environ.write_bytes(b"PATH=/usr/bin\x00")
    monkeypatch.setattr(reg, "_proc_environ_path", lambda pid: str(environ))
    assert reg._engine_of(1234) is None


def _prep_resume(tmp_path, monkeypatch, visto, motor):
    sid = "11111111-2222-3333-4444-555555555555"
    proj = tmp_path / "projects" / "-tmp"
    proj.mkdir(parents=True)
    (proj / f"{sid}.jsonl").write_text("", encoding="utf-8")

    def _fake_new(name, cwd, command, config_dir=None):
        visto["command"] = command
        return True

    monkeypatch.setattr(reg, "_engine_of", lambda pid: motor)
    monkeypatch.setattr(reg, "_config_dir_of", lambda pid: None)
    monkeypatch.setattr(reg, "sanitize_cwd", lambda cwd: "-tmp")
    monkeypatch.setattr(reg.tmux, "kill_session", lambda n: None)
    monkeypatch.setattr(reg.tmux, "new_session", _fake_new)
    r = reg.SessionRegistry(projects_dir=tmp_path / "projects")
    monkeypatch.setattr(r, "_pane_of", lambda name: {"cwd": "/tmp", "pid": 4242})
    monkeypatch.setattr(r, "_forget", lambda name: None)
    return r, sid


def test_resume_de_pane_vivo_preserva_o_motor(tmp_path, monkeypatch):
    _motor()
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "kimi")
    info = r.resume("s", sid)
    assert visto["command"] == f"cp-engine --exec kimi -- claude --resume {sid}"
    assert info.engine == "kimi"


def test_resume_de_motor_removido_nao_trava_a_sessao(tmp_path, monkeypatch):
    # Motor apagado no app depois da sessão nascer: melhor ressuscitar na conta Anthropic (e o badge
    # mostrar isso) do que recusar o resume e deixar a sessão inacessível.
    visto = {}
    r, sid = _prep_resume(tmp_path, monkeypatch, visto, "sumiu")
    info = r.resume("s", sid)
    assert visto["command"] == f"claude --resume {sid}"
    assert info.engine is None


def test_resume_do_arquivo_aceita_motor(tmp_path, monkeypatch):
    # api.py:1942 usa create(resume_session_id=...): o pane morreu, não há /proc para ler, então o
    # motor vem do cliente. Sem isto, retomar do Arquivo troca de motor calado.
    _motor()
    visto = {}
    r = _reg(tmp_path, monkeypatch, visto)
    sid = "11111111-2222-3333-4444-555555555555"
    info = r.create("s", str(tmp_path), resume_session_id=sid, engine="kimi")
    assert visto["command"] == f"cp-engine --exec kimi -- claude --resume {sid}"
    assert info.engine == "kimi"
