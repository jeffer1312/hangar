"""Casamento de pastas do import: basename sob CP_SCAN_ROOTS, realpath, colisão não chuta."""
import importlib.machinery
import importlib.util
import os
from pathlib import Path

# cp-panel-action não tem extensão .py: spec_from_file_location não acha loader por sufixo e
# devolve None (verificado, Python 3.14) — precisa do SourceFileLoader explícito.
_path = Path(__file__).with_name("cp-panel-action")
spec = importlib.util.spec_from_file_location(
    "cp_panel_action", _path, loader=importlib.machinery.SourceFileLoader("cp_panel_action", str(_path)))
cpa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cpa)


def test_match_unico(tmp_path):
    (tmp_path / "app-web").mkdir()
    local, matched = cpa._match_local("/home/outro/app-web", [str(tmp_path)])
    assert matched and local == str(tmp_path / "app-web")


def test_match_ausente(tmp_path):
    local, matched = cpa._match_local("/home/outro/naoexiste", [str(tmp_path)])
    assert not matched


def test_match_colisao_nao_chuta(tmp_path):
    r1, r2 = tmp_path / "a", tmp_path / "b"
    (r1 / "front").mkdir(parents=True)
    (r2 / "front").mkdir(parents=True)
    _, matched = cpa._match_local("/x/front", [str(r1), str(r2)])
    assert not matched      # dois candidatos -> não marca


def test_cmd_com_abspath_morto():
    assert cpa._cmd_has_dead_abspath("bash /home/nada/x/launcher.sh")
    assert not cpa._cmd_has_dead_abspath("pnpm dev")


def test_scan_roots_executa():
    # _scan_roots usa env() de cp_panel_common — se o import faltar, dá NameError. Este teste
    # exercita o caminho inteiro (sem ele os 4 acima nunca chamam _scan_roots e o bug passa).
    roots = cpa._scan_roots()
    assert isinstance(roots, list)
