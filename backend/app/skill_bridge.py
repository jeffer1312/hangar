"""Ponte de skills: materializa as skills do ecossistema Claude nos CLIs que não descobrem sozinhos.

O omp varre `~/.claude/skills`, o cache de plugins e `~/.agents/skills` na largada; pi, kimi e
codex não — cada um lê só a(s) pasta(s) declarada na própria config. Sem esta ponte, cada CLI
desses mantém uma fazenda de symlinks à mão apontando pro cache VERSIONADO dos plugins
(`.../ecc/2.2.0/skills/...`): bump de versão do plugin = dezenas de links pendurados, calados.

O módulo rebuilda essas fazendas a partir das fontes, com duas regras duras:

- **Só mexe em symlink cujo alvo RESOLVE pra dentro de uma fonte conhecida** (realpath +
  is_relative_to, nunca substring — um link pra `/mnt/backup/home/u/.claude/skills/foo` contém o
  marcador como substring e NÃO é gerenciado). Arquivo/diretório real (o `.system` do codex, uma
  skill própria do usuário) e link pra fora das fontes nunca são criados, movidos ou apagados.
- **Stdlib-only** (mesma regra do `engines.py`): o installer chama com o `python3` do sistema,
  sem a venv do backend. `tomllib` (3.11+) é importado condicionalmente — sem ele, só a
  checagem de config do kimi é pulada.

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
from pathlib import Path

try:
    import tomllib
except ImportError:  # python3 do sistema < 3.11 — só a checagem de config do kimi usa
    tomllib = None

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


def _raizes(home: Path) -> tuple[str, ...]:
    """Raízes de fonte normalizadas — a comparação de 'gerenciado' é por contenção de path."""
    return tuple(os.path.normpath(str(p)) for _, p in _fontes(home))


def _gerenciado(entrada: Path, raizes: tuple[str, ...]) -> bool:
    """Symlink cujo alvo APONTA pra dentro de uma fonte. Resolução de UM nível (absoluto como
    veio, relativo contra o pai do link, `..` colapsado lexicalmente): realpath seguiria a
    cadeia inteira, e um link pra `~/.claude/skills/X` que é ELE um symlink pra fora seria
    lido como 'não gerenciado' — quando a referência à fonte é exatamente o que a ponte cria.
    Inverso coberto: alvo fora das fontes (`/mnt/backup/.../.claude/skills/foo`) não é."""
    if not entrada.is_symlink():
        return False
    alvo = os.readlink(entrada)
    if not os.path.isabs(alvo):
        alvo = os.path.join(entrada.parent, alvo)
    norm = os.path.normpath(alvo)
    return any(norm == r or norm.startswith(r + os.sep) for r in raizes)


def _versao_chave(nome: str) -> tuple:
    """2.10.0 > 2.2.0 numericamente; nome não-numérico vai pro fim."""
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
            # ponte existe pra apagar. Desempate por nome: iterdir não tem ordem garantida, e
            # `max` puro sobre empate escolheria um alvo diferente a cada rebuild.
            for plugin in sorted(raiz.iterdir()) if raiz.is_dir() else []:
                versoes: list[Path] = []
                for sub in plugin.iterdir() if plugin.is_dir() else []:
                    for ver in sub.iterdir() if sub.is_dir() else []:
                        if (ver / "skills").is_dir():
                            versoes.append(ver)
                if versoes:
                    novo = max(versoes, key=lambda v: (_versao_chave(v.name), str(v)))
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


def _mesmo_path(a: str, b: Path) -> bool:
    """Config de harness pode expandir ~ ou $HOME; comparação é de path, não de substring."""
    return os.path.normpath(os.path.expandvars(os.path.expanduser(a))) == os.path.normpath(str(b))


def _config_ok_pi(base: Path, ponte: Path) -> bool | None:
    settings = base / ".pi" / "agent" / "settings.json"
    if not settings.is_file():
        return None
    try:
        skills = json.loads(settings.read_text(encoding="utf-8")).get("skills") or []
    except (ValueError, OSError) as e:
        print(f"  ⚠ pi: settings.json ilegível ({e}) — não deu pra conferir se a ponte está na lista")
        return None
    return any(_mesmo_path(str(s), ponte) for s in skills)


def _config_ok_kimi(base: Path, ponte: Path) -> bool | None:
    if tomllib is None:
        return None
    config = base / ".kimi-code" / "config.toml"
    if not config.is_file():
        return None
    try:
        dirs = tomllib.loads(config.read_text(encoding="utf-8")).get("extra_skill_dirs") or []
    except (ValueError, OSError) as e:
        print(f"  ⚠ kimi: config.toml ilegível ({e}) — não deu pra conferir se a ponte está na lista")
        return None
    return any(_mesmo_path(str(d), ponte) for d in dirs)


# (harness, dir que a ponte gerencia, checagem de config). Só entra quem NÃO descobre sozinho;
# a existência do dir-PAI (~/.pi/agent) é o que decide se o harness está instalado.
TARGETS = (
    ("pi", Path(".pi/agent/skills-bridge"), Path(".pi/agent"), _config_ok_pi),
    ("kimi", Path(".kimi-code/skills-bridge"), Path(".kimi-code"), _config_ok_kimi),
    ("codex", Path(".codex/skills"), Path(".codex"), None),
)


def _gravar_link(entrada: Path, origem: Path) -> None:
    """Troca/criação ATÔMICA: tmp + rename no mesmo dir. unlink-then-symlink deixava a ponte
    furada se o symlink_to falhasse no meio; e a checagem de gerenciado é refeita imediatamente
    antes do rename — o que apareceu no lugar entre a varredura e aqui não é sobrescrito."""
    tmp = entrada.with_name(f".{entrada.name}.tmp-{os.getpid()}")
    try:
        tmp.symlink_to(origem)
    except FileExistsError:  # tmp órfão de um run morto no meio
        tmp.unlink()
        tmp.symlink_to(origem)
    if os.path.lexists(entrada) and not entrada.is_symlink():
        tmp.unlink()
        raise FileExistsError(f"{entrada} virou arquivo real no meio do rebuild — não sobrescrito")
    os.replace(tmp, entrada)


def _rebuild_um(nome: str, ponte: Path, fontes: dict[str, Path], raizes: tuple[str, ...],
                config_ok, home: Path, dry_run: bool, log) -> dict[str, int]:
    st = {"criados": 0, "trocados": 0, "removidos": 0, "mantidos": 0, "pulados_reais": 0}

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
        if not _gerenciado(entrada, raizes):
            if nome_skill in fontes:
                st["pulados_reais"] += 1
                log(f"  {nome}: {nome_skill} é link pra fora das fontes "
                    f"({os.readlink(entrada)}) — mantido, a skill fica sem ponte")
            continue
        quer = fontes.get(nome_skill)
        if quer is None or not entrada.exists():
            st["removidos"] += 1
            if not dry_run:
                entrada.unlink()
            # Fora do snapshot em qualquer modo: o laço de criação abaixo recria no MESMO run
            # quando a skill existe noutra fonte (remover sem recriar deixava a skill fora das
            # pontes até o próximo rebuild).
            del existentes[nome_skill]

    if not dry_run:
        ponte.mkdir(parents=True, exist_ok=True)
    for nome_skill, origem in sorted(fontes.items()):
        entrada = ponte / nome_skill
        if nome_skill in existentes and not _gerenciado(existentes[nome_skill], raizes):
            continue  # real ou link do usuário — já avisado acima
        # Comparação por alvo RESOLVIDO: link relativo à mão apontando pra mesma skill é
        # "mantido", não reescrito pra absoluto.
        if entrada.is_symlink() and os.path.realpath(entrada) == os.path.realpath(origem):
            st["mantidos"] += 1
            continue
        if entrada.is_symlink() or not os.path.lexists(entrada):
            st["trocados" if entrada.is_symlink() else "criados"] += 1
            if not dry_run:
                _gravar_link(entrada, origem)
        # lexists sem ser symlink = apareceu agora e é real: _gravar_link recusaria; conta como
        # pulado em vez de estourar o rebuild do harness inteiro
        else:
            st["pulados_reais"] += 1

    if config_ok is not None and config_ok(home, ponte) is False:
        verbo = "seria montada" if dry_run else "foi montada"
        log(f"  ⚠ {nome}: {ponte} não está na config do {nome} — a ponte {verbo} mas "
            f"o {nome} não vai ler. Adicione o caminho à lista de skills dele.")
    return st


def rebuild(home: Path | None = None, *, dry_run: bool = False,
            log=print) -> dict[str, dict[str, int]]:
    """Rebuilda a ponte de cada harness instalado. Retorna estatística por harness."""
    home = home or Path.home()
    fontes = _varrer_fontes(home)
    if not fontes:
        # Sem NENHUMA skill nas fontes, o sweep removeria todas as pontes. Fonte vazia é
        # legítimo; fonte INEXISTENTE somada às outras vazias é layout reorganizado por update
        # do Claude — e aí apagar tudo é o pior comportamento possível.
        log("skill-bridge: nenhuma skill encontrada em nenhuma fonte — rebuild recusado "
            "(as pontes ficam como estão)")
        return {}
    raizes = _raizes(home)
    stats: dict[str, dict[str, int]] = {}

    for nome, rel_ponte, rel_pai, config_ok in TARGETS:
        if not (home / rel_pai).is_dir():
            continue
        # Isolamento por harness: um erro num (permissão, FS cheio) não pode deixar os outros
        # sem rebuild — e não pode abortar a subida do backend que chamou.
        try:
            stats[nome] = _rebuild_um(nome, home / rel_ponte, fontes, raizes, config_ok,
                                      home, dry_run, log)
            # Loga SEMPRE, inclusive o rebuild que não mudou nada: sem esta linha, "a ponte rodou
            # e achou pouco" e "a ponte nem rodou" são o mesmo silêncio no diário — e foi
            # exatamente essa dúvida que sobrou quando 67 skills sumiram das três pontes.
            s = stats[nome]
            log(f"skill-bridge[{nome}]: {s['criados']} criados, {s['trocados']} trocados, "
                f"{s['removidos']} removidos, {s['mantidos']} mantidos"
                + (f", {s['pulados_reais']} entradas do usuário puladas"
                   if s["pulados_reais"] else ""))
        except Exception as e:  # noqa: BLE001 — os outros harnesses seguem
            log(f"  ⚠ {nome}: rebuild falhou ({e}) — a ponte dele ficou como estava")
            stats[nome] = {"erro": 1}
    return stats


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="Rebuilding skill bridges for pi/kimi/codex")
    ap.add_argument("--dry-run", action="store_true")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args(argv)

    log = (lambda *a: None) if args.quiet else print
    stats = rebuild(dry_run=args.dry_run, log=log)
    if not args.quiet:
        # O resumo por harness sai do proprio rebuild (todo chamador o recebe); aqui sobra o que
        # so o CLI sabe dizer.
        for nome, st in stats.items():
            if "erro" in st:
                print(f"skill-bridge[{nome}]: FALHOU — ver aviso acima")
        if not stats:
            print("skill-bridge: nenhum harness (pi/kimi/codex) encontrado")
    return 0


if __name__ == "__main__":
    sys.exit(main())
