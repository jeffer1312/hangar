"""O espelho de ferramental (`scripts/install-skills-bridge.sh`) sobre um HOME falso.

O script inteiro lê `~` via `expanduser`, então dar-lhe um HOME de mentira é o seam. O que ele
faz hoje: persona (CLAUDE.md como AGENTS.md/CLAUDE.md) e os pacotes do Pi
— e no fim chama a `backend/app/skill_bridge.py`, dona das pontes de skills do Pi e Kimi. Ele
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
    """HOME falso com um plugin habilitado e os três agentes instalados."""
    home = tmp_path / "home"
    claude = home / ".claude"
    (claude / "plugins").mkdir(parents=True)
    (home / ".codex").mkdir()  # a raiz é o que decide que o agente está instalado
    (home / ".pi" / "agent").mkdir(parents=True)
    (home / ".kimi-code").mkdir()
    (claude / "CLAUDE.md").write_text("Instruções compartilhadas\n", encoding="utf-8")
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


def test_instalador_preserva_codex_para_o_reconciliador(tmp_path):
    home = _home(tmp_path)
    codex = home / ".codex"
    existentes = {"hooks.json": '{"hooks": {}}\n', "AGENTS.md": "Instruções próprias\n",
                  ".hangar-hooks.json": '{"anterior": true}\n'}
    for nome, texto in existentes.items():
        (codex / nome).write_text(texto, encoding="utf-8")
    _rodar(home)
    assert {p.name: p.read_text(encoding="utf-8") for p in codex.iterdir()} == existentes


def test_skills_vem_da_skill_bridge_e_o_script_nao_poda(tmp_path):
    """A pasta de skills é da skill_bridge.py: o script cria a skill do plugin por ela e deixa em
    paz um link que não é de nenhuma fonte dela (o caso dos 67 apagados)."""
    home = _home(tmp_path)
    ponte = home / ".pi" / "agent" / "skills-bridge"
    ponte.mkdir(parents=True)
    (ponte / "de-terceiro").symlink_to(home / "outro-lugar" / "skill")
    _rodar(home)
    assert (ponte / "uma-skill").is_symlink()
    assert os.readlink(ponte / "uma-skill").endswith("/cache/m/p/v1/skills/uma-skill")
    assert (ponte / "de-terceiro").is_symlink()


# --- rtk no Codex: `updatedInput` exige `permissionDecision: allow` ---------------------------

FILTRO = REPO / "scripts" / "codex-hook-allow.py"


def _filtrar(entrada: str) -> str:
    r = subprocess.run(["python3", str(FILTRO)], input=entrada, capture_output=True, text=True,
                       timeout=10)
    assert r.returncode == 0, r.stderr
    return r.stdout


def test_filtro_completa_o_allow_que_o_rtk_omite_em_comando_composto():
    """rtk 0.43.0 devolve `updatedInput` sem `permissionDecision` quando reescreve so um pedaco de
    um comando com `;`. Claude Code aceita; Codex recusa a reescrita ("PreToolUse hook returned
    updatedInput without permissionDecision:allow") e roda o original."""
    saida = json.loads(_filtrar(
        '{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"pwd; rtk read x"}}}'))
    assert saida["hookSpecificOutput"]["permissionDecision"] == "allow"
    assert saida["hookSpecificOutput"]["updatedInput"] == {"command": "pwd; rtk read x"}


def test_filtro_nao_mexe_em_decisao_ja_dada_nem_em_saida_vazia():
    deny = ('{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny",'
            '"updatedInput":{"command":"x"}}}')
    assert json.loads(_filtrar(deny))["hookSpecificOutput"]["permissionDecision"] == "deny"
    assert _filtrar("") == ""
    assert _filtrar("texto que nao e json\n") == "texto que nao e json\n"


def test_persona_e_pacotes_do_pi_seguem_idempotentes(tmp_path):
    home = _home(tmp_path)
    installed = home / ".claude" / "plugins" / "installed_plugins.json"
    cfg = json.loads(installed.read_text(encoding="utf-8"))
    cfg["plugins"]["superpowers@claude-plugins-official"] = cfg["plugins"]["p@m"]
    installed.write_text(json.dumps(cfg), encoding="utf-8")
    _rodar(home)
    _rodar(home)
    persona = home / ".claude" / "CLAUDE.md"
    assert (home / ".pi/agent/CLAUDE.md").resolve() == persona
    assert (home / ".kimi-code/AGENTS.md").resolve() == persona
    assert (home / ".kimi-code/skills-bridge/uma-skill").is_symlink()
    assert (home / ".pi/agent/packages/superpowers").resolve() == Path(cfg["plugins"]["p@m"][0]["installPath"])
    assert list((home / ".codex").iterdir()) == []


def test_persona_real_do_usuario_e_preservada(tmp_path):
    home = _home(tmp_path)
    propria = home / ".kimi-code/AGENTS.md"
    propria.write_text("Minha persona", encoding="utf-8")
    saida = _rodar(home)
    assert propria.read_text(encoding="utf-8") == "Minha persona"
    assert not propria.is_symlink()
    assert "já existe como arquivo de verdade" in saida
