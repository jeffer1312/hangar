#!/usr/bin/env bash
# Ponte de skills do Claude Code para o Kimi Code e para o Pi.
#
# POR QUE ISTO MORA NO HANGAR. Quem cria sessão Pi e Kimi é o app
# (`hangar-send --new … --provider pi`), e uma sessão nascida assim NÃO enxerga as skills do
# Claude: cada agente varre o próprio diretório. Sem esta ponte, a sessão que o app abriu começa
# sem o ferramental que a mesma máquina já tem — e o `scripts/checar-skills.sh` deste repo
# denuncia exatamente isso ("falta o symlink em ~/.pi/agent/skills-bridge") sem resolver.
#
# O que ele NÃO faz: não instala skill nenhuma, não baixa nada, não mexe no `settings.json`.
# Lê os plugins que o Claude já tem habilitados e cria um symlink por skill nos diretórios que os
# outros dois agentes leem. Rodar duas vezes não muda nada.
#
# Idempotente e seguro de rodar sem Pi nem Kimi instalados: cada ponte só é tocada se o diretório
# do agente existir.
set -euo pipefail

SETTINGS="$HOME/.claude/settings.json"
INSTALLED="$HOME/.claude/plugins/installed_plugins.json"

if [ ! -f "$SETTINGS" ] || [ ! -f "$INSTALLED" ]; then
  echo "pulado: sem $SETTINGS ou $INSTALLED (Claude Code não instalado nesta conta)"
  exit 0
fi

python3 - <<'PYEOF'
import glob, json, os, re

home = os.path.expanduser("~")
cache_prefix = os.path.join(home, ".claude/plugins/cache")

settings = json.load(open(os.path.join(home, ".claude/settings.json")))
installed = json.load(open(os.path.join(home, ".claude/plugins/installed_plugins.json")))
enabled = {k for k, v in settings.get("enabledPlugins", {}).items() if v}

# Plugin que o Pi carrega como PACOTE em vez de skill em ponte, porque o package.json dele declara
# um bloco `pi` (extensão + skills). O `superpowers` precisa disso: a extensão dele injeta o
# bootstrap no início da sessão, coisa que a ponte de skill sozinha não faz — sem o pacote, o
# modelo acaba abrindo o SKILL.md na mão a cada sessão nova. O valor é o nome estável do link em
# ~/.pi/agent/packages/, pra o settings.json nunca apontar pra um diretório com versão dentro.
PI_PACKAGES = {
    "superpowers@claude-plugins-official": "superpowers",
    "ponytail@ponytail": "ponytail",
}
# Plugin que NÃO deve entrar na ponte. Dois casos: o que já chega ao Pi por outro caminho (o
# settings.json dele aponta direto pro marketplace) — bridgeá-lo de novo carregaria cada skill duas
# vezes, com colisão de nome; e o de PI_PACKAGES, que traz as próprias skills dentro do pacote.
#
# O primeiro caso é por máquina, não do app: cada um tem os seus. Um por linha em
# ~/.claude/skills-bridge-skip.txt, no formato `plugin@marketplace`; `#` comenta.
skip_file = os.path.join(home, ".claude/skills-bridge-skip.txt")
PI_SKIP = set(PI_PACKAGES)
if os.path.isfile(skip_file):
    with open(skip_file, encoding="utf-8") as fh:
        PI_SKIP |= {ln.strip() for ln in fh if ln.strip() and not ln.startswith("#")}


def skill_name(d):
    """Nome do frontmatter, caindo no nome do diretório — a mesma regra que o Pi usa."""
    try:
        with open(os.path.join(d, "SKILL.md"), encoding="utf-8") as fh:
            for i, line in enumerate(fh):
                if i > 40 or (i and line.strip() == "---"):
                    break
                m = re.match(r"^name:\s*(\S+)", line)
                if m:
                    return m.group(1)
    except (OSError, UnicodeDecodeError):
        pass
    return os.path.basename(d)


def skill_dirs(install_path):
    """Um plugin traz skills/<nome>/SKILL.md, ou um SKILL.md único na raiz."""
    if os.path.isfile(os.path.join(install_path, "SKILL.md")):
        return [install_path]
    return [d for d in sorted(glob.glob(os.path.join(install_path, "skills", "*")))
            if os.path.isfile(os.path.join(d, "SKILL.md"))]


def collect(plugins):
    wanted = {}
    for name, entries in installed.get("plugins", {}).items():
        if name not in plugins:
            continue
        for e in entries:
            for d in skill_dirs(e["installPath"]):
                wanted[skill_name(d)] = d
    return wanted


def sync(bridge, wanted):
    os.makedirs(bridge, exist_ok=True)
    for name, target in wanted.items():
        link = os.path.join(bridge, name)
        if os.path.islink(link):
            if os.readlink(link) != target:
                os.unlink(link)
                os.symlink(target, link)
        elif not os.path.exists(link):
            os.symlink(target, link)
    # Remove link de plugin desabilitado, apagado ou que mudou de versão. Conjunto vazio quase
    # sempre significa que o JSON acima não disse nada (mudou de formato, arquivo ilegível) e quase
    # nunca "todo plugin está desligado" — então não poda, em vez de esvaziar a ponte inteira por
    # causa de uma surpresa de parse.
    if not wanted:
        return
    for f in os.listdir(bridge):
        link = os.path.join(bridge, f)
        if os.path.islink(link) and os.readlink(link).startswith(cache_prefix + os.sep) and f not in wanted:
            os.unlink(link)


feito = []

if os.path.isdir(os.path.join(home, ".kimi-code")):
    sync(os.path.join(home, ".kimi-code/skills-bridge"), collect(enabled))
    feito.append("kimi")

pi_bridge = os.path.join(home, ".pi/agent/skills-bridge")
if os.path.isdir(os.path.dirname(pi_bridge)):
    wanted = collect(enabled - PI_SKIP)
    # O Pi varre ~/.claude/skills primeiro, então o que mora lá vence; bridgear a mesma skill de
    # novo só produziria aviso de colisão.
    taken = {skill_name(d) for d in glob.glob(os.path.join(home, ".claude/skills", "*"))
             if os.path.isfile(os.path.join(d, "SKILL.md"))}
    sync(pi_bridge, {n: t for n, t in wanted.items() if n not in taken})

    # Aponta ~/.pi/agent/packages/<nome> pro install atual de cada PI_PACKAGES. O settings.json do
    # Pi lista esses caminhos estáveis, então subir de versão move o symlink em vez de quebrar a
    # entrada.
    pi_packages = os.path.join(home, ".pi/agent/packages")
    os.makedirs(pi_packages, exist_ok=True)
    for key, link_name in PI_PACKAGES.items():
        entries = installed.get("plugins", {}).get(key) or []
        if not entries:
            continue
        target = entries[0]["installPath"]
        link = os.path.join(pi_packages, link_name)
        if os.path.islink(link):
            if os.readlink(link) != target:
                os.unlink(link)
                os.symlink(target, link)
        elif not os.path.exists(link):
            os.symlink(target, link)
    feito.append("pi")

print("ponte de skills: " + (", ".join(feito) if feito else "nenhum agente instalado, nada a fazer"))
PYEOF
