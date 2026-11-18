"""Contas Claude do lado Python: nomes e listagem.

A reconciliação e a criação são do Rust (`crates/hangar-server/src/accounts/claude_config.rs`); o
comportamento delas é provado lá (`tests/it/accounts_reconcile.rs`) contra fixtures gravadas do
Python de origem.
"""
import json
import sys
from pathlib import Path

import pytest

from app import contas


@pytest.fixture
def casa(tmp_path, monkeypatch):
    monkeypatch.setenv("HOME", str(tmp_path))
    (tmp_path / ".claude" / "skills").mkdir(parents=True)
    (tmp_path / ".claude" / "settings.json").write_text('{"theme":"dark"}', encoding="utf-8")
    (tmp_path / ".claude.json").write_text(json.dumps({"oauthAccount": {"x": 1}, "mcpServers": {}}), encoding="utf-8")
    return tmp_path


def _carimbada(casa: Path, nome: str) -> Path:
    conta = casa / f".claude-{nome}"
    conta.mkdir()
    (conta / contas.MARCADOR).write_text("", encoding="utf-8")
    return conta


def test_nome_fora_da_regra_e_recusado_antes_do_disco(casa):
    """fullmatch e não match+$: `conta2\\n` passava e a pasta nascia com quebra de linha no nome."""
    for mal in ("Conta 2", "conta2\n", "conta/2", "..", "x" * 33, "-abc", "_abc", ""):
        with pytest.raises(contas.ContaError) as e:
            contas.caminho(mal)
        assert e.value.status == 400
    assert contas.caminho("x" * 32) == casa / f".claude-{'x' * 32}"


def test_listar_devolve_so_conta_carimbada_e_real(casa):
    """Sem as guardas, um `~/.claude-evil -> fora` com marcador seria listado, e uma pasta ainda
    sendo semeada apareceria no seletor."""
    _carimbada(casa, "b")
    _carimbada(casa, "a")
    (casa / ".claude-backup").mkdir()
    fora = casa / "fora"
    fora.mkdir()
    (fora / contas.MARCADOR).write_text("", encoding="utf-8")
    marcador_link = casa / ".claude-marcador-link"
    marcador_link.mkdir()
    try:
        (casa / ".claude-evil").symlink_to(fora, target_is_directory=True)
        (marcador_link / contas.MARCADOR).symlink_to(fora / contas.MARCADOR)
    except OSError as exc:
        if sys.platform != "win32":
            raise
        pytest.skip(f"Symlink indisponível nesta máquina: {exc}")
    (_carimbada(casa, "pendente") / ".hangar-account-pending").write_text("", encoding="utf-8")
    assert contas.listar() == ["a", "b"]
