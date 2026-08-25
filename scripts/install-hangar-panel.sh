#!/usr/bin/env bash
# install-hangar-panel.sh — instala o painel do hangar pro Hyprland/Quickshell NESTA máquina.
#
# Faz (idempotente; re-rodar atualiza):
#   1. symlink ~/.local/bin/hangar-panel-data          -> scripts/hangar-panel-data deste clone;
#   2. symlink ~/.config/quickshell/hangar  -> scripts/quickshell/hangar deste clone;
#   3. bind SUPER + SHIFT + U e autostart no ~/.config/hypr/custom/ (entre marcadores).
#
# Instância SEPARADA do quickshell (qs -c hangar): NÃO edita nenhum arquivo do rice, então
# update do dots-hyprland não apaga nada disto, e um erro aqui não derruba a barra.
#
# Rode uma vez por máquina, do clone local:  ./scripts/install-hangar-panel.sh
set -euo pipefail

REPO="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"

command -v qs >/dev/null || { echo "erro: 'qs' (quickshell) não encontrado — este painel é só pra Hyprland+Quickshell" >&2; exit 1; }

mkdir -p "$HOME/.local/bin" "$HOME/.config/quickshell"
for s in data open tray action; do
    ln -sf "$REPO/scripts/hangar-panel-$s" "$HOME/.local/bin/hangar-panel-$s"
    echo "ok: ~/.local/bin/hangar-panel-$s -> $REPO/scripts/hangar-panel-$s"
    # Nome antigo, permanente: o QML de uma instância do painel que ainda esteja VIVA chama
    # `~/.local/bin/cp-panel-action` pelo caminho literal, e ela só relê o QML quando reiniciar.
    ln -sf "$REPO/scripts/hangar-panel-$s" "$HOME/.local/bin/cp-panel-$s"
done

# -n: sem isto, re-rodar com o symlink já existente criaria hangar/hangar dentro dele.
ln -sfn "$REPO/scripts/quickshell/hangar" "$HOME/.config/quickshell/hangar"
echo "ok: ~/.config/quickshell/hangar -> $REPO/scripts/quickshell/hangar"

# ── Nome antigo da instância do quickshell ───────────────────────────────────
# A instância se chamava "claude-pocket". Ela NÃO ganha link de compatibilidade: duas instâncias
# com a mesma pasta seriam dois painéis desenhando na tela ao mesmo tempo, cada um segurando o
# lock do outro. O link velho sai, e a instância velha morre logo abaixo, junto com a nova subindo.
if [[ -L "$HOME/.config/quickshell/claude-pocket" ]]; then
    rm -f "$HOME/.config/quickshell/claude-pocket"
    echo "ok: symlink antigo ~/.config/quickshell/claude-pocket removido"
fi
# Config do painel: mesmo arquivo, nome novo (o PanelConfig.qml só conhece o novo).
if [[ -f "$HOME/.config/claude-pocket-panel.json" && ! -e "$HOME/.config/hangar-panel.json" ]]; then
    mv "$HOME/.config/claude-pocket-panel.json" "$HOME/.config/hangar-panel.json"
    echo "ok: config do painel migrada -> ~/.config/hangar-panel.json"
fi
# Units antigas: desabilitar e apagar ANTES de escrever as novas, senão as duas ficam habilitadas
# e o boot sobe dois painéis (um deles apontando pra uma instância que não existe mais).
for u in cp-panel.service cp-panel-tray.service; do
    if [[ -f "$HOME/.config/systemd/user/$u" ]]; then
        systemctl --user disable --now "$u" 2>/dev/null || true
        rm -f "$HOME/.config/systemd/user/$u"
        echo "ok: unit antiga $u removida"
    fi
done
pkill -f "qs -n -c claude-pocket" 2>/dev/null || true

# insere_bloco <arquivo> <marcador> <conteúdo> — idempotente: substitui entre marcadores se já existe.
insere_bloco() {
    local file=$1 tag=$2 body=$3
    local start="-- hangar:${tag}:start" end="-- hangar:${tag}:end"
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

# remove_bloco <arquivo> <marcador> [marca] — apaga um bloco que versões antigas do installer
# criaram. A `marca` (padrão: o nome ANTIGO) existe porque o marcador mudou de `claude-pocket:`
# pra `hangar:`: sem apagar o velho, o keybind ficaria escrito DUAS vezes no keybinds.lua, uma
# delas chamando um dispatcher (`claude-pocket:toggle`) que não existe mais.
remove_bloco() {
    local file=$1 tag=$2 marca=${3:-claude-pocket}
    local start="-- ${marca}:${tag}:start" end="-- ${marca}:${tag}:end"
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

# Os blocos com o marcador ANTIGO saem antes de os novos entrarem.
remove_bloco "$HOME/.config/hypr/custom/keybinds.lua" "panel-bind"
remove_bloco "$HOME/.config/hypr/custom/execs.lua" "panel-exec"

insere_bloco "$HOME/.config/hypr/custom/keybinds.lua" "panel-bind" \
'hl.bind("SUPER + SHIFT + U", hl.dsp.global("hangar:toggle"), { description = "Hangar: painel de sessões" })'

insere_bloco "$HOME/.config/hypr/custom/execs.lua" "panel-exec" \
'-- Painel e tray sobem como serviço systemd --user (gerados por este install). Aqui só
-- garantimos o ENV gráfico: o serviço pode subir antes do Hyprland exportar WAYLAND_DISPLAY /
-- HYPRLAND_INSTANCE_SIGNATURE. import-environment + restart resolve (idempotente, barato) — é a
-- rede pra máquinas onde essas vars não estão no systemd --user antes do serviço.
hl.exec_cmd("bash -c '\''systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE XDG_RUNTIME_DIR; systemctl --user restart hangar-panel.service hangar-panel-tray.service'\''")'

# Sem regra de blur aqui de propósito: o rice já aplica blur em "quickshell:.*", e o painel usa
# alpha dentro do ignore_alpha dele (0.79). Regra própria seria config redundante pra manter.
remove_bloco "$HOME/.config/hypr/custom/rules.lua" "panel-blur"
remove_bloco "$HOME/.config/hypr/custom/rules.lua" "panel-blur" hangar

# --- serviço do painel -----------------------------------------------------------------------
PANEL_UNIT="$HOME/.config/systemd/user/hangar-panel.service"
mkdir -p "$(dirname "$PANEL_UNIT")"
cat > "$PANEL_UNIT" <<EOF
[Unit]
Description=Hangar — painel (Quickshell layer-shell)
# O Hyprland não é gerenciado pelo systemd, então não dá After=; o Restart cobre a espera pelo
# compositor. flock -n no ExecStart evita instância zumbi em hyprctl reload.
StartLimitIntervalSec=0

[Service]
ExecStart=/bin/bash -c 'exec flock -n "\$XDG_RUNTIME_DIR/hangar-panel.lock" qs -n -c hangar'
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
EOF

# Instância avulsa do painel (subida na mão ou pelo login antigo) segura o hangar-panel.lock; sem
# matá-la, o flock -n do serviço bate no lock e entra em restart-loop até ela morrer. O pattern
# não casa a cmdline deste script (que é "bash install-hangar-panel.sh"), então é seguro aqui.
pkill -f "qs -n -c hangar" 2>/dev/null || true
sleep 1   # dá tempo do processo morto soltar o flock; se não bastar, o Restart do serviço recupera

systemctl --user daemon-reload
# && / else em vez de "|| true": engolir o código e ecoar "ok" incondicional reportava sucesso
# mesmo com unit malformada ou sessão systemd --user fora do ar. Falha tem que aparecer.
if systemctl --user enable --now hangar-panel.service; then
    echo "ok: hangar-panel.service (systemd --user) habilitado + iniciado"
else
    echo "ERRO: hangar-panel.service não subiu — veja 'systemctl --user status hangar-panel.service'" >&2
fi

# Ícone da bandeja como serviço systemd --user (antes era exec_cmd one-shot no execs.lua, que
# morria se a barra ainda não tivesse publicado o StatusNotifierWatcher). StartLimitIntervalSec=0
# + Restart=on-failure: o script sai com 1 enquanto o watcher não existe, e o systemd re-tenta sem
# teto até a barra subir — nada de ordenar via After=, já que o Quickshell do rice não é do systemd.
UNIT="$HOME/.config/systemd/user/hangar-panel-tray.service"
mkdir -p "$(dirname "$UNIT")"
cat > "$UNIT" <<EOF
[Unit]
Description=Hangar — ícone de bandeja (SNI) do painel
StartLimitIntervalSec=0

[Service]
Environment=PATH=$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin
ExecStart=$HOME/.local/bin/hangar-panel-tray
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
EOF
systemctl --user daemon-reload
if systemctl --user enable --now hangar-panel-tray.service; then
    echo "ok: hangar-panel-tray.service (systemd --user) habilitado + iniciado"
else
    echo "ERRO: hangar-panel-tray.service não subiu — veja 'systemctl --user status hangar-panel-tray.service'" >&2
fi

# Backend desatualizado após git pull foi a causa do HTTP 404 no launcher — reinicia se existir.
if systemctl --user list-unit-files hangar-backend.service >/dev/null 2>&1; then
    systemctl --user restart hangar-backend.service && echo "ok: backend reiniciado"
fi

echo
echo "Painel e tray já subiram como serviço systemd --user (e sobem sozinhos no boot)."
echo "Rode 'hyprctl reload' uma vez pra pegar o keybind e o autostart de env."
echo
echo "Uso: SUPER + SHIFT + U  (ou: qs -c hangar ipc call panel toggle)"
