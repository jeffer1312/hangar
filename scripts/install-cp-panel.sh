#!/usr/bin/env bash
# install-cp-panel.sh — instala o painel do claude-pocket pro Hyprland/Quickshell NESTA máquina.
#
# Faz (idempotente; re-rodar atualiza):
#   1. symlink ~/.local/bin/cp-panel-data          -> scripts/cp-panel-data deste clone;
#   2. symlink ~/.config/quickshell/claude-pocket  -> scripts/quickshell/claude-pocket deste clone;
#   3. bind SUPER + SHIFT + U e autostart no ~/.config/hypr/custom/ (entre marcadores).
#
# Instância SEPARADA do quickshell (qs -c claude-pocket): NÃO edita nenhum arquivo do rice, então
# update do dots-hyprland não apaga nada disto, e um erro aqui não derruba a barra.
#
# Rode uma vez por máquina, do clone local:  ./scripts/install-cp-panel.sh
set -euo pipefail

REPO="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"

command -v qs >/dev/null || { echo "erro: 'qs' (quickshell) não encontrado — este painel é só pra Hyprland+Quickshell" >&2; exit 1; }

mkdir -p "$HOME/.local/bin" "$HOME/.config/quickshell"
for s in cp-panel-data cp-panel-open cp-panel-tray cp-panel-action; do
    ln -sf "$REPO/scripts/$s" "$HOME/.local/bin/$s"
    echo "ok: ~/.local/bin/$s -> $REPO/scripts/$s"
done

# -n: sem isto, re-rodar com o symlink já existente criaria claude-pocket/claude-pocket dentro dele.
ln -sfn "$REPO/scripts/quickshell/claude-pocket" "$HOME/.config/quickshell/claude-pocket"
echo "ok: ~/.config/quickshell/claude-pocket -> $REPO/scripts/quickshell/claude-pocket"

# insere_bloco <arquivo> <marcador> <conteúdo> — idempotente: substitui entre marcadores se já existe.
insere_bloco() {
    local file=$1 tag=$2 body=$3
    local start="-- claude-pocket:${tag}:start" end="-- claude-pocket:${tag}:end"
    mkdir -p "$(dirname "$file")"
    touch "$file"
    local block="$start
$body
$end"
    # `--` obrigatório: o marcador começa com "--" (comentário Lua) e viraria opção do grep.
    if grep -qF -- "$start" "$file"; then
        BLOCK="$block" START="$start" END="$end" python3 - "$file" <<'PYEOF'
import os, re, sys
path, block = sys.argv[1], os.environ["BLOCK"]
start, end = os.environ["START"], os.environ["END"]
text = open(path, encoding="utf-8").read()
pattern = re.compile(re.escape(start) + r".*?" + re.escape(end), re.S)
open(path, "w", encoding="utf-8").write(pattern.sub(lambda _: block, text))
PYEOF
        echo "ok: bloco '$tag' ATUALIZADO em $file"
    else
        printf '\n%s\n' "$block" >> "$file"
        echo "ok: bloco '$tag' ADICIONADO em $file"
    fi
}

# remove_bloco <arquivo> <marcador> — apaga um bloco que versões antigas do installer criaram.
remove_bloco() {
    local file=$1 tag=$2
    local start="-- claude-pocket:${tag}:start" end="-- claude-pocket:${tag}:end"
    [[ -f $file ]] && grep -qF -- "$start" "$file" || return 0
    START="$start" END="$end" python3 - "$file" <<'PYEOF'
import os, re, sys
path = sys.argv[1]
start, end = os.environ["START"], os.environ["END"]
text = open(path, encoding="utf-8").read()
pattern = re.compile(r"\n*" + re.escape(start) + r".*?" + re.escape(end) + r"\n*", re.S)
open(path, "w", encoding="utf-8").write(pattern.sub("\n", text))
PYEOF
    echo "ok: bloco '$tag' REMOVIDO de $file (não é mais necessário)"
}

insere_bloco "$HOME/.config/hypr/custom/keybinds.lua" "panel-bind" \
'hl.bind("SUPER + SHIFT + U", hl.dsp.global("claude-pocket:toggle"), { description = "Claude Pocket: painel de sessões" })'

insere_bloco "$HOME/.config/hypr/custom/execs.lua" "panel-exec" \
'hl.exec_cmd("qs -c claude-pocket")
-- Ícone na bandeja (SNI). sleep: precisa do StatusNotifierWatcher da barra já no barramento.
hl.exec_cmd("bash -c \"sleep 5; $HOME/.local/bin/cp-panel-tray\"")'

# Sem regra de blur aqui de propósito: o rice já aplica blur em "quickshell:.*", e o painel usa
# alpha dentro do ignore_alpha dele (0.79). Regra própria seria config redundante pra manter.
remove_bloco "$HOME/.config/hypr/custom/rules.lua" "panel-blur"

echo
echo "Falta só subir o painel (uma vez; nos próximos logins o autostart cuida):"
echo "    qs -c claude-pocket &"
echo "    hyprctl reload        # pra pegar o keybind"
echo
echo "Uso: SUPER + SHIFT + U  (ou: qs -c claude-pocket ipc call panel toggle)"
