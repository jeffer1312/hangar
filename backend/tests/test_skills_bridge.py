"""A ponte de skills (`scripts/install-skills-bridge.sh`) sobre um HOME falso.

O script inteiro lê `~` via `expanduser`, então dar-lhe um HOME de mentira é o seam: dá pra montar
dois `installPath` do MESMO plugin — um sob o home principal, outro sob uma conta secundária — e
conferir para onde o symlink aponta, sem tocar na instalação de verdade da máquina.
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


def _home(tmp_path: Path, install_paths: list[str]) -> Path:
    """HOME falso com um plugin habilitado instalado em cada caminho de `install_paths`."""
    home = tmp_path / "home"
    claude = home / ".claude"
    (claude / "plugins").mkdir(parents=True)
    (home / ".codex").mkdir()  # a raiz é o que decide que o agente está instalado
    (claude / "settings.json").write_text(
        json.dumps({"enabledPlugins": {"p@m": True}}), encoding="utf-8")
    entradas = []
    for rel in install_paths:
        alvo = home / rel
        _skill(alvo, "uma-skill")
        entradas.append({"installPath": str(alvo)})
    (claude / "plugins" / "installed_plugins.json").write_text(
        json.dumps({"plugins": {"p@m": entradas}}), encoding="utf-8")
    return home


def _rodar(home: Path) -> str:
    env = dict(os.environ, HOME=str(home))
    r = subprocess.run([str(BRIDGE)], capture_output=True, text=True, timeout=60, env=env)
    assert r.returncode == 0, r.stdout + r.stderr
    return os.readlink(home / ".codex" / "skills" / "uma-skill")


PRINCIPAL = ".claude/plugins/cache/m/p/v1"
SECUNDARIA = ".claude-outra/plugins/cache/m/p/v1"


@pytest.mark.parametrize("ordem", [
    [PRINCIPAL, SECUNDARIA],   # a secundária vem por último: era ela que ganhava
    [SECUNDARIA, PRINCIPAL],
])
def test_desempate_prefere_o_home_principal(tmp_path, ordem):
    home = _home(tmp_path, ordem)
    assert _rodar(home) == str(home / PRINCIPAL / "skills" / "uma-skill")


def test_hook_de_estado_do_app_entra_no_hooks_json_do_codex(tmp_path):
    """O estado ao vivo da sessão Codex vem do marcador que este hook grava, e o dono do arquivo é
    o INSTALADOR: o comando carrega o caminho do venv deste checkout, então recriar venv, mover o
    repo ou subir de outra worktree mudaria o arquivo — e hook alterado é hook não aprovado, que
    não roda e não avisa. Escrevendo aqui, o arquivo só muda num momento explícito."""
    home = _home(tmp_path, [PRINCIPAL])
    _rodar(home)
    cfg = json.loads((home / ".codex" / "hooks.json").read_text(encoding="utf-8"))
    eventos = cfg["hooks"]
    for ev in ("SessionStart", "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"):
        comandos = [e["command"] for g in eventos.get(ev, []) for e in g["hooks"]]
        assert any("state_hook.py" in c for c in comandos), f"sem hook de estado em {ev}"
    # `Notification` não existe no Codex — escrevê-lo seria hook que nunca roda, que é pior que
    # hook ausente (parece ligado).
    assert "Notification" not in eventos


def test_poda_link_antigo_para_conta_secundaria(tmp_path):
    """Link que a ponte fez para o cache de uma conta secundária e que não volta ao `wanted` (o
    agente já varre aquela skill sozinho) tem que sair — parado ele aponta pra um cache que a
    outra conta pode limpar."""
    home = _home(tmp_path, [PRINCIPAL])
    ponte = home / ".codex" / "skills"
    ponte.mkdir(parents=True)
    (ponte / "velha").symlink_to(home / SECUNDARIA / "skills" / "uma-skill")
    (ponte / "de-terceiro").symlink_to(home / "outro-lugar" / "skill")
    _rodar(home)
    assert not (ponte / "velha").is_symlink()
    assert (ponte / "de-terceiro").is_symlink()


@pytest.mark.parametrize("empatadas", [
    [SECUNDARIA, ".claude-terceira/plugins/cache/m/p/v1"],   # nenhuma sob o principal
    [PRINCIPAL, ".claude/plugins/cache/m/p/v2"],             # as duas sob o principal
])
def test_empate_segue_na_ultima_entrada(tmp_path, empatadas):
    home = _home(tmp_path, empatadas)
    assert _rodar(home) == str(home / empatadas[-1] / "skills" / "uma-skill")
