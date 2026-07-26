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
else
    echo "== fish: não encontrado no PATH, pulando =="
fi

if [ "$fail" = 0 ]; then
    echo "PASS: todos os wrappers"
else
    echo "FAIL: ver acima"
fi
exit "$fail"
