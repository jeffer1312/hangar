#!/usr/bin/env bash
# Espelha a persona e as skills do Claude Code no Pi e Kimi.
# O Codex é gerenciado exclusivamente por app.codex_integracao, pelo importador nativo.
# Cada ponte só é tocada se a raiz daquele agente existir; arquivos do usuário são preservados.
set -euo pipefail

SETTINGS="$HOME/.claude/settings.json"
INSTALLED="$HOME/.claude/plugins/installed_plugins.json"
REPO="$(cd "$(dirname "$0")/.." && pwd)"

if [ ! -f "$SETTINGS" ] || [ ! -f "$INSTALLED" ]; then
  echo "pulado: sem $SETTINGS ou $INSTALLED (Claude Code não instalado nesta conta)"
  exit 0
fi

python3 - <<'PYEOF'
import json, os

home = os.path.expanduser("~")
CLAUDE_MD = os.path.join(home, ".claude/CLAUDE.md")
with open(os.path.join(home, ".claude/plugins/installed_plugins.json"), encoding="utf-8") as fh:
    installed = json.load(fh)

# O Pi carrega estes plugins como pacotes para executar também suas extensões de bootstrap.
# Os links estáveis evitam caminhos versionados no settings.json do Pi.
PI_PACKAGES = {
    "superpowers@claude-plugins-official": "superpowers",
    "ponytail@ponytail": "ponytail",
}
AGENTES = [
    {"nome": "kimi", "raiz": ".kimi-code", "persona": "AGENTS.md"},
    {"nome": "pi", "raiz": ".pi/agent", "persona": "CLAUDE.md"},
]


def ligar(link, target):
    """Reaponta symlinks sem sobrescrever arquivos reais do usuário."""
    if os.path.islink(link):
        if os.readlink(link) != target:
            os.unlink(link)
            os.symlink(target, link)
        return True
    if os.path.exists(link):
        return False
    os.symlink(target, link)
    return True


feito, avisos = [], []
for ag in AGENTES:
    raiz = os.path.join(home, ag["raiz"])
    if not os.path.isdir(raiz):
        continue

    if os.path.isfile(CLAUDE_MD) and not ligar(os.path.join(raiz, ag["persona"]), CLAUDE_MD):
        avisos.append(f"{ag['nome']}: {ag['persona']} já existe como arquivo de verdade — não mexi")
    feito.append(ag["nome"])

    if ag["nome"] == "pi":
        pi_packages = os.path.join(home, ".pi/agent/packages")
        os.makedirs(pi_packages, exist_ok=True)
        for key, link_name in PI_PACKAGES.items():
            entries = installed.get("plugins", {}).get(key) or []
            if entries:
                ligar(os.path.join(pi_packages, link_name), entries[0]["installPath"])

print("ponte: " + (", ".join(feito) if feito else "nenhum agente instalado, nada a fazer"))
for a in avisos:
    print("  " + a)
PYEOF

# Uma única dona das pontes do Pi e Kimi evita podas concorrentes das mesmas skills.
python3 "$REPO/backend/app/skill_bridge.py" --quiet || echo "  ponte de skills falhou"
