#!/usr/bin/env bash
# Confere qual passo cada mudança dispara no scripts/verificar-local (a parte com mais ramos dele).
# Usage: ./scripts/test-verificar-local.sh
set -uo pipefail

V="$(dirname "$0")/verificar-local"
falhou=0

# O preparo conjunto não alterna grafos de features entre builds do servidor.
(
    verifier_path="$(realpath "$V")"
    preparation_root="$(mktemp -d)"
    trap 'rm -rf "$preparation_root"' EXIT
    mkdir -p "$preparation_root/crates" "$preparation_root/backend"
    preparation_log="$preparation_root/cargo.log"
    cargo() { printf '%s\n' "$*" >> "$preparation_log"; }
    uv() { :; }
    python3() { :; }
    pin_auth_pair() { :; }
    omp_de_teste() { :; }
    wait_for_memory() { printf 'espera %s\n' "$1" >> "$preparation_log"; }
    PLANO='rust backend'
    PROCESSOS=2
    CARGO_BUILD_JOBS=2
    MEM_BUILD_GB=11
    eval "$(sed -n '/^passo_preparo() {/,/^}/p' "$verifier_path")"
    cd "$preparation_root" || exit 1
    passo_preparo || exit 1
    # As bibliotecas de teste antes do resto: os dois rustc do hangar-server nunca compilam juntos.
    [[ "$(cat "$preparation_log")" == $'espera 11\ntest --locked --workspace --lib --no-run\nbuild --locked --workspace --all-targets' ]] || {
        echo 'FALHOU: preparo Rust/backend não compilou em fila, ou repetiu compilação'
        cat "$preparation_log"
        exit 1
    }
    # Sem memória o preparo para antes de compilar.
    : > "$preparation_log"
    wait_for_memory() { return 1; }
    passo_preparo && { echo 'FALHOU: preparo compilou sem memória'; exit 1; }
    [[ ! -s "$preparation_log" ]] || { echo 'FALHOU: preparo chamou o cargo sem memória'; cat "$preparation_log"; exit 1; }
    # Plano só de backend: o cargo muitas vezes não tem o que compilar, e o preparo não espera memória.
    PLANO='backend_rapido'
    passo_preparo || { echo 'FALHOU: preparo de backend esperou memória'; exit 1; }
    [[ "$(cat "$preparation_log")" == 'build --locked -p hangar-server -p hangar-auth' ]] || {
        echo 'FALHOU: preparo de backend não compilou o par'; cat "$preparation_log"; exit 1; }
) || falhou=1

# Na VM o roteiro é copiado sozinho: sem o memory-scope ao lado, roda direto.
alone="$(mktemp -d)"
cp "$V" "$alone/run.sh"
saida="$(printf 'crates/hangar-server/src/main.rs\n' | bash "$alone/run.sh" --classificar-rapido 2>&1)"
grep -qx "linux: rust_rapido" <<< "$saida" || { echo "FALHOU roteiro copiado sozinho: \"$saida\""; falhou=1; }
rm -rf "$alone"

# Rust no rápido: confere o workspace; testes só dos crates que não são o hangar-server.
quick_rust() {   # $1 = arquivos mudados, $2 = chamadas esperadas do cargo
    (
        d="$(mktemp -d)"; trap 'rm -rf "$d"' EXIT
        mkdir -p "$d/crates"
        cargo() { printf '%s\n' "$*" >> "$d/cargo.log"; }
        wait_for_memory() { :; }
        mudados() { grep -E "$1" <<< "$ARQUIVOS" || true; }
        ARQUIVOS="$1" MEM_CHECK_GB=6
        eval "$(sed -n '/^passo_rust_rapido() {/,/^}/p' "$(realpath "$V")")"
        cd "$d" && passo_rust_rapido > /dev/null || exit 1
        [[ "$(cat "$d/cargo.log" 2>/dev/null)" == "$2" ]] || { echo "FALHOU rust rápido de \"$1\": $(tr '\n' '|' < "$d/cargo.log")"; exit 1; }
    ) || falhou=1
}
# As bibliotecas em modo de teste antes do resto: as duas conferências do hangar-server em fila.
check_calls=$'check --locked --workspace --lib --profile test\ncheck --locked --workspace --all-targets'
quick_rust "crates/hangar-server/src/main.rs" "$check_calls"
quick_rust $'crates/hangar-server/src/main.rs\ncrates/hangar-core/src/lib.rs' \
    "$check_calls"$'\ntest --locked --no-fail-fast --workspace --exclude hangar-server'
quick_rust "crates/Cargo.lock" \
    "$check_calls"$'\ntest --locked --no-fail-fast --workspace --exclude hangar-server'
quick_rust "frontend/src/b.ts" ""
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
caso "scripts/memory-scope" "backend shell" ""
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
rapido "scripts/memory-scope" "shell" ""
rapido "scripts/test_memory_scope.py" "shell" ""
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

# Com a main juntada à branch, o que veio dela não entra no plano: só o que a branch mudou.
hist="$(mktemp -d)"
(cd "$hist" && export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 && git init -q -b main && \
    g() { git -c user.name=t -c user.email=t@t "$@"; } && \
    g commit -q --allow-empty -m base && git checkout -q -b tarefa && \
    mkdir -p crates/hangar-server mobile && touch crates/hangar-server/a.rs && git add . && g commit -q -m tarefa && \
    git checkout -q main && touch mobile/b.tsx && git add . && g commit -q -m main-andou && \
    git update-ref refs/remotes/origin/main main && git checkout -q tarefa && g merge -q --no-edit main)
plano="$(cd "$hist" && GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 "$Vabs" --plano 2>&1)"
grep -q rust_rapido <<< "$plano" && ! grep -q mobile_rapido <<< "$plano" || { echo "FALHOU main juntada entrou no plano: $plano"; falhou=1; }
# Arquivo que veio da main e a branch mudou depois é dela: entra no plano.
(cd "$hist" && export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 && echo x > mobile/b.tsx && git add . && \
    git -c user.name=t -c user.email=t@t commit -q -m tarefa-mexe)
plano="$(cd "$hist" && GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 "$Vabs" --plano 2>&1)"
grep -q mobile_rapido <<< "$plano" || { echo "FALHOU mudança da branch em arquivo da main ficou fora do plano: $plano"; falhou=1; }
rm -rf "$hist"

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
# O mesmo no layout novo do cargo (build/<pacote>/<hash>/out/): o script de build não é executável de teste.
alvo() {
    local d f; d="$(mktemp -d)"
    mkdir -p "$d/debug/incremental/x" "$d/debug/build/dep/1/out" "$d/debug/build/app/2/out" "$d/debug/build/app/3/out"
    for f in debug/incremental/x/a debug/build/dep/1/out/build_script_build debug/build/app/2/out/velho-2 debug/build/app/3/out/novo-3; do
        head -c 65536 /dev/urandom > "$d/$f"
    done
    chmod +x "$d/debug/build/dep/1/out/build_script_build" "$d/debug/build/app/2/out/velho-2" "$d/debug/build/app/3/out/novo-3"
    touch -d '3 days ago' "$d/debug/build/dep/1/out/build_script_build" "$d/debug/build/app/2/out/velho-2"
    echo "$d"
}
podar 250 "build/app/2/out/velho-2 build/app/3/out/novo-3 build/dep/1/out/build_script_build "
podar 180 "build/app/3/out/novo-3 build/dep/1/out/build_script_build "
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
# Com a faixa do cargo aberta, a reserva dela entra na conta: 10 GB cobrem 6 mas não 6 + 6.
with_reserve() {   # $1 = MemAvailable em kB, $2 = reserva em GB, $3 = curta|folga
    local f r; f="$(mktemp)"
    printf 'MemAvailable: %s kB\n' "$1" > "$f"
    if HANGAR_VERIFICAR_MEMINFO="$f" "$V" --memoria-curta "$2"; then r=curta; else r=folga; fi
    [[ "$r" == "$3" ]] || { echo "FALHOU memória $1 kB com reserva $2: $r (esperado $3)"; falhou=1; }
    rm -f "$f"
}
with_reserve 10485760 0 folga
with_reserve 10485760 6 curta

# Espera de memória antes de compilar: com folga segue; sem ela, espera o prazo e falha dizendo
# quanto falta; sem leitura, segue.
memory_wait() {   # $1 = MemAvailable em kB ("" = sem a linha), $2 = GB pedidos, $3 = código esperado
    local f rc; f="$(mktemp)"
    printf 'MemTotal: 24000000 kB\n' > "$f"
    [[ -n "$1" ]] && printf 'MemAvailable: %s kB\n' "$1" >> "$f"
    # Trava própria: a da máquina pode estar com o build de outra sessão. Sem o fd 3: dentro do
    # passo `shell` ele é o canal de andamento da rodada, e o aviso de espera do teste sairia lá.
    HANGAR_BUILD_LOCK="$f.lock" HANGAR_VERIFICAR_MEMINFO="$f" HANGAR_VERIFY_WAIT=0 "$V" --wait-for-memory "$2" > "$f.out" 2>&1 3>&-; rc=$?
    [[ "$rc" == "$3" ]] || { echo "FALHOU espera de memória $1 kB para $2 GB: saiu $rc (esperado $3)"; falhou=1; }
    (( $3 == 0 )) || grep -q "precisa de $2 GB" "$f.out" || { echo "FALHOU espera sem dizer quanto falta: $(cat "$f.out")"; falhou=1; }
    rm -f "$f" "$f.out" "$f.lock"
}
memory_wait 20000000 11 0
memory_wait 5000000 11 1
memory_wait "" 11 0
# Com outro build na trava o prazo não corre: a espera continua em vez de falhar.
if command -v flock > /dev/null; then
    f="$(mktemp)"
    printf 'MemAvailable: 5000000 kB\n' > "$f"
    ( exec 8> "$f.lock"; flock 8; exec sleep 30 ) &
    holder=$!
    until ! flock -n "$f.lock" true 2> /dev/null; do sleep 0.1; done   # estavel: espera a trava ser pega
    HANGAR_BUILD_LOCK="$f.lock" HANGAR_VERIFICAR_MEMINFO="$f" HANGAR_VERIFY_WAIT=0 "$V" --wait-for-memory 11 > /dev/null 2>&1 3>&- &
    waiting=$!
    sleep 1   # estavel: só confirma que seguiu esperando; atraso da máquina nunca vira falha
    kill -0 "$waiting" 2> /dev/null || { echo "FALHOU espera desistiu com outro build na trava"; falhou=1; }
    kill "$waiting" "$holder" 2> /dev/null; wait "$waiting" "$holder" 2> /dev/null
    rm -f "$f" "$f.lock"
fi

# A função `cargo` dos passos: só quem só compila passa pelo memory-scope; os argumentos chegam
# intactos, o runtime do passo volta para o cargo e os pedidos de teto ficam fora do ambiente dele.
if command -v flock > /dev/null; then
    (
        d="$(mktemp -d)"; trap 'rm -rf "$d"' EXIT
        printf '#!/usr/bin/env bash\nprintf "%%s\\n" "$@" "xdg=${XDG_RUNTIME_DIR:-}" "left=${HANGAR_BUILD_MEMORY_HIGH:-}${HANGAR_BUILD_BIND_TO:-}${HANGAR_BUILD_WARN_FD:-}"\n' > "$d/cargo"
        # Um memory-scope que só registra o que recebeu e segue.
        printf '#!/usr/bin/env bash\necho "scope high=$HANGAR_BUILD_MEMORY_HIGH bind=$HANGAR_BUILD_BIND_TO xdg=$XDG_RUNTIME_DIR" >> "%s"\nexec "$@"\n' "$d/scope.log" > "$d/memory-scope"
        chmod +x "$d/cargo"
        eval "$(sed -n '/^    cargo() {/,/^    }/p' "$V")"
        export PATH="$d:$PATH" HANGAR_BUILD_LOCK="$d/lock" XDG_RUNTIME_DIR="$d/passo"
        export HANGAR_VERIFY_RUNTIME="$d/real" HANGAR_VERIFY_UNIT=x.scope HANGAR_VERIFY_MEMORY_SCOPE="$d/memory-scope"
        seen() { printf '%s\na b\n--x\nxdg=%s\nleft=' "$1" "$d/passo"; }
        [[ "$(cargo build 'a b' --x)" == "$(seen build)" ]] || { echo "FALHOU cargo build pelo memory-scope"; exit 1; }
        [[ "$(cat "$d/scope.log")" == "scope high=available bind=x.scope xdg=$d/real" ]] || { echo "FALHOU pedido ao memory-scope: $(cat "$d/scope.log")"; exit 1; }
        # Quem roda testes fica fora do teto; `--no-run` só compila e entra.
        : > "$d/scope.log"
        [[ "$(cargo test 'a b' --x)" == "$(seen test)" && ! -s "$d/scope.log" ]] || { echo "FALHOU cargo test passou pelo teto"; exit 1; }
        cargo test --no-run > /dev/null
        [[ -s "$d/scope.log" ]] || { echo "FALHOU cargo test --no-run ficou fora do teto"; exit 1; }
        # O checkout de quem chamou mudou no meio da rodada: sem o arquivo, o cargo roda direto.
        HANGAR_VERIFY_MEMORY_SCOPE="$d/sumiu"
        [[ "$(cargo build 'a b' --x)" == "$(seen build)" ]] || { echo "FALHOU cargo sem o memory-scope"; exit 1; }
    ) || falhou=1
fi

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

# Trava do pre-push no fork de um PR: a main pública juntada passa, segredo novo não.
gancho="$(cd "$(dirname "$0")/hooks" && pwd)/pre-push"
base="$(mktemp -d)"
(
    set -e
    unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
    export GIT_AUTHOR_NAME=t GIT_AUTHOR_EMAIL=t@t.invalid GIT_COMMITTER_NAME=t GIT_COMMITTER_EMAIL=t@t.invalid HANGAR_SEM_VERIFICACAO=1
    chave="sk-$(printf 'a%.0s' {1..24})"
    git init -q --bare -b main "$base/publico.git"
    git clone -q "$base/publico.git" "$base/w" 2> /dev/null
    cd "$base/w"
    git config url."$base/publico.git".insteadOf https://github.com/jeffer1312/hangar.git
    git checkout -q -B main
    git commit -q --allow-empty -m raiz
    git push -q origin main
    git checkout -q -b pr
    git commit -q --allow-empty -m pr
    no_fork="$(git rev-parse HEAD)"
    git checkout -q main
    echo "$chave" > fixture.txt
    git add fixture.txt
    git commit -q -m fixture
    git push -q origin main
    git checkout -q pr
    git merge -q --no-edit main
    empurra() { printf 'refs/heads/pr %s refs/heads/pr %s\n' "$(git rev-parse HEAD)" "$no_fork" | "$gancho" "file://$base/fork.git" 2>&1; }
    # Recusa pela varredura: um hook quebrado também sai com erro e não pode contar como barrado.
    barra() { local saida; if saida="$(empurra)"; then return 1; fi; grep -q 'push recusado' <<< "$saida"; }
    empurra > /dev/null || { echo "FALHOU pre-push cobrou do fork o que a main pública já tem"; exit 1; }
    # Sem resposta do GitHub nada sai da varredura.
    git config --unset url."$base/publico.git".insteadOf
    git config url."$base/nada.git".insteadOf https://github.com/jeffer1312/hangar.git
    barra || { echo "FALHOU pre-push descontou a main sem resposta do GitHub"; exit 1; }
    git config --unset url."$base/nada.git".insteadOf
    git config url."$base/publico.git".insteadOf https://github.com/jeffer1312/hangar.git
    # `origin/main` local com commit que o GitHub não tem (espelho privado, ref velha) não conta.
    git checkout -q main
    echo "$chave" > privado.txt
    git add privado.txt
    git commit -q -m privado
    git update-ref refs/remotes/origin/main HEAD
    git checkout -q pr
    git merge -q --no-edit main
    barra || { echo "FALHOU pre-push descontou uma origin/main que não está publicada"; exit 1; }
    git reset -q --hard HEAD~1
    git update-ref refs/remotes/origin/main "$(git ls-remote origin refs/heads/main | cut -f1)"
    echo "$chave" > novo.txt
    git add novo.txt
    git commit -q -m novo
    barra || { echo "FALHOU pre-push deixou passar segredo novo"; exit 1; }
) || falhou=1
rm -rf "$base"

(( falhou )) && exit 1
echo "ok: classificação do verificar-local"
