"""Raiz da instalação: com `~/.hangar/current` (pacote), o que se grava fora do Hangar aponta para o
`current` sem resolver o link; sem ele, tudo fica como no checkout."""
import importlib
import json
import os
import sys
from pathlib import Path

import pytest

from app import install_root

CHECKOUT = Path(install_root.__file__).resolve().parents[2]
PY = Path("python/python.exe") if os.name == "nt" else Path("python/bin/python3")


def _pacote(home: Path) -> Path:
    """`~/.hangar/current -> versions/X`, com o Python embutido."""
    real = home / ".hangar" / "versions" / "X"
    (real / PY).parent.mkdir(parents=True)
    (real / PY).write_text("", encoding="utf-8")
    (real / "backend" / "hooks").mkdir(parents=True)
    current = home / ".hangar" / "current"
    current.symlink_to(real, target_is_directory=True)
    return current


@pytest.fixture
def home(tmp_path, monkeypatch):
    h = tmp_path / "home"
    h.mkdir()
    monkeypatch.setenv("HOME", str(h))
    monkeypatch.setenv("USERPROFILE", str(h))
    return h


def test_sem_current_a_raiz_e_o_checkout_e_o_python_o_de_hoje(home):
    assert install_root.root() == CHECKOUT
    assert install_root.interpreter() == sys.executable
    venv = Path("backend/.venv/Scripts/python.exe") if os.name == "nt" else Path("backend/.venv/bin/python")
    assert install_root.python_exe(home) == home / venv


def test_com_current_a_raiz_e_o_link_sem_resolver(home):
    current = _pacote(home)
    assert install_root.root() == current
    assert install_root.python_exe(current) == current / PY
    assert install_root.interpreter() == str(current / PY)


def test_hook_gravado_aponta_para_o_current(home, tmp_path, monkeypatch):
    # O hook_installer monta os comandos na importação: recarrega com o HOME do teste e devolve ao fim.
    from app import hook_installer
    current = _pacote(home)
    try:
        hi = importlib.reload(hook_installer)
        settings = tmp_path / "settings.json"
        assert hi._ensure_event_hook(settings, "Stop", hi._STATE_COMMAND, por_nome=True)
        comando = json.loads(settings.read_text(encoding="utf-8"))["hooks"]["Stop"][0]["hooks"][0]["command"]
        assert comando.startswith(f'"{current / PY}" "{current / "backend" / "hooks" / "state_hook.py"}"')
        assert "versions" not in comando
    finally:
        monkeypatch.undo()
        importlib.reload(hook_installer)


@pytest.mark.parametrize("modulo, atributo, relativo", [
    ("app.skill_bridge", "_REPO", ""),
    ("app.plugin_bridge", "PLUGINS_ROOT", "plugins"),
    ("app.omp_context", "RULE_TEMPLATE", "scripts/omp/carregar-claude-md-do-projeto.md"),
    ("app.harness_saude", "_REPO", ""),
    ("app.codex_integracao", "_REPO", ""),
    ("app.codex_contas_plugins", "_REPO", ""),
    ("app.codex_compat", "_REPO", ""),
])
def test_caminho_gravado_fora_do_hangar_aponta_para_o_current(home, monkeypatch, modulo, atributo, relativo):
    mod = importlib.import_module(modulo)
    current = _pacote(home)
    try:
        assert getattr(importlib.reload(mod), atributo) == current / relativo
    finally:
        monkeypatch.undo()
        importlib.reload(mod)


def test_python_gravado_nos_hooks_do_codex_e_o_do_pacote(home):
    from app import codex_integracao
    current = _pacote(home)
    data = {"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "rtk hook claude"}]}]}}
    texto = json.dumps(codex_integracao.IntegracaoCodex._normalizar(None, data))
    assert json.dumps(str(current / PY))[1:-1] in texto, texto


def test_binario_do_pacote_vence(home, monkeypatch):
    from app import rust_bins
    current = _pacote(home)
    monkeypatch.delenv("CP_RUST_CANO_BIN", raising=False)
    exe = current / "bin" / ("hangar-cano" + (".exe" if os.name == "nt" else ""))
    exe.parent.mkdir()
    exe.write_text("#!/bin/sh\n", encoding="utf-8")
    exe.chmod(0o755)
    # Pelo caminho real da versão: processo longo não pode seguir o `current` que a troca move.
    assert rust_bins.find_bin("hangar-cano", "CP_RUST_CANO_BIN") == exe.resolve()


def test_marcador_hangar_reconhece_current_e_versao(home):
    from app.config_sync_paths import Roots, canonicalize
    current = _pacote(home)
    roots = Roots.this_machine()
    assert roots.hangar == str(current)
    real = home / ".hangar" / "versions" / "X"
    texto = f'"{current}/backend/hooks/a.py" {real}/scripts/b.py {home}/x'
    assert canonicalize(texto, roots) == '"⟦HANGAR⟧/backend/hooks/a.py" ⟦HANGAR⟧/scripts/b.py ⟦HOME⟧/x'
