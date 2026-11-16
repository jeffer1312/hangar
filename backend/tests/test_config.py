from pathlib import Path

import pytest
from app.config import (
    _default_projects_dir,
    detect_lan_ip,
    resolve_bind_ip,
    Settings,
)


def test_default_projects_dir_honors_claude_config_dir(monkeypatch):
    """The transcript dir must follow $CLAUDE_CONFIG_DIR, not a hardcoded ~/.claude —
    machines/users set CLAUDE_CONFIG_DIR to different locations."""
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", "/tmp/some-custom-config")
    assert _default_projects_dir() == Path("/tmp/some-custom-config/projects")


def test_default_projects_dir_falls_back_to_home(monkeypatch):
    monkeypatch.delenv("CLAUDE_CONFIG_DIR", raising=False)
    assert _default_projects_dir() == Path.home() / ".claude" / "projects"


def test_detect_lan_ip_returns_ipv4():
    ip = detect_lan_ip()
    assert isinstance(ip, str) and ip.count(".") == 3


def test_resolve_bind_ip_passthrough_when_not_auto():
    assert resolve_bind_ip(Settings(lan_bind_ip="192.168.1.50")) == "192.168.1.50"


@pytest.mark.parametrize("valor", ["-x", "a..b", "com espaco", "x;rm", "a" * 101])
def test_update_branch_invalida_vira_main_e_vai_pro_diario(monkeypatch, valor):
    from app import diag
    eventos = []
    monkeypatch.setattr(diag, "registrar", lambda evento, nivel="ok", **c: eventos.append((evento, nivel)))
    assert Settings(update_branch=valor).update_branch == ""
    assert eventos == [("atualizacao.branch_invalida", "aviso")]


def test_update_branch_valida_vem_do_ambiente(monkeypatch):
    monkeypatch.setenv("CP_UPDATE_BRANCH", " feat/hangar-server_1.2 ")
    assert Settings().update_branch == "feat/hangar-server_1.2"
    monkeypatch.setenv("CP_UPDATE_BRANCH", "")
    assert Settings().update_branch == ""


def test_invalid_update_last_branch_reports_correct_variable(monkeypatch, caplog):
    from app import diag
    events = []
    monkeypatch.setattr(diag, "registrar", lambda event, level="ok", **fields: events.append(fields))
    result = Settings(update_branch="test/channel", update_last_branch="bad..branch", _env_file=None)
    assert result.update_branch == "test/channel"
    assert result.update_last_branch == ""
    assert "CP_UPDATE_LAST_BRANCH" in caplog.text
    assert events[-1]["detalhe"] == "CP_UPDATE_LAST_BRANCH ignorado"


def test_settings_read_the_env_from_the_user_data_folder(monkeypatch, tmp_path):
    """Máquina migrada: o `.env` está só em `~/.hangar/config/` e o token sai de lá."""
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.delenv("CP_AUTH_TOKEN", raising=False)
    config = tmp_path / ".hangar" / "config"
    config.mkdir(parents=True)
    (config / ".env").write_text("CP_AUTH_TOKEN=do-lugar-novo\n", encoding="utf-8")
    # Comparado fora do assert: na falha o pytest imprimiria o token lido, que pode ser o real.
    veio_do_lugar_novo = Settings().auth_token == "do-lugar-novo"
    assert veio_do_lugar_novo, "o token não veio de ~/.hangar/config/.env"


def test_settings_keep_reading_backend_env_on_an_unmigrated_machine(monkeypatch, tmp_path):
    monkeypatch.setenv("HOME", str(tmp_path))
    from app import user_data
    assert user_data.path(".env") == Path(__file__).resolve().parent.parent / ".env"
    assert not (tmp_path / ".hangar").exists()
