#!/usr/bin/env bash
# Confere qual passo cada mudança dispara no scripts/verificar-local (a parte com mais ramos dele).
# Usage: ./scripts/test-verificar-local.sh
set -uo pipefail

V="$(dirname "$0")/verificar-local"
falhou=0
caso() {   # $1 = arquivos (um por linha), $2 = linux esperado, $3 = windows esperado
    local saida lin win
    saida="$(printf '%s\n' "$1" | "$V" --classificar)"
    lin="$(sed -n 's/^linux: //p' <<< "$saida")"
    win="$(sed -n 's/^windows: //p' <<< "$saida")"
    if [[ "$lin" != "$2" || "$win" != "$3" ]]; then
        printf 'FALHOU %s\n  linux:   "%s" (esperado "%s")\n  windows: "%s" (esperado "%s")\n' \
            "$(tr '\n' ' ' <<< "$1")" "$lin" "$2" "$win" "$3"
        falhou=1
    fi
}

caso "crates/hangar-server/src/main.rs" "rust backend" "rust"
caso "crates/hangar-api/src/lib.rs" "rust backend nativo" "rust"
caso "backend/app/registry.py" "backend" "runtime"
caso "backend/app/runtime_terminal.py" "rust backend" "rust"
caso "frontend/src/App.svelte" "backend front" ""
caso "packages/core/src/api.ts" "backend front mobile" ""
caso "mobile/src/App.tsx" "backend mobile" ""
caso "desktop-native/src/main.rs" "nativo" ""
caso "install.ps1" "backend" "runtime"
caso "scripts/shell/claude.fish" "backend shell" ""
caso "scripts/hooks/pre-push" "backend shell" ""
caso "scripts/configure-statusline.cjs" "backend statusline" "statusline"
caso ".github/workflows/server.yml" "" ""
caso "docs/migracao-rust/pedidos/x.md" "" ""
caso "docs/decisoes/instalacao.md" "backend" ""

# Erro no cálculo do plano nunca vira "nada a verificar".
"$V" --plano --base ref-que-nao-existe >/dev/null 2>&1 && { echo "FALHOU --plano com base inválida saiu 0"; falhou=1; }
"$V" --exigir 0000000000000000000000000000000000000000 >/dev/null 2>&1 && { echo "FALHOU --exigir com commit inválido saiu 0"; falhou=1; }

# Hook do `gh pr create` (.claude/settings.json): só age no comando dele, aceita o escape e recusa
# com frase quando não consegue resolver a base.
hook() {   # $1 = comando, $2 = código esperado
    local rc
    printf '{"cwd":"%s","tool_input":{"command":"%s"}}' "$PWD" "$1" | node "$(dirname "$0")/hook-pr-verificado.mjs" 2>/dev/null
    rc=$?
    (( rc == $2 )) || { echo "FALHOU hook \"$1\": saiu $rc (esperado $2)"; falhou=1; }
}
hook "git status" 0
hook "git commit -m 'o hook do gh pr create --base now'" 0
hook "printf '%s' 'x && gh pr create --base nao-existe'" 0
hook "HANGAR_SEM_VERIFICACAO=1 gh pr create --base main" 0
if [[ "$(uname -s)" == Linux ]]; then
    hook "gh pr create --base base-que-nao-existe" 2
    hook "true && gh pr create --base base-que-nao-existe" 2
    hook "(cd . && FOO=1 gh pr create --base base-que-nao-existe)" 2
    hook "if true; then gh pr create --base base-que-nao-existe; fi" 2
    hook "gh -R dono/repo pr create --base base-que-nao-existe" 2
    hook "cd / && gh pr create --base base-que-nao-existe" 0
    hook "gh pr create --base 'base-que-nao-existe' --body 'x'" 2
    hook "gh pr create --body 'usa --base base-que-nao-existe' --help" 0
    hook "gh pr create --base main --head main --body 'cita --base base-que-nao-existe'" 0
fi

"$V" --plano --passos inexistente >/dev/null 2>&1 && { echo "FALHOU --passos desconhecido saiu 0"; falhou=1; }
HANGAR_VERIFICAR_PROCESSOS=0 "$V" --plano >/dev/null 2>&1 && { echo "FALHOU com 0 processos saiu 0"; falhou=1; }
# Registro por passo: o que já passou nesta árvore sai do plano, e --de-novo o devolve.
estado="$(mktemp -d)"
mkdir -p "$estado/hangar-verificacao/resultados"
printf 'passos=rust\n' > "$estado/hangar-verificacao/resultados/$(git rev-parse 'HEAD^{tree}')-linux"
plano="$(XDG_STATE_HOME="$estado" "$V" --plano --tudo --passos "rust shell" 2>&1 | sed -n 's/^linux: *//p')"
[[ "$plano" == "shell" ]] || { echo "FALHOU não pulou o passo gravado: \"$plano\""; falhou=1; }
plano="$(XDG_STATE_HOME="$estado" "$V" --plano --tudo --passos "shell rust" --de-novo 2>&1 | sed -n 's/^linux: *//p')"
[[ "$plano" == "rust shell" ]] || { echo "FALHOU --de-novo ou ordem do plano: \"$plano\""; falhou=1; }
rm -rf "$estado"

(( falhou )) && exit 1
echo "ok: classificação do verificar-local"
