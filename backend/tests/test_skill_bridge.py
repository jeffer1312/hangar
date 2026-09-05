"""Testes da ponte de skills — o contrato é o efeito no disco, não a varredura."""

from __future__ import annotations

import json
import os
from pathlib import Path

from app import skill_bridge


def _skill(raiz: Path, *partes: str) -> Path:
    d = raiz.joinpath(*partes)
    d.mkdir(parents=True)
    (d / "SKILL.md").write_text("---\nname: x\n---\n", encoding="utf-8")
    return d


def _home(tmp_path: Path, monkeypatch) -> Path:
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setattr(skill_bridge, "_REPO", tmp_path / "repo")
    return home


def _alvos(ponte: Path) -> dict[str, str]:
    return {p.name: os.readlink(p) for p in sorted(ponte.iterdir()) if p.is_symlink()}


def test_materializa_fontes_com_precedencia(tmp_path, monkeypatch):
    home = _home(tmp_path, monkeypatch)
    claude = _skill(home, ".claude", "skills", "dup")
    _skill(home, ".claude", "plugins", "cache", "ecc", "ecc", "2.2.0", "skills", "dup")
    _skill(home, ".claude", "plugins", "cache", "ecc", "ecc", "2.2.0", "skills", "do-cache")
    _skill(home, ".agents", "skills", "dos-agents")
    (home / ".pi" / "agent").mkdir(parents=True)

    skill_bridge.rebuild(home, log=lambda *a: None)

    alvos = _alvos(home / ".pi" / "agent" / "skills-bridge")
    # usuário > cache > agents; cada nome uma vez
    assert alvos == {"dup": str(claude),
                     "do-cache": str(home / ".claude/plugins/cache/ecc/ecc/2.2.0/skills/do-cache"),
                     "dos-agents": str(home / ".agents/skills/dos-agents")}


def test_cache_usa_so_versao_mais_nova_do_plugin(tmp_path, monkeypatch):
    home = _home(tmp_path, monkeypatch)
    _skill(home, ".claude", "plugins", "cache", "ecc", "ecc", "2.2.0", "skills", "antiga")
    nova = _skill(home, ".claude", "plugins", "cache", "ecc", "ecc", "2.10.0", "skills", "nova")
    (home / ".codex").mkdir()

    skill_bridge.rebuild(home, log=lambda *a: None)

    alvos = _alvos(home / ".codex" / "skills")
    assert alvos == {"nova": str(nova)}  # 2.10.0 > 2.2.0 numericamente; a antiga nem entra


def test_rebuild_limpa_link_podre_e_troca_versao(tmp_path, monkeypatch):
    home = _home(tmp_path, monkeypatch)
    velha = _skill(home, ".claude", "plugins", "cache", "ecc", "ecc", "2.2.0", "skills", "s")
    ponte = home / ".pi" / "agent" / "skills-bridge"
    ponte.mkdir(parents=True)
    (ponte / "s").symlink_to(velha)
    (ponte / "morta").symlink_to(home / ".claude" / "plugins" / "cache" / "ecc" / "ecc" /
                                 "1.0.0" / "skills" / "morta")  # alvo inexistente

    skill_bridge.rebuild(home, log=lambda *a: None)
    assert not (ponte / "morta").exists() and (ponte / "s").exists()

    # bump de versão: o link MUDA pra versão nova sem intervenção
    nova = _skill(home, ".claude", "plugins", "cache", "ecc", "ecc", "2.3.0", "skills", "s")
    skill_bridge.rebuild(home, log=lambda *a: None)
    assert Path(os.readlink(ponte / "s")) == nova

    # idempotente: terceiro rebuild não cria/troca/remove nada
    st = skill_bridge.rebuild(home, log=lambda *a: None)["pi"]
    assert (st["criados"], st["trocados"], st["removidos"]) == (0, 0, 0)


def test_nunca_toca_em_o_que_nao_e_gerenciado(tmp_path, monkeypatch):
    home = _home(tmp_path, monkeypatch)
    _skill(home, ".claude", "skills", "comum")
    ponte = home / ".codex" / "skills"
    ponte.mkdir(parents=True)
    real = ponte / ".system"  # dir real do codex
    real.mkdir()
    (real / "SKILL.md").write_text("x", encoding="utf-8")
    link_estranho = ponte / "comum"  # link À MÃO pra fora das fontes, com nome de skill fonte
    link_estranho.symlink_to(tmp_path / "outro-lugar")

    skill_bridge.rebuild(home, log=lambda *a: None)

    assert real.is_dir() and not real.is_symlink()
    assert os.readlink(link_estranho) == str(tmp_path / "outro-lugar")

def test_link_quebrado_com_skill_viva_noutra_fonte_recria_no_mesmo_run(tmp_path, monkeypatch):
    """Regressão: sweep removia e o laço de criação pulava (snapshot velho) — a skill ficava
    fora das pontes até o rebuild seguinte."""
    home = _home(tmp_path, monkeypatch)
    viva = _skill(home, ".claude", "skills", "s")
    ponte = home / ".pi" / "agent" / "skills-bridge"
    ponte.mkdir(parents=True)
    # link gerenciado pendurado pro cache (plugin removido), mas a skill vive em ~/.claude/skills
    (ponte / "s").symlink_to(home / ".claude" / "plugins" / "cache" / "ecc" / "ecc" /
                             "1.0.0" / "skills" / "s")

    st = skill_bridge.rebuild(home, log=lambda *a: None)["pi"]

    assert st["removidos"] == 1 and st["criados"] == 1
    assert Path(os.readlink(ponte / "s")) == viva


def test_marcador_como_substring_fora_das_fontes_nao_e_gerenciado(tmp_path, monkeypatch):
    """Regra dura: /mnt/backup/<home>/.claude/skills/foo CONTÉM o marcador como substring mas
    resolve fora das fontes — nunca pode ser apagado."""
    home = _home(tmp_path, monkeypatch)
    _skill(home, ".claude", "skills", "s")
    backup = tmp_path / "mnt" / "backup" / str(home).lstrip("/") / ".claude" / "skills" / "foo"
    backup.mkdir(parents=True)
    (backup / "SKILL.md").write_text("x", encoding="utf-8")
    ponte = home / ".codex" / "skills"
    ponte.mkdir(parents=True)
    link = ponte / "backup-do-usuario"
    link.symlink_to(backup)

    skill_bridge.rebuild(home, log=lambda *a: None)

    assert link.is_symlink() and Path(os.readlink(link)) == backup


def test_link_relativo_pra_dentro_de_fonte_e_gerenciado(tmp_path, monkeypatch):
    """Link relativo (o jeito natural de fazer à mão de dentro da ponte) resolve pra fonte:
    é gerenciado — apontando certo, vira 'mantido', não bloqueio silencioso."""
    home = _home(tmp_path, monkeypatch)
    origem = _skill(home, ".claude", "skills", "s")
    ponte = home / ".kimi-code" / "skills-bridge"
    ponte.mkdir(parents=True)
    (ponte / "s").symlink_to(os.path.relpath(origem, ponte))

    st = skill_bridge.rebuild(home, log=lambda *a: None)["kimi"]

    assert st["mantidos"] == 1 and st["criados"] == 0 and st["removidos"] == 0


def test_zero_skills_nas_fontes_recusa_o_sweep(tmp_path, monkeypatch):
    """Update do Claude reorganizando o layout não pode derrubar as três pontes de uma vez."""
    home = _home(tmp_path, monkeypatch)
    ponte = home / ".pi" / "agent" / "skills-bridge"
    ponte.mkdir(parents=True)
    link = ponte / "s"
    link.symlink_to(home / ".claude" / "skills" / "s")  # pendurado, fonte não existe

    assert skill_bridge.rebuild(home, log=lambda *a: None) == {}
    assert link.is_symlink()  # intacto


def test_avisa_quando_config_do_harness_nao_aponta_pra_ponte(tmp_path, monkeypatch, caplog):
    home = _home(tmp_path, monkeypatch)
    _skill(home, ".claude", "skills", "s")
    agent = home / ".pi" / "agent"
    agent.mkdir(parents=True)
    (agent / "settings.json").write_text(json.dumps({"skills": ["~/.claude/skills"]}),
                                         encoding="utf-8")

    avisos: list[str] = []
    skill_bridge.rebuild(home, log=avisos.append)

    assert any("não está na config" in a and "pi" in a for a in avisos)


def test_harness_ausente_e_ignorado(tmp_path, monkeypatch):
    home = _home(tmp_path, monkeypatch)
    _skill(home, ".claude", "skills", "s")
    # nenhum dir de harness — nada acontece, nada quebra
    assert skill_bridge.rebuild(home, log=lambda *a: None) == {}
