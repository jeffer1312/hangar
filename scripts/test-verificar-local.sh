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
caso "packages/core/src/api.ts" "backend front" ""
caso "messages/pt.json" "backend front nativo" ""
caso "mobile/src/App.tsx" "backend mobile" ""
caso "desktop-native/src/main.rs" "nativo" ""
caso "install.ps1" "backend" "runtime"
caso "scripts/shell/claude.fish" "backend shell" ""
caso "scripts/hooks/pre-push" "backend shell" ""
caso "scripts/configure-statusline.cjs" "backend statusline" "statusline"
caso ".github/workflows/server.yml" "" ""
caso "docs/migracao-rust/pedidos/x.md" "" ""
caso "docs/decisoes/instalacao.md" "backend" ""

# Plano rápido (o padrão, que as travas exigem): só o ligado aos arquivos mudados; Windows só com
# arquivo de Windows; documentação não roda nada.
rapido() {   # $1 = arquivos, $2 = linux esperado, $3 = windows esperado
    local saida lin win
    saida="$(printf '%s\n' "$1" | "$V" --classificar-rapido)"
    lin="$(sed -n 's/^linux: //p' <<< "$saida")"
    win="$(sed -n 's/^windows: //p' <<< "$saida")"
    if [[ "$lin" != "$2" || "$win" != "$3" ]]; then
        printf 'FALHOU rápido %s\n  linux:   "%s" (esperado "%s")\n  windows: "%s" (esperado "%s")\n' \
            "$(tr '\n' ' ' <<< "$1")" "$lin" "$2" "$win" "$3"
        falhou=1
    fi
}
rapido "crates/hangar-server/src/main.rs" "rust_rapido" ""
rapido "crates/hangar-api/src/lib.rs" "rust_rapido nativo" ""
rapido "backend/app/registry.py" "backend_rapido" ""
rapido "backend/app/procinfo.py" "backend_rapido" "runtime"
rapido "frontend/src/App.svelte" "front_rapido" ""
rapido "packages/core/src/api.ts" "front_rapido" ""
rapido "messages/pt.json" "front_rapido nativo" ""
rapido "mobile/src/App.tsx" "mobile_rapido" ""
rapido "desktop-native/src/main.rs" "nativo" ""
rapido "install.ps1" "" "runtime"
rapido "scripts/shell/claude.fish" "shell" ""
rapido "scripts/configure-statusline.cjs" "statusline" "statusline"
rapido "docs/decisoes/instalacao.md" "" ""
rapido $'crates/hangar-server/src/a.rs\nfrontend/src/b.ts\ndocs/x.md' "rust_rapido front_rapido" ""

# O passo completo já gravado cobre o rápido da mesma área; o rápido não cobre o completo.
reg="$(mktemp)"
printf 'passos=rust nativo\n' > "$reg"
falta="$("$V" --faltando "$reg" rust_rapido backend_rapido)"
[[ "$falta" == "backend_rapido" ]] || { echo "FALHOU completo não cobriu o rápido: \"$falta\""; falhou=1; }
printf 'passos=rust_rapido\n' > "$reg"
falta="$("$V" --faltando "$reg" rust)"
[[ "$falta" == "rust" ]] || { echo "FALHOU rápido cobriu o completo: \"$falta\""; falhou=1; }
# O rápido olhou o diff desde a base dele: base mais nova que a pedida cobre menos e não vale.
# Repositório próprio com dois commits: clone raso do CI não tem HEAD~1.
Vabs="$(realpath "$V")"
hist="$(mktemp -d)"
(cd "$hist" && export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 && git init -q && \
    git -c user.name=t -c user.email=t@t commit -q --allow-empty -m a && \
    git -c user.name=t -c user.email=t@t commit -q --allow-empty -m b)
velho="$(git -C "$hist" rev-parse HEAD~1)" novo="$(git -C "$hist" rev-parse HEAD)"
faltando_em() { (cd "$hist" && BASE_ATUAL="$1" "$Vabs" --faltando "$reg" rust_rapido); }
printf 'base=%s\npassos=rust_rapido\n' "$novo" > "$reg"
falta="$(faltando_em "$velho")"
[[ "$falta" == "rust_rapido" ]] || { echo "FALHOU rápido de base mais nova valeu: \"$falta\""; falhou=1; }
falta="$(faltando_em "$novo")"
[[ -z "$falta" ]] || { echo "FALHOU rápido da mesma base não valeu: \"$falta\""; falhou=1; }
printf 'base=%s\npassos=rust_rapido\n' "$velho" > "$reg"
falta="$(faltando_em "$novo")"
[[ -z "$falta" ]] || { echo "FALHOU rápido de base mais velha (diff maior) não valeu: \"$falta\""; falhou=1; }
rm -rf "$reg" "$hist"
"$V" --plano --tudo --passos rust_rapido >/dev/null 2>&1 && { echo "FALHOU --tudo aceitou passo rápido"; falhou=1; }

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

# Poda do crates/target acima do teto: primeiro o incremental, depois os executáveis de teste mais
# velhos (voltam com um link); apagar tudo só se ainda não couber.
alvo() {   # monta um target falso: incremental 64 KB, rlib 64 KB, executáveis velho e novo de 64 KB
    local d; d="$(mktemp -d)"
    mkdir -p "$d/debug/incremental/x" "$d/debug/deps"
    for f in debug/incremental/x/a debug/deps/libdep-1.rlib debug/deps/velho-1 debug/deps/novo-1; do
        head -c 65536 /dev/urandom > "$d/$f"
    done
    chmod +x "$d/debug/deps/velho-1" "$d/debug/deps/novo-1"
    touch -d '3 days ago' "$d/debug/deps/velho-1"
    echo "$d"
}
podar() {   # $1 = teto em KB, $2 = arquivos que têm de sobrar (relativos a debug/)
    local d sobrou
    d="$(alvo)"
    "$V" --podar "$d" "$1" >/dev/null 2>&1
    sobrou="$( (cd "$d/debug" 2>/dev/null && find . -type f | sed 's|^\./||' | sort | tr '\n' ' ') )"
    [[ "$sobrou" == "$2" ]] || { echo "FALHOU poda com teto $1 KB: sobrou \"$sobrou\" (esperado \"$2\")"; falhou=1; }
    rm -rf "$d"
}
podar 1024 "deps/libdep-1.rlib deps/novo-1 deps/velho-1 incremental/x/a "
podar 220 "deps/libdep-1.rlib deps/novo-1 deps/velho-1 "
podar 160 "deps/libdep-1.rlib deps/novo-1 "
podar 32 ""

# Memória curta põe as faixas em fila: limite padrão 6 GB, ajustável; leitura falha conta como curta.
memoria() {   # $1 = MemAvailable em kB ("" = sem a linha), $2 = limite em GB ("" = padrão), $3 = curta|folga
    local f r; f="$(mktemp)"
    printf 'MemTotal: 24000000 kB\n' > "$f"
    [[ -n "$1" ]] && printf 'MemAvailable: %s kB\n' "$1" >> "$f"
    if HANGAR_VERIFICAR_MEMINFO="$f" HANGAR_VERIFICAR_MEM_GB="$2" "$V" --memoria-curta; then r=curta; else r=folga; fi
    [[ "$r" == "$3" ]] || { echo "FALHOU memória $1 kB, limite \"$2\": $r (esperado $3)"; falhou=1; }
    rm -f "$f"
}
memoria 2000000 "" curta
memoria 20000000 "" folga
memoria 2000000 1 folga
memoria "" "" curta

# Andamento: repassa tudo e só as linhas de passo vão ao arquivo, com a hora na frente.
anda="$(mktemp)"
saida="$(printf '>> linux: rust…\ncompilando\n<< linux: rust 12s ok\n' | "$V" --marcar "$anda")"
[[ "$saida" == $'>> linux: rust…\ncompilando\n<< linux: rust 12s ok' ]] || { echo "FALHOU --marcar não repassou a entrada: \"$saida\""; falhou=1; }
[[ "$(sed 's/^[0-9]* //' "$anda" | tr '\n' '|')" == ">> linux: rust…|<< linux: rust 12s ok|" ]] || \
    { echo "FALHOU --marcar gravou: \"$(cat "$anda")\""; falhou=1; }
# Última linha que não é de passo (o resumo do Windows): o filtro sai com 0, senão a VM conta como falha.
printf '<< windows: rust ok\n  ok      rust 1s\n' | "$V" --marcar "$anda" > /dev/null || \
    { echo "FALHOU --marcar saiu com erro numa última linha que não é de passo"; falhou=1; }
rm -f "$anda"

(( falhou )) && exit 1
echo "ok: classificação do verificar-local"
