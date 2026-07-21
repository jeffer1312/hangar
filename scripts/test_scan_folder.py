"""Testes de _detect_command (scan-folder): heurística de script + gerenciador por lockfile."""
import importlib.machinery
import importlib.util
import json
from pathlib import Path

# mesmo carregamento de test_import_candidates.py: cp-panel-action não tem extensão .py.
_path = Path(__file__).with_name("cp-panel-action")
spec = importlib.util.spec_from_file_location(
    "cp_panel_action", _path, loader=importlib.machinery.SourceFileLoader("cp_panel_action", str(_path)))
cpa = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cpa)


def _write_pkg(tmp_path, scripts=None, package_manager=None):
    pkg = {}
    if scripts is not None:
        pkg["scripts"] = scripts
    if package_manager is not None:
        pkg["packageManager"] = package_manager
    (tmp_path / "package.json").write_text(json.dumps(pkg))


def test_sem_package_json(tmp_path):
    assert cpa._detect_command(str(tmp_path)) == ""


def test_dev_com_pnpm_lock(tmp_path):
    _write_pkg(tmp_path, scripts={"dev": "vite"})
    (tmp_path / "pnpm-lock.yaml").touch()
    assert cpa._detect_command(str(tmp_path)) == "pnpm dev"


def test_dev_com_package_lock(tmp_path):
    _write_pkg(tmp_path, scripts={"dev": "vite"})
    (tmp_path / "package-lock.json").touch()
    assert cpa._detect_command(str(tmp_path)) == "npm run dev"


def test_dev_com_yarn_lock(tmp_path):
    _write_pkg(tmp_path, scripts={"dev": "vite"})
    (tmp_path / "yarn.lock").touch()
    assert cpa._detect_command(str(tmp_path)) == "yarn dev"


def test_start_sem_lock_sem_package_manager(tmp_path):
    _write_pkg(tmp_path, scripts={"start": "node index.js"})
    assert cpa._detect_command(str(tmp_path)) == "npm run start"


def test_sem_scripts(tmp_path):
    _write_pkg(tmp_path, scripts={})
    assert cpa._detect_command(str(tmp_path)) == ""
