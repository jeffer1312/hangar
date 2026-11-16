"""Sidecar das sessões Codex e a confiança da pasta no config do Codex."""
import os

import pytest
from unittest.mock import patch

from app.adapters.codex import sessions as codex_sessions


@pytest.fixture(autouse=True)
def _isolate(tmp_path):
    sdir = tmp_path / "codex-sessions"
    with patch.object(codex_sessions, "_dir", lambda: sdir):
        yield


def test_sidecar_preserva_codex_home_em_update(tmp_path):
    codex_sessions.save("one", "thread-a", "/tmp/rollout-a.jsonl", "/tmp/project",
                        codex_home="/tmp/codex-work")
    codex_sessions.update_model("one", "model-a", "high")
    assert codex_sessions.load("one")["codex_home"] == "/tmp/codex-work"


def _config_codex(tmp_path, conteudo):
    casa = tmp_path / "casa"
    (casa / ".codex").mkdir(parents=True)
    cfg = casa / ".codex" / "config.toml"
    cfg.write_text(conteudo, encoding="utf-8")
    return casa, cfg


def test_pretrust_escreve_antes_do_bloco_gerenciado(tmp_path):
    """A entrada nao pode cair no fim do arquivo: o fim, hoje, esta DENTRO do bloco que a ponte de
    skills reescreve inteiro (o Codex apenda a confianca dos hooks la)."""
    casa, cfg = _config_codex(tmp_path, 'model = "x"\n\n# >>> hangar: provedor\nfoo = 1\n')
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/pasta-nova")
    texto = cfg.read_text()
    assert texto.index('[projects."/tmp/pasta-nova"]') < texto.index("# >>> hangar:")
    assert texto.endswith("foo = 1\n")     # o bloco de terceiro segue intacto no fim


@pytest.mark.skipif(os.name != "posix", reason="modo de arquivo POSIX")
def test_pretrust_preserva_o_modo_do_config(tmp_path):
    """O config do Codex guarda credencial de provedor. Um pretrust nao pode afrouxar o arquivo
    pro umask so por ter passado por um temporario."""
    casa, cfg = _config_codex(tmp_path, 'model = "x"\n')
    cfg.chmod(0o600)
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/pasta-nova")
    assert cfg.stat().st_mode & 0o777 == 0o600


def test_pretrust_e_idempotente(tmp_path):
    casa, cfg = _config_codex(tmp_path, '[projects."/tmp/ja"]\ntrust_level = "trusted"\n')
    antes = cfg.read_text()
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/ja")
    assert cfg.read_text() == antes


def test_pretrust_nao_redefine_tabela_escrita_de_outra_forma(tmp_path):
    """Uma checagem por regex de `[projects."<alvo>"]` nao veria esta forma, e apendar a nossa
    REDEFINIRIA a tabela: o config pararia de abrir, pro Codex e pra ponte."""
    casa, cfg = _config_codex(tmp_path, "[projects]\n'/tmp/ja' = { trust_level = 'trusted' }\n")
    antes = cfg.read_text()
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/ja")
    assert cfg.read_text() == antes


def test_pretrust_nao_derruba_a_criacao_com_config_quebrado(tmp_path):
    casa, cfg = _config_codex(tmp_path, "isto ] nao [ e toml\n")
    with patch.object(codex_sessions.Path, "home", staticmethod(lambda: casa)):
        codex_sessions.pretrust_cwd("/tmp/pasta-nova")   # best-effort: nao levanta
    assert cfg.read_text() == "isto ] nao [ e toml\n"    # e nao corrompe mais ainda


@pytest.mark.parametrize("headless", [False, True])
def test_sidecar_save_keeps_transfer_budget_and_key_on_same_thread(tmp_path, headless):
    codex_sessions.save("s", "t", "/tmp/rollout", "/tmp/project", key="original", headless=headless,
                        transfer_id="transfer-1", tool_output_token_limit=144000, codex_home=tmp_path / "account")
    codex_sessions.update_model("s", "model", "high")
    codex_sessions.update_app_pid("s", 123)
    codex_sessions.save("s", "t", "/tmp/rollout", "/tmp/project", headless=headless)
    meta = codex_sessions.load("s")
    assert meta["key"] == "original"
    assert meta["transfer_id"] == "transfer-1"
    assert meta["tool_output_token_limit"] == 144000
    assert meta["codex_home"] == str(tmp_path / "account")


@pytest.mark.parametrize("tier", ["priority", "default"])
@pytest.mark.parametrize("headless", [False, True])
def test_service_tier_survives_same_thread_save(tier, headless):
    codex_sessions.save("s", "t", "/rollout", "/project", service_tier=tier, headless=headless)
    codex_sessions.update_model("s", "model", "high")
    codex_sessions.save("s", "t", "/rollout", "/project", headless=headless)
    assert codex_sessions.load("s")["service_tier"] == tier
    codex_sessions.save("s", "new", "/new-rollout", "/project")
    assert codex_sessions.load("s").get("service_tier") is None


def test_service_tier_old_thread_cannot_update_new_sidecar():
    codex_sessions.save("s", "old", "/rollout", "/project", service_tier="priority")
    codex_sessions.update("s", thread_id="new")
    assert codex_sessions.load("s").get("service_tier") is None
    assert not codex_sessions.update_service_tier("s", "old", "priority")
    assert codex_sessions.load("s").get("service_tier") is None
    assert codex_sessions.update_service_tier("s", "new", "default")
    assert codex_sessions.load("s")["service_tier"] == "default"


@pytest.mark.parametrize("mutation", ["save", "update", "switch_thread"])
def test_new_thread_removes_active_import_association(tmp_path, mutation):
    codex_sessions.save("s", "t", "/tmp/rollout", "/tmp/project", key="original",
                        transfer_id="transfer-1", tool_output_token_limit=144000,
                        endpoint="ws://127.0.0.1:9", app_pid=123)
    if mutation == "save":
        codex_sessions.save("s", "new", "/tmp/new-rollout", "/tmp/project")
    elif mutation == "update":
        codex_sessions.update("s", thread_id="new", rollout_path="/tmp/new-rollout")
    else:
        assert codex_sessions.switch_thread("s", {"id": "new", "path": "/tmp/new-rollout",
                                                   "cwd": "/tmp/project", "source": "cli"},
                                           "t", endpoint="ws://127.0.0.1:9", app_pid=123)
    meta = codex_sessions.load("s")
    assert meta["key"] == "original" and meta["thread_id"] == "new"
    assert "transfer_id" not in meta and "tool_output_token_limit" not in meta
