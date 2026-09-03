# hangar — `omp` wrapper (fish). Installed by scripts/install-claude-wrapper.sh.
#
# Mesmo papel do pi.fish, com UMA diferença que muda a forma: o omp NÃO tem `--session-id`.
# Quem escolhe a sessão é o CAMINHO do transcript (`--session <arquivo>`), então o wrapper monta o
# caminho igual ao backend (`backend/app/adapters/omp/adapter.py` -> `pi/sessions.transcript_alvo`)
# e exporta CP_PI_SESSION com o MESMO uuid do nome do arquivo — é por ele que o backend acha a
# sessão em /proc/<pid>/environ quando o bilhete da extensão ainda não existe (registry:_pi_sid_of).
#
# Regras (idênticas às do pi):
#  - flag que já gerencia a própria sessão (--session, --resume/-r, --continue/-c, --no-session,
#    --session-dir) -> NÃO monta caminho nenhum (sobrepor `--session` a um `omp -c` abriria sessão
#    FRESCA em vez de continuar), mas AINDA envolve em tmux quando interativo e fora dele: sessão
#    retomada tem que continuar visível no app, e o rastreio não depende do uuid injetado — a
#    hangar-state.ts publica o arquivo real por dentro do omp.
#  - uso não interativo -> cru. Duas formas: (a) o PRIMEIRO argumento é um subcomando do omp
#    (`omp models` lista modelos; `omp "models are slow"` continua sendo prompt inicial);
#    (b) uma flag que imprime e sai (-p/--print, --export, --help/-h, --version/-v).
#  - já em tmux ($TMUX) / stdin não é tty -> só o --session + o export.
#  - fora do tmux + interativo -> cria sessão tmux com o BASENAME da pasta (sufixo -2/-3 se já
#    existir) e roda o omp dentro dela.
#
# CP_PI_SESSION é EXPORTADO, nunca passado por `tmux -e`: um servidor tmux já rodando não entrega o
# ambiente do cliente a um pane novo, então o export tem que acontecer DENTRO do processo do pane —
# daí o `sh -c` do ramo que cria sessão (mesmo desenho do pi.fish).
#
# Escape hatch: `command omp ...` roda o binário cru, sem este wrapper.

# Mesmo caminho que backend/app/adapters/pi/sessions.py:transcript_alvo monta pro `--session`:
# <raiz>/<slug do cwd>/<ts>_<id>.jsonl. O slug troca só o separador por '-' (acento e espaço passam).
function hangar_omp_alvo --argument-names id
    set -l raiz "$PI_CODING_AGENT_DIR"
    test -n "$raiz"; or set raiz "$HOME/.omp/agent"
    set -l slug "--"(string replace -r '^[/\\\\]' '' -- "$PWD" | string replace -ra '[/\\\\:]' '-')"--"
    printf '%s/sessions/%s/%s_%s.jsonl' "$raiz" "$slug" (date -u +%Y-%m-%dT%H-%M-%S-000Z) "$id"
end

function omp
    # subcomando: só vale como PRIMEIRO argumento (lista de `omp --help`, seção COMMANDS).
    if test (count $argv) -gt 0
        switch $argv[1]
            case acp agents auth-broker auth-gateway bench browser-relay cleanse commit completions \
                 compress config dry-balance gallery gc git grep grievances if-bench images install \
                 join models plugin ps read render say search setup share shell ssh stats \
                 tiny-models token ttsr update usage worktree
                command omp $argv
                return
        end
    end

    # usos não interativos: o omp imprime e sai, não tem TUI pra envolver em tmux.
    for a in $argv
        switch $a
            case -p --print --export '--export=*' --help -h --version -v
                command omp $argv
                return
        end
    end

    set -l own_session 0
    for a in $argv
        switch $a
            case --session '--session=*' --resume '--resume=*' -r --continue -c --no-session --session-dir '--session-dir=*'
                set own_session 1
                break
        end
    end

    # Substituição que falha deixa a variável como LISTA VAZIA no fish (não como string vazia): sem o
    # fallback — o mesmo do omp.posix.sh — uma máquina sem uuidgen faria o nome do transcript nascer
    # sem uuid nenhum, e o backend nunca acharia o arquivo pelo glob `*_<uuid>.jsonl`.
    set -l id (uuidgen 2>/dev/null)
    test -n "$id"; or set id (cat /proc/sys/kernel/random/uuid 2>/dev/null)

    # TMUX herdado pode estar MORTO (kitty single-instance cujo mestre nasceu num pane já fechado).
    if set -q TMUX
        if not set -q TMUX_PANE; or not tmux list-panes -t "$TMUX_PANE" >/dev/null 2>&1
            set -e TMUX TMUX_PANE
        end
    end

    if set -q TMUX; or not isatty stdin
        if test $own_session -eq 1
            command omp $argv
            return
        end
        set -x CP_PI_SESSION $id
        command omp --session (hangar_omp_alvo "$id") $argv
        return
    end

    # Mesma regra do backend (app/names.py) e do wrapper claude: acento -> ASCII antes do filtro.
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

    # NAO usar `$run tmux ...` com $run possivelmente vazio: em fish, variavel vazia na posicao de
    # COMANDO e erro fatal, ao contrario de bash/zsh. Por isso a chamada e duplicada nos dois ramos,
    # igual ao pi.fish, que documenta a mesma escolha.
    # `sh -c` exporta CP_PI_SESSION DENTRO do pane antes do exec — ver o cabeçalho pra o porquê.
    # $0 vira "_" (placeholder), $1 o uuid, $2 o caminho do transcript, o resto os args originais.
    # CP_SESSION_NAME: carimbo de identidade pro hangar-send de dentro do pane (ver claude.fish).
    if command -q systemd-run; and set -q XDG_RUNTIME_DIR; and systemd-run --user --scope --collect -q -- true >/dev/null 2>&1
        if test $own_session -eq 1
            systemd-run --user --scope --collect -q -- tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'exec omp "$@"' _ $argv
        else
            systemd-run --user --scope --collect -q -- tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'export CP_PI_SESSION="$1"; alvo="$2"; shift 2; exec omp --session "$alvo" "$@"' \
                _ "$id" (hangar_omp_alvo "$id") $argv
        end
    else
        if test $own_session -eq 1
            tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'exec omp "$@"' _ $argv
        else
            tmux new-session -s $name -c "$PWD" -e "CP_SESSION_NAME=$name" \
                sh -c 'export CP_PI_SESSION="$1"; alvo="$2"; shift 2; exec omp --session "$alvo" "$@"' \
                _ "$id" (hangar_omp_alvo "$id") $argv
        end
    end
end
