"""Ponte de skills: materializa as skills do ecossistema Claude nos CLIs que não descobrem sozinhos.

O omp varre `~/.claude/skills`, o cache de plugins e `~/.agents/skills` na largada; pi, kimi e
codex não — cada um lê só a(s) pasta(s) declarada na própria config. Sem esta ponte, cada CLI
desses mantém uma fazenda de symlinks à mão apontando pro cache VERSIONADO dos plugins
(`.../ecc/2.2.0/skills/...`): bump de versão do plugin = dezenas de links pendurados, calados.

O módulo rebuilda essas fazendas a partir das fontes, com duas regras duras:

- **Só mexe em symlink cujo alvo está numa fonte conhecida** — arquivo/diretório real (o
  `.system` do codex, uma skill própria do usuário) nunca é criado, movido ou apagado.
- **Stdlib-only** (mesma regra do `engines.py`): o installer chama com o `python3` do sistema,
  sem a venv do backend.

Roda em dois momentos (precedente `migracao_sidecars`): no `install-claude-wrapper.sh` e na
subida do backend — atualizar aqui é `git pull` + restart, e ninguém garante o installer.

Harness novo = uma linha em `TARGETS`. O omp não está na tabela de propósito: ele descobre
nativo, materializar seria duplicar.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import tomllib
from pathlib import Path

_REPO = Path(__file__).resolve().parents[2]

# Ordem = precedência; primeiro com o nome vence (mesmo desenho dos providers do omp: skills do
# usuário > plugins instalados > convenção cruzada). O cache vem antes do marketplace: é a cópia
# instalada que o Claude realmente executa.
def _fontes(home: Path) -> list[tuple[str, Path]]:
    return [
        ("claude", home / ".claude" / "skills"),
        ("repo", _REPO / "skills"),
        ("cache", home / ".claude" / "plugins" / "cache"),
        ("marketplace", home / ".claude" / "plugins" / "marketplaces"),
        ("agents", home / ".agents" / "skills"),
    ]


# Marcadores de alvo que identificam um symlink como GERENCIADO (criado pela ponte ou à mão
# apontando pra uma fonte). Sem eles, a varredura apagaria link do usuário pra lugar nenhum.
def _marcadores(home: Path) -> tuple[str, ...]:
    return (
        str(home / ".claude" / "skills") + os.sep,
        str(home / ".claude" / "plugins") + os.sep,
        str(home / ".agents" / "skills") + os.sep,
        str(_REPO / "skills") + os.sep,
    )


def _versao_chave(nome: str) -> tuple:
    """2.10.0 > 2.2.0 numericamente; nome não-numérico vai pro fim e desempata por string."""
    try:
        return (1, *(int(p) for p in nome.split(".")))
    except ValueError:
        return (0, nome)


def _skills_em(dir_skills: Path) -> list[Path]:
    """Dirs com SKILL.md um nível abaixo — mesmo layout não-recursivo dos providers do omp."""
    if not dir_skills.is_dir():
        return []
    return sorted(p for p in dir_skills.iterdir() if (p / "SKILL.md").is_file())


def _varrer_fontes(home: Path) -> dict[str, Path]:
    """nome da skill -> diretório fonte, respeitando a precedência de `_fontes`."""
    achadas: dict[str, Path] = {}
    for tipo, raiz in _fontes(home):
        if tipo == "cache":
            # cache/<plugin>/<plugin>/<versão>/skills/* — só a versão MAIS NOVA de cada plugin:
            # é ela que o Claude carrega, e link pras antigas é exatamente o defeito que esta
            # ponte existe pra apagar.
            for plugin in sorted(raiz.iterdir()) if raiz.is_dir() else []:
                versoes: dict[str, list[Path]] = {}
                for sub in plugin.iterdir() if plugin.is_dir() else []:
                    for ver in sub.iterdir() if sub.is_dir() else []:
                        if (ver / "skills").is_dir():
                            versoes.setdefault(str(plugin), []).append(ver)
                for vers in versoes.values():
                    novo = max(vers, key=lambda v: _versao_chave(v.name))
                    for skill in _skills_em(novo / "skills"):
                        achadas.setdefault(skill.name, skill)
        elif tipo == "marketplace":
            for mkt in sorted(raiz.iterdir()) if raiz.is_dir() else []:
                for skill in _skills_em(mkt / "skills"):
                    achadas.setdefault(skill.name, skill)
        else:
            for skill in _skills_em(raiz):
                achadas.setdefault(skill.name, skill)
    return achadas


def _config_ok_pi(base: Path, ponte: Path) -> bool | None:
    settings = base / ".pi" / "agent" / "settings.json"
    if not settings.is_file():
        return None
    try:
        skills = json.loads(settings.read_text(encoding="utf-8")).get("skills") or []
    except (ValueError, OSError):
        return None
    return any(str(ponte) in os.path.expanduser(str(s)) for s in skills)


def _config_ok_kimi(base: Path, ponte: Path) -> bool | None:
    config = base / ".kimi-code" / "config.toml"
    if not config.is_file():
        return None
    try:
        dirs = tomllib.loads(config.read_text(encoding="utf-8")).get("extra_skill_dirs") or []
    except (ValueError, OSError):
        return None
    return any(str(ponte) in os.path.expanduser(str(d)) for d in dirs)


# (harness, dir que a ponte gerencia, checagem de config). Só entra quem NÃO descobre sozinho;
# a existência do dir-PAI (~/.pi/agent) é o que decide se o harness está instalado.
TARGETS = (
    ("pi", Path(".pi/agent/skills-bridge"), Path(".pi/agent"), _config_ok_pi),
    ("kimi", Path(".kimi-code/skills-bridge"), Path(".kimi-code"), _config_ok_kimi),
    ("codex", Path(".codex/skills"), Path(".codex"), None),
)


def rebuild(home: Path | None = None, *, dry_run: bool = False,
            log=print) -> dict[str, dict[str, int]]:
    """Rebuilda a ponte de cada harness instalado. Retorna estatística por harness."""
    home = home or Path.home()
    fontes = _varrer_fontes(home)
    marcadores = _marcadores(home)
    stats: dict[str, dict[str, int]] = {}

    for nome, rel_ponte, rel_pai, config_ok in TARGETS:
        if not (home / rel_pai).is_dir():
            continue
        ponte = home / rel_ponte
        st = stats[nome] = {"criados": 0, "trocados": 0, "removidos": 0, "mantidos": 0,
                            "pulados_reais": 0}

        existentes = {p.name: p for p in ponte.iterdir()} if ponte.is_dir() else {}

        # Varre o que está lá: link gerenciado que saiu do conjunto (ou aponta pra alvo que
        # sumiu) é removido; entrada REAL com nome de skill fica e bloqueia o link (avisa).
        for nome_skill, entrada in sorted(existentes.items()):
            if not entrada.is_symlink():
                if nome_skill in fontes:
                    st["pulados_reais"] += 1
                    log(f"  {nome}: {nome_skill} é arquivo de verdade em {ponte} — mantido, "
                        f"link não criado")
                continue
            alvo = os.readlink(entrada)
            gerenciado = any(m in alvo for m in marcadores)
            if not gerenciado:
                continue
            quer = fontes.get(nome_skill)
            if quer is None or not entrada.exists():
                st["removidos"] += 1
                if not dry_run:
                    entrada.unlink()
            # alvo mudou (versão nova do plugin): a troca acontece no laço de criação abaixo

        if not dry_run:
            ponte.mkdir(parents=True, exist_ok=True)
        for nome_skill, origem in sorted(fontes.items()):
            entrada = ponte / nome_skill
            if nome_skill in existentes:
                # Entrada pré-existente só é tocada se for link GERENCIADO (alvo numa fonte
                # conhecida); link à mão pra fora das fontes e arquivo real ficam intactos.
                prev = existentes[nome_skill]
                if not prev.is_symlink() or not any(m in os.readlink(prev) for m in marcadores):
                    continue
            if entrada.is_symlink():
                if Path(os.readlink(entrada)) == origem:
                    st["mantidos"] += 1
                    continue
                st["trocados"] += 1
                if not dry_run:
                    entrada.unlink()
            else:
                st["criados"] += 1
            if not dry_run:
                entrada.symlink_to(origem)

        if config_ok is not None and config_ok(home, ponte) is False:
            log(f"  ⚠ {nome}: {ponte} não está na config do {nome} — a ponte foi montada mas "
                f"o {nome} não vai ler. Adicione o caminho à lista de skills dele.")

    return stats


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Rebuilding skill bridges for pi/kimi/codex")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args(argv)

    log = (lambda *a: None) if args.quiet else print
    stats = rebuild(dry_run=args.dry_run, log=log)
    if not args.quiet:
        for nome, st in stats.items():
            print(f"skill-bridge[{nome}]: {st['criados']} criados, {st['trocados']} trocados, "
                  f"{st['removidos']} removidos, {st['mantidos']} mantidos"
                  + (f", {st['pulados_reais']} reais pulados" if st["pulados_reais"] else ""))
        if not stats:
            print("skill-bridge: nenhum harness (pi/kimi/codex) encontrado")
    return 0


if __name__ == "__main__":
    sys.exit(main())
