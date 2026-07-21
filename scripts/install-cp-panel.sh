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
'-- Painel e tray sobem como serviço systemd --user (gerados por este install). Aqui só
-- garantimos o ENV gráfico: o serviço pode subir antes do Hyprland exportar WAYLAND_DISPLAY /
-- HYPRLAND_INSTANCE_SIGNATURE. import-environment + restart resolve (idempotente, barato) — é a
-- rede pra máquinas onde essas vars não estão no systemd --user antes do serviço.
hl.exec_cmd("bash -c '\''systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE XDG_RUNTIME_DIR; systemctl --user restart cp-panel.service cp-panel-tray.service'\''")'

# Sem regra de blur aqui de propósito: o rice já aplica blur em "quickshell:.*", e o painel usa
# alpha dentro do ignore_alpha dele (0.79). Regra própria seria config redundante pra manter.
remove_bloco "$HOME/.config/hypr/custom/rules.lua" "panel-blur"

# --- serviço do painel -----------------------------------------------------------------------
PANEL_UNIT="$HOME/.config/systemd/user/cp-panel.service"
mkdir -p "$(dirname "$PANEL_UNIT")"
cat > "$PANEL_UNIT" <<EOF
[Unit]
Description=Claude Pocket — painel (Quickshell layer-shell)
# O Hyprland não é gerenciado pelo systemd, então não dá After=; o Restart cobre a espera pelo
# compositor. flock -n no ExecStart evita instância zumbi em hyprctl reload.
StartLimitIntervalSec=0

[Service]
ExecStart=/bin/bash -c 'exec flock -n "\$XDG_RUNTIME_DIR/cp-panel.lock" qs -n -c claude-pocket'
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
EOF

# Instância avulsa do painel (subida na mão ou pelo login antigo) segura o cp-panel.lock; sem
# matá-la, o flock -n do serviço bate no lock e entra em restart-loop até ela morrer. O pattern
# não casa a cmdline deste script (que é "bash install-cp-panel.sh"), então é seguro aqui.
pkill -f "qs -n -c claude-pocket" 2>/dev/null || true
sleep 1   # dá tempo do processo morto soltar o flock; se não bastar, o Restart do serviço recupera

systemctl --user daemon-reload
# && / else em vez de "|| true": engolir o código e ecoar "ok" incondicional reportava sucesso
# mesmo com unit malformada ou sessão systemd --user fora do ar. Falha tem que aparecer.
if systemctl --user enable --now cp-panel.service; then
    echo "ok: cp-panel.service (systemd --user) habilitado + iniciado"
else
    echo "ERRO: cp-panel.service não subiu — veja 'systemctl --user status cp-panel.service'" >&2
fi

# Ícone da bandeja como serviço systemd --user (antes era exec_cmd one-shot no execs.lua, que
# morria se a barra ainda não tivesse publicado o StatusNotifierWatcher). StartLimitIntervalSec=0
# + Restart=on-failure: o script sai com 1 enquanto o watcher não existe, e o systemd re-tenta sem
# teto até a barra subir — nada de ordenar via After=, já que o Quickshell do rice não é do systemd.
UNIT="$HOME/.config/systemd/user/cp-panel-tray.service"
mkdir -p "$(dirname "$UNIT")"
cat > "$UNIT" <<EOF
[Unit]
Description=Claude Pocket — ícone de bandeja (SNI) do painel
StartLimitIntervalSec=0

[Service]
Environment=PATH=$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=$HOME/.local/bin/cp-panel-tray
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload
if systemctl --user enable --now cp-panel-tray.service; then
    echo "ok: cp-panel-tray.service (systemd --user) habilitado + iniciado"
else
    echo "ERRO: cp-panel-tray.service não subiu — veja 'systemctl --user status cp-panel-tray.service'" >&2
fi

# Backend desatualizado após git pull foi a causa do HTTP 404 no launcher — reinicia se existir.
if systemctl --user list-unit-files claude-cockpit-backend.service >/dev/null 2>&1; then
    systemctl --user restart claude-cockpit-backend.service && echo "ok: backend reiniciado"
fi

echo
echo "Painel e tray já subiram como serviço systemd --user (e sobem sozinhos no boot)."
echo "Rode 'hyprctl reload' uma vez pra pegar o keybind e o autostart de env."
echo
echo "Uso: SUPER + SHIFT + U  (ou: qs -c claude-pocket ipc call panel toggle)"
