#!/usr/bin/env bash
# Trava o Python dos scripts: o `hangar-python` roda o do pacote (`~/.hangar/current/python`), senão o
# venv da cópia, senão o `python3` da máquina; o hangar-send e os scripts Python com shebang usam o
# `hangar-python` quando ele existe e caem no `python3` sem ele (máquina não migrada). HOME, cópia e
# interpretadores falsos em pasta temporária.
#
# Uso: ./scripts/test-hangar-python.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
falhas=0
HOME_T="$TMP/home"
INST="$TMP/inst"
FAKE="$TMP/fake"
mkdir -p "$HOME_T" "$INST/scripts" "$FAKE"

checa() { # <caso> <esperado> <obtido>
    if [[ "$2" == "$3" ]]; then
        printf 'ok   %s\n' "$1"
    else
        printf 'FALHA %s -> esperava %q, veio %q\n' "$1" "$2" "$3"; falhas=$((falhas + 1))
    fi
}

# Interpretador falso: diz quem é, o primeiro argumento e o cache de bytecode.
falso() { # <caminho> <nome>
    mkdir -p "$(dirname "$1")"
    printf '#!/bin/sh\necho "%s|$1|${PYTHONPYCACHEPREFIX:-}"\n' "$2" > "$1"
    chmod +x "$1"
}
falso "$FAKE/python3" sistema
BASE_PATH="$FAKE:/usr/bin:/bin"

# --- o atalho hangar-python -------------------------------------------------------------------
cp "$REPO/scripts/hangar-python" "$INST/scripts/"
hp() { HOME="$HOME_T" PATH="$BASE_PATH" "$INST/scripts/hangar-python" arg 2>&1; }
checa "atalho sem pacote nem venv: python3" "sistema|arg|" "$(hp)"
falso "$INST/backend/.venv/bin/python" venv
checa "atalho no checkout: o venv" "venv|arg|" "$(hp)"
real="$HOME_T/.hangar/versions/0.1.0.9"
falso "$real/python/bin/python3" pacote
ln -s "$real" "$HOME_T/.hangar/current"
checa "atalho com o pacote: o Python dele, cache fora da versão" \
    "pacote|arg|$HOME_T/.hangar/cache/pycache/0.1.0.9" "$(hp)"

# --- o Python cravado no atalho do Codex (install-claude-wrapper.sh) ---------------------------
funcao=$(sed -n '/^py_do_venv() {/,/^}/p' "$REPO/scripts/install-claude-wrapper.sh")
[[ -n "$funcao" ]] || { echo "FALHA py_do_venv não encontrado"; falhas=$((falhas + 1)); }
py_wrapper() { HOME="$1" REPO_DIR="$INST" bash -c "$funcao"$'\npy_do_venv'; }
checa "atalho do Codex com o pacote: o Python pelo current" \
    "$HOME_T/.hangar/current/python/bin/python3" "$(py_wrapper "$HOME_T")"
checa "atalho do Codex sem o pacote: o venv do checkout" "$INST/backend/.venv/bin/python" "$(py_wrapper "$TMP/vazio")"

# --- hangar-send: um PY só, e nenhum python3 chamado direto ------------------------------------
linha=$(grep -m1 '^PY=' "$REPO/scripts/hangar-send")
[[ -n "$linha" ]] || { echo "FALHA hangar-send sem a linha PY="; falhas=$((falhas + 1)); }
py_do_send() { PATH="$1" bash -c "$linha"$'\nprintf "%s" "$PY"'; }
checa "hangar-send sem o atalho: python3" "$FAKE/python3" "$(py_do_send "$BASE_PATH")"
falso "$TMP/bin/hangar-python" atalho
checa "hangar-send com o atalho: hangar-python" "$TMP/bin/hangar-python" "$(py_do_send "$TMP/bin:$BASE_PATH")"
diretos=$(grep -nE '(^|[^-[:alnum:]_/.])python3([[:space:]]|$)' "$REPO/scripts/hangar-send" | grep -vE '^[0-9]+:[[:space:]]*#|^[0-9]+:PY=')
checa "hangar-send não chama python3 direto" "" "$diretos"

# --- scripts Python pelo shebang ---------------------------------------------------------------
for s in scripts/hangar-codex scripts/hangar-engine scripts/hangar-conta scripts/hangar-open \
         skills/orquestrar/scripts/orq.py skills/orquestrar/scripts/orq-valida-eventos.py \
         skills/pr-review-auto/scripts/pr_watch.py; do
    copia="$TMP/s/$(basename "$s")"
    mkdir -p "$TMP/s"
    cp "$REPO/$s" "$copia"
    chmod +x "$copia"
    checa "$s com o atalho" "atalho|$copia|" "$(PATH="$TMP/bin:$BASE_PATH" "$copia" 2>&1)"
    checa "$s sem o atalho" "sistema|$copia|" "$(PATH="$BASE_PATH" "$copia" 2>&1)"
done

exit "$falhas"
