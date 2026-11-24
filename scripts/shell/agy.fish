# hangar — `agy` wrapper (fish). Installed by scripts/install-claude-wrapper.sh.
#
# Makes every interactive `agy` opened in a terminal a Hangar session, same as the app opens.
# O agy cria o ID no primeiro prompt. O hook `hangar-server agy-state-hook` herda
# TMUX_PANE e grava o bilhete que liga o pane à sessão.
#
# Rules (same as the posix version):
#  - subcommand as the FIRST argument -> raw (acp/agent/agents/changelog/help/install/mcp/
#    mic-serve/models/plugin/plugins/remote-control/update). Matched on $1 only, so `agy "update the docs"`
#    stays an interactive launch with an initial prompt. Same rule as the pi/codex wrappers.
#  - a flag that makes agy print and exit anywhere in the args -> raw: -p/--print/--prompt,
#    --output-format, --input-format, --json-schema, --help/-h, --version/-v. Wrapping these would open a TUI that never prints.
#  - resume flags (--conversation, -c/--continue) -> raw.
#  - outside tmux + terminal -> `hangar-open antigravity …`: the Hangar launcher opens the session and
#    the terminal attaches to it. An argument the request doesn't cover, no hangar-server or no hangar-open -> a tmux session named after the folder.
#  - already in tmux ($TMUX) / stdin not a tty (pipe/script) -> raw: the hook ticket links the pane.
#
# Escape hatch: `command agy ...` runs the raw binary, bypassing this wrapper.
function agy
    # subcomando: só vale como PRIMEIRO argumento.
    if test (count $argv) -gt 0
        switch $argv[1]
            case acp agent agents changelog help install mcp mic-serve models plugin plugins remote-control update
                command agy $argv
                return
        end
    end

    # usos não interativos primeiro: o agy imprime e sai, não tem TUI pra envolver em tmux.
    for a in $argv
        switch $a
            case -p --print --prompt '--prompt=*' --output-format '--output-format=*' --input-format '--input-format=*' --json-schema '--json-schema=*' --help -h --version -v
                command agy $argv
                return
        end
    end

    # Daqui pra baixo abre TUI: refaz a ponte de skills agora, pra skill instalada no Claude valer
    # já nesta abertura (antes dependia de reiniciar o backend). Silencioso e fail-soft.
    command -q hangar-skills-sync; and hangar-skills-sync

    # Retomada (--conversation, -c/--continue): CLI cru.
    for a in $argv
        switch $a
            case --conversation '--conversation=*' -c --continue
                command agy $argv
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
            hangar-open antigravity $argv
            set -l rc $status
            test $rc -eq 100; or return $rc
        else
            echo "hangar: hangar-open não encontrado; abrindo o agy sem o Hangar" >&2
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
        if command -q systemd-run; and set -q XDG_RUNTIME_DIR; and systemd-run --user --scope --collect -q -- true >/dev/null 2>&1
            systemd-run --user --scope --collect -q -- tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'exec agy "$@"' _ $argv
        else
            tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'exec agy "$@"' _ $argv
        end
        return
    end

    # já no tmux ou stdin sem tty: nada a injetar (o bilhete do hook liga
    # pane->sessão).
    command agy $argv
end
