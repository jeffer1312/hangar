"""O espelho de ferramental (`scripts/install-skills-bridge.sh`) sobre um HOME falso.

O script inteiro lê `~` via `expanduser`, então dar-lhe um HOME de mentira é o seam. O que ele
faz hoje: persona (CLAUDE.md como AGENTS.md/CLAUDE.md), `hooks.json` do Codex e os pacotes do Pi
— e no fim chama a `backend/app/skill_bridge.py`, que é a ÚNICA dona das pastas de skills. Ele
próprio não cria nem apaga link de skill: com dois donos, a poda de um desfazia o outro a cada
largada do Pi (67 skills sumindo, listadas como "skill path does not exist").
"""
import json
import os
import shutil
import subprocess
from pathlib import Path

import pytest

REPO = Path(__file__).resolve().parents[2]
BRIDGE = REPO / "scripts" / "install-skills-bridge.sh"

pytestmark = [
    pytest.mark.skipif(os.name != "posix",
                       reason="script com shebang e symlinks: nao roda no Windows"),
    pytest.mark.skipif(not shutil.which("bash"), reason="precisa de bash no PATH"),
]


def _skill(raiz: Path, nome: str) -> None:
    d = raiz / "skills" / nome
    d.mkdir(parents=True, exist_ok=True)
    (d / "SKILL.md").write_text(f"---\nname: {nome}\n---\n", encoding="utf-8")


def _home(tmp_path: Path) -> Path:
    """HOME falso com um plugin habilitado no cache e o Codex 'instalado'."""
    home = tmp_path / "home"
    claude = home / ".claude"
    (claude / "plugins").mkdir(parents=True)
    (home / ".codex").mkdir()  # a raiz é o que decide que o agente está instalado
    (claude / "settings.json").write_text(
        json.dumps({"enabledPlugins": {"p@m": True}}), encoding="utf-8")
    alvo = claude / "plugins" / "cache" / "m" / "p" / "v1"
    _skill(alvo, "uma-skill")
    (claude / "plugins" / "installed_plugins.json").write_text(
        json.dumps({"plugins": {"p@m": [{"installPath": str(alvo)}]}}), encoding="utf-8")
    return home


def _rodar(home: Path) -> str:
    env = dict(os.environ, HOME=str(home))
    r = subprocess.run([str(BRIDGE)], capture_output=True, text=True, timeout=60, env=env)
    assert r.returncode == 0, r.stdout + r.stderr
    return r.stdout


def test_hook_de_estado_do_app_entra_no_hooks_json_do_codex(tmp_path):
    """O estado ao vivo da sessão Codex vem do marcador que este hook grava, e o dono do arquivo é
    o INSTALADOR: o comando carrega o caminho do venv deste checkout, então recriar venv, mover o
    repo ou subir de outra worktree mudaria o arquivo — e hook alterado é hook não aprovado, que
    não roda e não avisa. Escrevendo aqui, o arquivo só muda num momento explícito."""
    home = _home(tmp_path)
    _rodar(home)
    cfg = json.loads((home / ".codex" / "hooks.json").read_text(encoding="utf-8"))
    eventos = cfg["hooks"]
    for ev in ("SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"):
        comandos = [e["command"] for g in eventos.get(ev, []) for e in g["hooks"]]
        assert any("state_hook.py" in c for c in comandos), f"sem hook de estado em {ev}"
    # `Notification` não existe no Codex — escrevê-lo seria hook que nunca roda, que é pior que
    # hook ausente (parece ligado).
    assert "Notification" not in eventos


def test_skills_vem_da_skill_bridge_e_o_script_nao_poda(tmp_path):
    """A pasta de skills é da skill_bridge.py: o script cria a skill do plugin por ela e deixa em
    paz um link que não é de nenhuma fonte dela (o caso dos 67 apagados)."""
    home = _home(tmp_path)
    ponte = home / ".codex" / "skills"
    ponte.mkdir(parents=True)
    (ponte / "de-terceiro").symlink_to(home / "outro-lugar" / "skill")
    _rodar(home)
    assert (ponte / "uma-skill").is_symlink()
    assert os.readlink(ponte / "uma-skill").endswith("/cache/m/p/v1/skills/uma-skill")
    assert (ponte / "de-terceiro").is_symlink()
