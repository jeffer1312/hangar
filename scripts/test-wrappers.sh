#!/usr/bin/env bash
# Testa os wrappers `claude` (scripts/shell/claude.posix.sh + claude.fish) sem tocar no claude de
# verdade nem abrir tmux: troca `claude` por um fake no PATH que despeja o PRÓPRIO argv + as vars
# ANTHROPIC_*/CP_ENGINE recebidas num arquivo, e confere contra o esperado.
#
# Por que existe: nada na suíte pytest cobre os wrappers (são shell, não python). O regression de
# -c/--resume/--session-id dropando o motor silenciosamente — o `pre` só era montado DEPOIS do
# early-return dessas flags — só foi achado testando à mão. Isto vira commit pra ninguém reintroduzir.
#
# Usage: ./scripts/test-wrappers.sh
set -euo pipefail

REPO="$(cd "$(dirname "$0")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

mkdir -p "$TMP/bin"
# Fake "claude": nunca chama o binário real nem abre sessão nenhuma. Só regista o que recebeu — em
# CHAVES explícitas, não um dump cru de `env`: ANTHROPIC_AUTH_TOKEN tem que existir no ambiente (é
# assim que o motor funciona), então um dump cru misturaria a secret "esperada" no env com a secret
# "proibida" no argv no MESMO arquivo, e a checagem de vazamento (grep -v sk-) pegaria as duas.
cat >"$TMP/bin/claude" <<'FAKE'
#!/usr/bin/env bash
{
    printf 'ARGV:'; printf ' %q' "$@"; printf '\n'
    printf 'ENV_BASE_URL=%s\n' "${ANTHROPIC_BASE_URL:-}"
    printf 'ENV_MODEL=%s\n' "${ANTHROPIC_MODEL:-}"
    printf 'ENV_CP_ENGINE=%s\n' "${CP_ENGINE:-}"
} > "$CP_TEST_OUT"
FAKE
chmod +x "$TMP/bin/claude"

# Fake "pi": mesma ideia do fake claude acima, mas so precisa registar ARGV + CP_PI_SESSION (o pi
# wrapper nao tem o indireto de motor).
cat >"$TMP/bin/pi" <<'FAKE'
#!/usr/bin/env bash
{
    printf 'ARGV:'; printf ' %q' "$@"; printf '\n'
    printf 'ENV_CP_PI_SESSION=%s\n' "${CP_PI_SESSION:-}"
} > "$CP_TEST_OUT"
FAKE
chmod +x "$TMP/bin/pi"

# Motor de teste isolado — NUNCA o ~/.claude/engines.json real.
CP_ENGINES_FILE="$TMP/engines.json"
cat >"$CP_ENGINES_FILE" <<'JSON'
{"probe": {"label": "probe", "base_url": "https://a.b", "api_key": "sk-test-probe-abcdefgh", "model": "m1"}}
JSON

PATH_WITH_FAKES="$REPO/scripts:$TMP/bin:$PATH"

fail=0

# $1=descrição  $2=arquivo de saída  $3..=linhas exatas esperadas (ex: "ENV_MODEL=m1")
check() {
    local desc="$1" out="$2" pat
    shift 2
    for pat in "$@"; do
        if ! [ -f "$out" ] || ! grep -qxF -- "$pat" "$out"; then
            echo "FAIL: $desc — esperava a linha '$pat'"
            [ -f "$out" ] && sed 's/^/    /' "$out"
            fail=1
        fi
    done
}

# $1=descrição  $2=arquivo de saída — confere que o --session-id do ARGV é o MESMO uuid exportado em
# CP_PI_SESSION (o ponto inteiro do wrapper: pi reescreve o próprio argv, então o backend só acha o
# id lendo a env var — ver registry.py:_pi_sid_of).
check_pi_injected() {
    local desc="$1" out="$2" argv_line sid_line sid_argv sid_env
    if ! [ -f "$out" ]; then
        echo "FAIL: $desc — sem arquivo de saída"
        fail=1
        return
    fi
    argv_line=$(grep '^ARGV:' "$out")
    sid_line=$(grep '^ENV_CP_PI_SESSION=' "$out")
    sid_argv=$(printf '%s' "$argv_line" | sed -n 's/.*--session-id \([^ ]*\).*/\1/p')
    sid_env="${sid_line#ENV_CP_PI_SESSION=}"
    if [ -z "$sid_argv" ] || [ -z "$sid_env" ] || [ "$sid_argv" != "$sid_env" ]; then
        echo "FAIL: $desc — --session-id do ARGV ('$sid_argv') difere de CP_PI_SESSION ('$sid_env')"
        sed 's/^/    /' "$out"
        fail=1
    fi
}

# A secret só pode existir no ENV (é assim que o motor funciona); no ARGV é o vazamento que a task
# inteira existe pra evitar (/proc/<pid>/cmdline é legível por qualquer usuário da máquina).
check_argv_sem_segredo() {
    local desc="$1" out="$2"
    if ! [ -f "$out" ] || grep '^ARGV:' "$out" | grep -qF 'sk-'; then
        echo "FAIL: $desc — ARGV contém a secret"
        [ -f "$out" ] && sed 's/^/    /' "$out"
        fail=1
    fi
}

# $1=binário do shell (bash/zsh)  $2=CP_ENGINE ("" p/ nenhum)  resto=args do `claude`
# env -i: ambiente limpo de propósito — sem isto um ANTHROPIC_* que já esteja no ambiente de quem
# roda este script (ex: a própria sessão Claude Code atual) mascararia um teste "sem motor" quebrado.
posix_case() {
    local sh="$1" engine="$2" out="$TMP/out.$RANDOM.$RANDOM"
    shift 2
    env -i PATH="$PATH_WITH_FAKES" HOME="$HOME" CP_TEST_OUT="$out" CP_ENGINE="$engine" \
        CP_ENGINES_FILE="$CP_ENGINES_FILE" \
        "$sh" -c '
            source "'"$REPO"'/scripts/shell/claude.posix.sh"
            claude "$@"
        ' _ "$@" </dev/null || true
    printf '%s' "$out"
}

fish_case() {
    local engine="$1" out="$TMP/out.$RANDOM.$RANDOM"
    shift
    # `--` separa os args do próprio `claude` dos flags do binário fish — sem isto "fish -c '...' --print"
    # tenta interpretar --print como opção do fish (erro "unknown option").
    env -i PATH="$PATH_WITH_FAKES" HOME="$HOME" CP_TEST_OUT="$out" CP_ENGINE="$engine" \
        CP_ENGINES_FILE="$CP_ENGINES_FILE" \
        fish --no-config -c '
            source "'"$REPO"'/scripts/shell/claude.fish"
            claude $argv
        ' -- "$@" </dev/null || true
    printf '%s' "$out"
}

# $1=binário do shell (bash/zsh)  resto=args do `pi`. Sem CP_ENGINE — o wrapper pi não tem esse
# indireto. env -i por consistência com posix_case.
posix_case_pi() {
    local sh="$1" out="$TMP/out.$RANDOM.$RANDOM"
    shift
    env -i PATH="$PATH_WITH_FAKES" HOME="$HOME" CP_TEST_OUT="$out" \
        "$sh" -c '
            source "'"$REPO"'/scripts/shell/pi.posix.sh"
            pi "$@"
        ' _ "$@" </dev/null || true
    printf '%s' "$out"
}

fish_case_pi() {
    local out="$TMP/out.$RANDOM.$RANDOM"
    env -i PATH="$PATH_WITH_FAKES" HOME="$HOME" CP_TEST_OUT="$out" \
        fish --no-config -c '
            source "'"$REPO"'/scripts/shell/pi.fish"
            pi $argv
        ' -- "$@" </dev/null || true
    printf '%s' "$out"
}

for SH in bash zsh; do
    echo "== $SH =="

    out=$(posix_case "$SH" "" --print)
    check "$SH sem motor, --print" "$out" 'ENV_BASE_URL=' 'ENV_CP_ENGINE='

    out=$(posix_case "$SH" probe --print)
    check "$SH motor + --print" "$out" 'ENV_BASE_URL=https://a.b' 'ENV_MODEL=m1' 'ENV_CP_ENGINE=probe'
    check_argv_sem_segredo "$SH motor + --print" "$out"

    out=$(posix_case "$SH" probe -c)
    check "$SH motor + -c (regressão)" "$out" 'ENV_BASE_URL=https://a.b' 'ENV_MODEL=m1' 'ENV_CP_ENGINE=probe'
    check_argv_sem_segredo "$SH motor + -c" "$out"

    out=$(posix_case "$SH" probe --resume abc)
    check "$SH motor + --resume (regressão)" "$out" 'ENV_BASE_URL=https://a.b' 'ENV_MODEL=m1' 'ENV_CP_ENGINE=probe'
    check_argv_sem_segredo "$SH motor + --resume" "$out"

    out=$(posix_case_pi "$SH")
    check_pi_injected "$SH pi bare (injeta --session-id + CP_PI_SESSION)" "$out"

    out=$(posix_case_pi "$SH" --resume abc)
    check "$SH pi --resume (passthrough, sem injeção)" "$out" 'ARGV: --resume abc' 'ENV_CP_PI_SESSION='

    # Fix round 1: -c/-r são os short flags REAIS do pi (confirmado em `pi --help`) — mesma classe de
    # bug que o scan do wrapper claude já cobria pro -c dele. --session/--fork/--no-session são as
    # outras três flags que também gerenciam a própria sessão.
    out=$(posix_case_pi "$SH" -c)
    check "$SH pi -c (passthrough, sem injeção)" "$out" 'ARGV: -c' 'ENV_CP_PI_SESSION='

    out=$(posix_case_pi "$SH" -r)
    check "$SH pi -r (passthrough, sem injeção)" "$out" 'ARGV: -r' 'ENV_CP_PI_SESSION='

    out=$(posix_case_pi "$SH" --continue)
    check "$SH pi --continue (passthrough, sem injeção)" "$out" 'ARGV: --continue' 'ENV_CP_PI_SESSION='

    out=$(posix_case_pi "$SH" --session-id existing-uuid-test)
    check "$SH pi --session-id (passthrough, sem injeção)" "$out" 'ARGV: --session-id existing-uuid-test' 'ENV_CP_PI_SESSION='

    out=$(posix_case_pi "$SH" --session foo)
    check "$SH pi --session (passthrough, sem injeção)" "$out" 'ARGV: --session foo' 'ENV_CP_PI_SESSION='

    out=$(posix_case_pi "$SH" --fork bar)
    check "$SH pi --fork (passthrough, sem injeção)" "$out" 'ARGV: --fork bar' 'ENV_CP_PI_SESSION='

    out=$(posix_case_pi "$SH" --no-session)
    check "$SH pi --no-session (passthrough, sem injeção)" "$out" 'ARGV: --no-session' 'ENV_CP_PI_SESSION='

    # Fix round 2 (bug real do usuário): `pi remove npm:pi-claude-code-tui` abria a TUI e não removia
    # nada. Subcomando e uso não interativo têm que chegar CRUS no binário — sem --session-id, sem
    # CP_PI_SESSION, sem tmux. Precedente: os subcomandos do wrapper codex.
    for sub in install remove uninstall update list config; do
        out=$(posix_case_pi "$SH" "$sub" npm:foo)
        check "$SH pi $sub (subcomando cru)" "$out" "ARGV: $sub npm:foo" 'ENV_CP_PI_SESSION='
    done

    for flag in -p --print --list-models --help -h --version -v; do
        out=$(posix_case_pi "$SH" "$flag")
        check "$SH pi $flag (não interativo, cru)" "$out" "ARGV: $flag" 'ENV_CP_PI_SESSION='
    done

    out=$(posix_case_pi "$SH" --mode json -p oi)
    check "$SH pi --mode json (cru)" "$out" 'ARGV: --mode json -p oi' 'ENV_CP_PI_SESSION='

    out=$(posix_case_pi "$SH" --export out.html)
    check "$SH pi --export (cru)" "$out" 'ARGV: --export out.html' 'ENV_CP_PI_SESSION='

    # A distinção que mordeu o usuário: subcomando SÓ como primeiro argumento. Uma mensagem que por
    # acaso começa com a palavra "remove" continua sendo lançamento interativo com prompt inicial.
    out=$(posix_case_pi "$SH" 'remove the dead code')
    check_pi_injected "$SH pi \"remove the dead code\" (prompt, não subcomando)" "$out"

    out="$TMP/out.$RANDOM.$RANDOM"
    env -i PATH="$PATH_WITH_FAKES" HOME="$HOME" CP_TEST_OUT="$out" \
        "$SH" -c '
            source "'"$REPO"'/scripts/shell/pi.posix.sh"
            command pi --raw
        ' </dev/null || true
    check "$SH command pi (bypass, sem injeção)" "$out" 'ARGV: --raw' 'ENV_CP_PI_SESSION='
done

if command -v fish >/dev/null 2>&1; then
    echo "== fish =="

    out=$(fish_case "" --print)
    check "fish sem motor, --print" "$out" 'ENV_BASE_URL=' 'ENV_CP_ENGINE='

    out=$(fish_case probe --print)
    check "fish motor + --print" "$out" 'ENV_BASE_URL=https://a.b' 'ENV_MODEL=m1' 'ENV_CP_ENGINE=probe'
    check_argv_sem_segredo "fish motor + --print" "$out"

    out=$(fish_case probe -c)
    check "fish motor + -c (regressão)" "$out" 'ENV_BASE_URL=https://a.b' 'ENV_MODEL=m1' 'ENV_CP_ENGINE=probe'
    check_argv_sem_segredo "fish motor + -c" "$out"

    out=$(fish_case probe --resume abc)
    check "fish motor + --resume (regressão)" "$out" 'ENV_BASE_URL=https://a.b' 'ENV_MODEL=m1' 'ENV_CP_ENGINE=probe'
    check_argv_sem_segredo "fish motor + --resume" "$out"

    out=$(fish_case_pi)
    check_pi_injected "fish pi bare (injeta --session-id + CP_PI_SESSION)" "$out"

    out=$(fish_case_pi --resume abc)
    check "fish pi --resume (passthrough, sem injeção)" "$out" 'ARGV: --resume abc' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi -c)
    check "fish pi -c (passthrough, sem injeção)" "$out" 'ARGV: -c' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi -r)
    check "fish pi -r (passthrough, sem injeção)" "$out" 'ARGV: -r' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi --continue)
    check "fish pi --continue (passthrough, sem injeção)" "$out" 'ARGV: --continue' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi --session-id existing-uuid-test)
    check "fish pi --session-id (passthrough, sem injeção)" "$out" 'ARGV: --session-id existing-uuid-test' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi --session foo)
    check "fish pi --session (passthrough, sem injeção)" "$out" 'ARGV: --session foo' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi --fork bar)
    check "fish pi --fork (passthrough, sem injeção)" "$out" 'ARGV: --fork bar' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi --no-session)
    check "fish pi --no-session (passthrough, sem injeção)" "$out" 'ARGV: --no-session' 'ENV_CP_PI_SESSION='

    # Mesmos casos do bloco posix — os dois shells têm que tomar SEMPRE o mesmo ramo.
    for sub in install remove uninstall update list config; do
        out=$(fish_case_pi "$sub" npm:foo)
        check "fish pi $sub (subcomando cru)" "$out" "ARGV: $sub npm:foo" 'ENV_CP_PI_SESSION='
    done

    for flag in -p --print --list-models --help -h --version -v; do
        out=$(fish_case_pi "$flag")
        check "fish pi $flag (não interativo, cru)" "$out" "ARGV: $flag" 'ENV_CP_PI_SESSION='
    done

    out=$(fish_case_pi --mode json -p oi)
    check "fish pi --mode json (cru)" "$out" 'ARGV: --mode json -p oi' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi --export out.html)
    check "fish pi --export (cru)" "$out" 'ARGV: --export out.html' 'ENV_CP_PI_SESSION='

    out=$(fish_case_pi 'remove the dead code')
    check_pi_injected "fish pi \"remove the dead code\" (prompt, não subcomando)" "$out"

    # Sem uuidgen: no fish a substituição que falha deixa $id como LISTA VAZIA, então
    # `pi --session-id $id oi` colapsava pra `pi --session-id oi` — a flag comia o argumento do
    # usuário e CP_PI_SESSION saía vazio (o posix já tinha o fallback /proc; o fish, não).
    # Sombreia em vez de tirar do PATH: /usr/bin também tem bash/cat, não dá pra remover.
    mkdir -p "$TMP/sem-uuidgen"
    printf '#!/bin/sh\nexit 127\n' > "$TMP/sem-uuidgen/uuidgen"
    chmod +x "$TMP/sem-uuidgen/uuidgen"
    out="$TMP/out.$RANDOM.$RANDOM"
    env -i PATH="$TMP/sem-uuidgen:$PATH_WITH_FAKES" HOME="$HOME" CP_TEST_OUT="$out" \
        fish --no-config -c '
            source "'"$REPO"'/scripts/shell/pi.fish"
            pi $argv
        ' -- oi </dev/null || true
    check_pi_injected "fish pi sem uuidgen (fallback /proc)" "$out"
    if ! grep -qE '^ARGV: --session-id [0-9a-fA-F-]+ oi$' "$out"; then
        echo "FAIL: fish pi sem uuidgen — o argumento do usuário não sobreviveu ao --session-id"
        [ -f "$out" ] && sed 's/^/    /' "$out"
        fail=1
    fi

    out="$TMP/out.$RANDOM.$RANDOM"
    env -i PATH="$PATH_WITH_FAKES" HOME="$HOME" CP_TEST_OUT="$out" \
        fish --no-config -c '
            source "'"$REPO"'/scripts/shell/pi.fish"
            command pi --raw
        ' </dev/null || true
    check "fish command pi (bypass, sem injeção)" "$out" 'ARGV: --raw' 'ENV_CP_PI_SESSION='
else
    echo "== fish: não encontrado no PATH, pulando =="
fi

if [ "$fail" = 0 ]; then
    echo "PASS: todos os wrappers"
else
    echo "FAIL: ver acima"
fi
exit "$fail"
