# hangar — `kimi` wrapper (fish). Installed by scripts/install-claude-wrapper.sh.
#
# Makes every interactive `kimi` opened in a terminal a Hangar session, same as the app opens.
# O Kimi cria o ID no primeiro prompt. O hook `hangar-server kimi-state-hook` herda
# TMUX_PANE e grava o bilhete que liga o pane à sessão.
#
# Rules (same as the posix version):
#  - subcommand as the FIRST argument -> raw (export/provider/acp/web/server/login/doctor/vis/
#    migrate/upgrade/update). Matched on $1 only, so `kimi "export the function"` stays an
#    interactive launch with an initial prompt. Same rule as the pi/codex wrappers.
#  - a flag that makes kimi print and exit anywhere in the args -> raw: -p/--prompt,
#    --output-format, --help/-h, --version/-V. Wrapping these would open a TUI that never prints.
#  - resume flags (-S/--session, -c/--continue) -> raw.
#  - outside tmux + terminal -> `hangar-open kimi …`: the Hangar launcher opens the session and
#    the terminal attaches to it. An argument the request doesn't cover, no hangar-server or no hangar-open -> a tmux session named after the folder.
#  - already in tmux ($TMUX) / stdin not a tty (pipe/script) -> raw: the hook ticket links the pane.
#
# Escape hatch: `command kimi ...` runs the raw binary, bypassing this wrapper.
function kimi
    # subcomando: só vale como PRIMEIRO argumento.
    if test (count $argv) -gt 0
        switch $argv[1]
            case export provider acp web server login doctor vis migrate upgrade update
                command kimi $argv
                return
        end
    end

    # usos não interativos primeiro: o kimi imprime e sai, não tem TUI pra envolver em tmux.
    for a in $argv
        switch $a
            case -p --prompt '--prompt=*' --output-format '--output-format=*' --help -h --version -V
                command kimi $argv
                return
        end
    end

    # Daqui pra baixo abre TUI: refaz a ponte de skills agora, pra skill instalada no Claude valer
    # já nesta abertura (antes dependia de reiniciar o backend). Silencioso e fail-soft.
    command -q hangar-skills-sync; and hangar-skills-sync

    # Retomada (-S/--session, -c/--continue): CLI cru.
    for a in $argv
        switch $a
            case -S --session '--session=*' -c --continue
                command kimi $argv
                return
        end
    end

    # TMUX herdado pode estar MORTO (mesmo caso dos outros wrappers). Valida o pane; stale -> limpa.
    if set -q TMUX
        if not set -q TMUX_PANE; or not tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1
            set -e TMUX TMUX_PANE
        end
    end

    # Fora do tmux e em terminal: a sessão nasce pelo lançador do Hangar (ver claude.fish).
    if not set -q TMUX; and isatty stdin
        if command -q hangar-open
            hangar-open kimi $argv
            set -l rc $status
            test $rc -eq 100; or return $rc
        else
            echo "hangar: hangar-open não encontrado; abrindo o kimi sem o Hangar" >&2
        end

        # Sem o tmux o CLI ficaria invisível pro app: sessão com o nome da pasta, única.
        # Acento -> ASCII antes do filtro, mesma regra do backend (app/names.py).
        set -l base (basename "$PWD")
        if command -v iconv >/dev/null 2>&1
            set -l ascii (printf '%s' "$base" | iconv -f UTF-8 -t ASCII//TRANSLIT 2>/dev/null)
            test $status -eq 0; and test -n "$ascii"; and set base $ascii
        end
        set base (string replace -ra '[^A-Za-z0-9_-]' '-' -- $base)
        set base (string replace -ra '^-+|-+$' '' -- $base)
        test -n "$base"; or set base session
        set -l name $base
        set -l i 2
        while tmux has-session -t "=$name" 2>/dev/null
            set name "$base-$i"
            set i (math $i + 1)
        end
        # Chamada duplicada: variável vazia na posição de comando é erro fatal no fish.
        # CP_SESSION_NAME: identidade da sessão pro hangar-send de dentro do pane.
        # KIMI_CODE_TUI_FULL_SCREEN=1: mesma TUI das sessões que o app abre.
        if command -q systemd-run; and set -q XDG_RUNTIME_DIR; and systemd-run --user --scope --collect -q -- true >/dev/null 2>&1
            systemd-run --user --scope --collect -q -- tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                -e "KIMI_CODE_TUI_FULL_SCREEN=1" \
                sh -c 'exec kimi "$@"' _ $argv
        else
            tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                -e "KIMI_CODE_TUI_FULL_SCREEN=1" \
                sh -c 'exec kimi "$@"' _ $argv
        end
        return
    end

    # já no tmux ou stdin sem tty: nada a injetar (o bilhete do hook liga
    # pane->sessão).
    command kimi $argv
end
