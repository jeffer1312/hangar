#!/usr/bin/env bash
# migrate-to-hangar.sh — migra ESTA máquina do nome claude-cockpit pro hangar.
#
# Roda do clone local (qualquer nome de pasta):  ./scripts/migrate-to-hangar.sh
# Idempotente: re-rodar numa máquina já migrada só re-verifica.
#
# Mesma estrutura da migrate-to-cockpit.sh (pocket -> cockpit), que já rodou em produção.
# O que faz:
#   1. renomeia a pasta do clone pra "hangar" (irmã, mesmo pai);
#   2. aponta o remote origin pra github.com/jeffer1312/hangar;
#   3. corrige paths absolutos antigos em settings.json de TODOS os perfis (~/.claude*) —
#      hooks/statusline são por perfil — e em ~/.tmux.conf (hooks do resurrect);
#   3b. copia as memórias do projeto pro slug novo (a pasta é chaveada pelo path);
#   3c. corrige o Icon= do .desktop já instalado, se houver;
#   4. troca as units systemd claude-cockpit-* pelas hangar-* (services-setup.sh)
#      e migra a claude-cockpit-deploy.service se existir (servidores com webhook);
#   5. re-roda install-cp-send.sh (cp-send + skills) e, se Hyprland+Quickshell,
#      install-cp-panel.sh (painel/tray).
#
# PRÉ-REQUISITO: o clone já tem que estar no commit do rename (units chamadas hangar-* no
# services-setup.sh, manifest com o nome novo). Este script mexe na MÁQUINA, não no conteúdo
# do repo — se rodar num clone antigo, o passo 4 recria as units com o nome velho.
#
# NÃO muda (de propósito — são dados/ids internos, e renomear derruba pareamento e loop em
# andamento): ~/.claude/.claude-pocket-pair/, .claude-pocket-uploads/, .claude-pocket-loop/,
# .claude-pocket-status/, instância quickshell "claude-pocket", cp-send/cp-*/CP_*.
set -euo pipefail

OLD="$(cd "$(dirname "$(realpath "$0")")/.." && pwd)"
NEW="$(dirname "$OLD")/hangar"
SD="$HOME/.config/systemd/user"

log() { printf '\033[36m==>\033[0m %s\n' "$*"; }

# ── 0. O clone está no commit do rename? ─────────────────────────────────────
# Sem isto, o passo 4 chama services-setup.sh e ele reescreve as units com o nome ANTIGO —
# a migração "termina bem" e a máquina volta pro estado anterior sem avisar.
if ! grep -q "hangar-backend" "$OLD/scripts/services-setup.sh" 2>/dev/null; then
    echo "erro: este clone ainda não tem o commit do rename (services-setup.sh não menciona" >&2
    echo "      hangar-backend). Rode 'git pull' antes — senão o passo das units desfaz tudo." >&2
    exit 1
fi

# ── 1. Renomear a pasta ──────────────────────────────────────────────────────
if [[ "$OLD" == "$NEW" ]]; then
    log "pasta já se chama hangar"
else
    if command -v tmux >/dev/null && tmux list-panes -a -F '#{pane_current_path}' 2>/dev/null | grep -q "^$OLD"; then
        echo "AVISO: há painéis tmux com cwd dentro de $OLD — os shells deles vão ficar órfãos após o rename." >&2
    fi
    [[ -e "$NEW" ]] && { echo "erro: $NEW já existe — resolva antes." >&2; exit 1; }
    mv "$OLD" "$NEW"
    log "pasta: $OLD -> $NEW"
fi
cd "$NEW"

# ── 1b. Worktrees ────────────────────────────────────────────────────────────
# Cada worktree guarda caminho ABSOLUTO nos dois sentidos (o .git dela aponta pro repo, e o repo
# lista o caminho dela em .git/worktrees/*/gitdir). Depois do mv as duas pontas apontam pro caminho
# que não existe mais, e qualquer comando git de dentro delas falha. `repair` reescreve os dois
# lados. Medido nesta máquina: 9 worktrees registradas na hora da migração.
if [[ "$OLD" != "$NEW" ]] && git worktree list >/dev/null 2>&1; then
    git worktree repair 2>/dev/null || true
    for wt in $(git worktree list --porcelain | awk '/^worktree /{print $2}'); do
        [[ "$wt" == "$NEW" ]] && continue
        git -C "$wt" worktree repair 2>/dev/null || true
    done
    log "worktrees reparadas ($(git worktree list | wc -l) registradas)"
fi

# ── 2. Remote ────────────────────────────────────────────────────────────────
if git remote get-url origin | grep -q "claude-cockpit"; then
    git remote set-url origin "$(git remote get-url origin | sed 's/claude-cockpit/hangar/')"
fi
log "remote: $(git remote get-url origin)"

# ── 3. Paths absolutos antigos em configs do usuário ─────────────────────────
# TODOS os perfis do Claude (~/.claude, ~/.claude-work, …), não só o ativo: hooks e statusline
# são instalados por PERFIL, e um settings.json apontando pro path velho TRAVA TODA TOOL da
# sessão que usa aquele perfil.
if [[ "$OLD" != "$NEW" ]]; then
    targets=("$HOME/.tmux.conf")
    for d in "$HOME"/.claude*/; do
        [[ -d "$d" ]] || continue
        for f in settings.json settings.local.json; do
            [[ -f "$d$f" ]] && targets+=("$d$f")
        done
    done
    for f in "${targets[@]}"; do
        if [[ -f "$f" ]] && grep -qF "$OLD" "$f" 2>/dev/null; then
            sed -i "s|$OLD|$NEW|g" "$f"
            log "paths corrigidos em $f"
        fi
    done
fi

# ── 3b. Memórias E TRANSCRIPTS do projeto ────────────────────────────────────
# A pasta do projeto é chaveada pelo PATH (projects/<path-com-hifens>/), então após o rename o slug
# novo nasce VAZIO e o conteúdo antigo fica órfão — some sem erro nenhum.
#
# São DUAS coisas ali dentro, e a segunda é a maior: `memory/` (as memórias) e os `.jsonl` na raiz,
# que são o HISTÓRICO das conversas. Sem levar os .jsonl, `claude --resume` não encontra nenhuma
# conversa anterior e o app não mostra o histórico delas — medido nesta máquina: 67 arquivos na hora
# da migração. Copia (nunca move: original intacto) e corrige paths absolutos nas memórias.
#
# Os .jsonl NÃO são reescritos de propósito: são registro do que aconteceu, com o caminho que era
# verdade na época; reescrever ali seria falsificar transcript.
if [[ "$OLD" != "$NEW" ]]; then
    old_slug="${OLD//\//-}"
    new_slug="${NEW//\//-}"
    for d in "$HOME"/.claude*/; do
        op="$d/projects/$old_slug"
        [[ -d "$op" ]] || continue
        np="$d/projects/$new_slug"
        mkdir -p "$np"

        # Duas políticas de cópia, e a diferença importa quando o script roda COM uma sessão viva:
        #   - o resto (memory/, sidecars) vai com -n: já foi copiado e teve os paths corrigidos,
        #     sobrescrever desfaria a correção;
        #   - os .jsonl vão com -u (atualiza se a origem for mais nova), porque a sessão que roda
        #     este script continua ESCREVENDO no slug antigo até morrer. Com -n, a cauda daquela
        #     conversa nunca chegaria no slug novo, nem re-rodando. Assim, uma segunda passada
        #     depois de encerrar as sessões completa o histórico.
        n_jsonl=$(find "$op" -maxdepth 1 -name '*.jsonl' 2>/dev/null | wc -l)
        cp -rn "$op"/. "$np"/ 2>/dev/null || true
        find "$op" -maxdepth 1 -name '*.jsonl' -exec cp -u {} "$np"/ \; 2>/dev/null || true
        log "projeto copiado: $op -> $np (${n_jsonl} transcript(s))"

        # memórias: além de copiadas, têm os caminhos absolutos corrigidos
        if [[ -d "$np/memory" ]]; then
            grep -rlF "$OLD" "$np/memory" 2>/dev/null | while read -r m; do sed -i "s|$OLD|$NEW|g" "$m"; done
            echo "    (confira nomes de unit 'claude-cockpit-*' no texto delas — não são reescritos)"
        fi
    done
fi

# ── 3c. Lançador do desktop (Electron) ───────────────────────────────────────
# O .desktop instalado guarda o caminho ABSOLUTO expandido no install (Exec, Path e agora Icon).
# Sem isto o lançador aponta pra pasta que não existe mais e o app some do menu.
APPS="$HOME/.local/share/applications"
for antigo in "$APPS/claude-cockpit.desktop" "$APPS/hangar.desktop"; do
    [[ -f "$antigo" ]] || continue
    sed "s|$OLD|$NEW|g" "$antigo" > "$APPS/hangar.desktop.novo"
    mv "$APPS/hangar.desktop.novo" "$APPS/hangar.desktop"
    [[ "$antigo" == "$APPS/hangar.desktop" ]] || rm -f "$antigo"
    command -v update-desktop-database >/dev/null && update-desktop-database "$APPS" 2>/dev/null || true
    log "lançador migrado: $APPS/hangar.desktop"
done

# ── 4. Units systemd ─────────────────────────────────────────────────────────
if command -v systemctl >/dev/null && [[ -f "$SD/claude-cockpit-backend.service" ]]; then
    log "trocando units claude-cockpit-* por hangar-*"
    # A TOPOLOGIA da maquina vai junto: o services-setup.sh decide quem serve a interface olhando se
    # existe unit de frontend NO DISCO — e nos acabamos de apagar a antiga. Sem CP_SERVE explicito
    # ele conclui "instalacao nova" e passa a servir a UI pelo backend, matando o 5173.
    # Numa maquina atras de reverse proxy apontado pro 5173 isso derruba o site inteiro: medido em
    # 09/08/2026 na VPS, 4 minutos de 502 no celular do usuario. Migracao renomeia; nao muda desenho.
    if [[ -f "$SD/claude-cockpit-frontend.service" ]]; then
        export CP_SERVE=preview
        log "topologia preservada: esta maquina tinha servico de frontend proprio (5173)"
    fi
    systemctl --user disable --now claude-cockpit-backend.service claude-cockpit-frontend.service 2>/dev/null || true
    rm -f "$SD/claude-cockpit-backend.service" "$SD/claude-cockpit-frontend.service"
    ./scripts/services-setup.sh        # escreve/sobe as hangar-* a partir do path novo

# Units JA renomeadas, mas com o path VELHO dentro — e este ramo custou um apagao.
# Medido em 09/08/2026 nesta maquina: as units viraram hangar-* antes da pasta ser renomeada (uma
# sessao fez o rename do projeto, outra o da pasta). Como o `if` acima so entra quando encontra
# `claude-cockpit-backend.service`, ele nao rodou; o `mv` seguiu em frente e as units ficaram
# apontando pra uma pasta que nao existe mais. O systemd nao "falha" de um jeito obvio: ele fica em
# `activating`, reiniciando em loop com `status=200/CHDIR`, e o app some do celular sem nenhuma
# linha vermelha na saida da migracao — que terminou dizendo "migracao concluida".
# Detectar pelo NOME da unit nao serve; e comparar com "$OLD" tambem nao — numa maquina ja migrada
# $OLD e $NEW sao o MESMO caminho, entao o grep casaria o path novo e reescreveria a unit toda vez
# (falso positivo medido na 2a passada). O criterio honesto e o unico que descreve o defeito:
# o WorkingDirectory declarado na unit APONTA PRA UM DIRETORIO QUE NAO EXISTE.
elif command -v systemctl >/dev/null && [[ -f "$SD/hangar-backend.service" ]]; then
    wd="$(grep -m1 '^WorkingDirectory=' "$SD/hangar-backend.service" 2>/dev/null | cut -d= -f2-)"
    if [[ -n "$wd" && ! -d "$wd" ]]; then
        log "unit hangar-backend aponta pra '$wd', que nao existe — reescrevendo a partir de $NEW"
        [[ -f "$SD/hangar-frontend.service" ]] && export CP_SERVE=preview
        ./scripts/services-setup.sh
    fi
fi
if [[ -f "$SD/claude-cockpit-deploy.service" ]]; then
    systemctl --user disable claude-cockpit-deploy.service 2>/dev/null || true
    sed "s|$OLD|$NEW|g" "$SD/claude-cockpit-deploy.service" > "$SD/hangar-deploy.service"
    rm -f "$SD/claude-cockpit-deploy.service"
    systemctl --user daemon-reload
    log "unit de deploy migrada pra hangar-deploy.service"
fi

# ── 5. Re-instalar symlinks (cp-send, skills, painel) ────────────────────────
./scripts/install-cp-send.sh
if command -v qs >/dev/null && pgrep -x Hyprland >/dev/null; then
    ./scripts/install-cp-panel.sh
fi

# ── Verificação ──────────────────────────────────────────────────────────────
# FALHA ALTO, e nao com AVISO. Em 09/08/2026 esta verificacao viu o backend fora (units com o path
# velho, `activating` em loop de CHDIR), imprimiu duas linhas de aviso no meio da saida e terminou
# com "migracao concluida" e exit 0 — enquanto o app estava fora do ar no celular. Migracao que
# derruba o servico e diz que terminou bem e pior do que migracao que para: o dono so descobre
# quando vai usar, longe do terminal onde a mensagem passou.
echo
falhou=0

if command -v systemctl >/dev/null && [[ -f "$SD/hangar-backend.service" ]]; then
    # `is-active` responde `activating` num servico em loop de restart, e isso NAO e sucesso —
    # exigir a palavra `active` e o que separa "subiu" de "esta tentando subir pra sempre".
    sleep 3   # o systemd acabou de receber a unit; da tempo do primeiro bind
    estado="$(systemctl --user is-active hangar-backend.service 2>/dev/null || true)"
    if [[ "$estado" == "active" ]]; then
        log "hangar-backend ativo"
    else
        echo "ERRO: hangar-backend está '$estado' — veja: journalctl --user -u hangar-backend.service -n 30" >&2
        falhou=1
    fi
fi

# A prova que importa e a porta respondendo, nao a unit existir: um backend com WorkingDirectory
# errado fica `activating` e nunca escuta.
porta="$(grep -m1 '^CP_PORT=' "$NEW/backend/.env" 2>/dev/null | cut -d= -f2)"
porta="${porta:-8765}"
if command -v curl >/dev/null; then
    codigo="$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "http://127.0.0.1:$porta/" 2>/dev/null || true)"
    if [[ "$codigo" =~ ^(200|401|404)$ ]]; then
        log "backend respondendo em 127.0.0.1:$porta (HTTP $codigo)"
    else
        echo "ERRO: backend não respondeu em 127.0.0.1:$porta (curl: '${codigo:-sem resposta}')" >&2
        falhou=1
    fi
fi

if command -v cp-send >/dev/null; then
    cp-send --list >/dev/null 2>&1 && log "cp-send ok" \
        || { echo "ERRO: cp-send --list falhou — symlink quebrado ou backend fora" >&2; falhou=1; }
fi

if [[ "$falhou" -ne 0 ]]; then
    echo >&2
    echo "MIGRAÇÃO INCOMPLETA: a pasta foi renomeada mas o serviço não está no ar." >&2
    echo "  Conserto mais provável (units com o caminho antigo dentro):  ./scripts/services-setup.sh" >&2
    exit 1
fi
log "migração concluída. Clone em: $NEW"
