#!/usr/bin/env bash
# Trava a regra única de qual `hangar-server` os atalhos rodam (scripts/hangar-server-lookup) e o que
# cada atalho repassa: hangar-send, hangar-open, hangar-conta, hangar-mcp-headers, hangar-mcp-stdio, hangar-server-command
# e hangar-engine. Binários falsos que só registram o argv; HOME e cópia em pasta temporária.
#
# Uso: ./scripts/test-hangar-server-lookup.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
falhas=0

checa() { # <caso> <esperado> <obtido>
    if [[ "$2" == "$3" ]]; then printf 'ok   %s\n' "$1"
    else printf 'FALHA %s -> esperava %q, veio %q\n' "$1" "$2" "$3"; falhas=$((falhas + 1)); fi
}

# Binário falso: grava "<caminho> <args>" e sai com FAKE_RC (ou morre pelo FAKE_SIGNAL).
falso() { # <caminho>
    mkdir -p "$(dirname "$1")"
    printf '#!/bin/sh\nprintf "%%s" "$0" > "$FAKE_LOG"; for a in "$@"; do printf " [%%s]" "$a" >> "$FAKE_LOG"; done\n[ -n "${FAKE_SIGNAL:-}" ] && kill -"$FAKE_SIGNAL" $$\nexit "${FAKE_RC:-0}"\n' > "$1"
    chmod +x "$1"
}

# Uma cópia limpa por caso: raiz, casa e o log do falso.
caso() {
    C="$TMP/c$RANDOM$RANDOM"
    mkdir -p "$C/root/scripts" "$C/home"
    cp "$REPO/scripts/hangar-server-lookup" "$REPO/scripts/hangar-send" "$REPO/scripts/hangar-open" \
        "$REPO/scripts/hangar-conta" "$REPO/scripts/hangar-mcp-headers.sh" "$REPO/scripts/hangar-mcp-stdio.sh" "$REPO/scripts/hangar-server-command" \
        "$REPO/scripts/hangar-engine" "$C/root/scripts/"
    export HOME="$C/home" FAKE_LOG="$C/log"
    unset CP_RUST_SERVER_BIN FAKE_RC FAKE_SIGNAL
}
# Qual binário o atalho rodou (o argv[0] gravado pelo falso).
quem() { sh "$C/root/scripts/hangar-server-command" x >/dev/null 2>&1; cut -d' ' -f1 "$C/log" 2>/dev/null; }

BUILD=crates/target/release/hangar-server

caso; falso "$C/env/hangar-server"; falso "$C/root/$BUILD"
CP_RUST_SERVER_BIN="$C/env/hangar-server" checa "variável válida vence" "$C/env/hangar-server" "$(CP_RUST_SERVER_BIN="$C/env/hangar-server" quem)"

caso; falso "$C/home/.hangar/bin/hangar-server"
checa "variável com ~" "$C/home/.hangar/bin/hangar-server" "$(CP_RUST_SERVER_BIN='~/.hangar/bin/hangar-server' quem)"
checa "variável em branco segue a regra" "$C/home/.hangar/bin/hangar-server" "$(CP_RUST_SERVER_BIN='   ' quem)"

caso; falso "$C/root/$BUILD"
saida=$(CP_RUST_SERVER_BIN=/nao/existe sh "$C/root/scripts/hangar-server-command" x 2>&1); rc=$?
checa "variável inválida não cai para outro: código" "126" "$rc"
checa "variável inválida não cai para outro: frase" "hangar: CP_RUST_SERVER_BIN não é um executável" "$saida"
falso "$C/env/hangar-server"; chmod -x "$C/env/hangar-server"
CP_RUST_SERVER_BIN="$C/env/hangar-server" sh "$C/root/scripts/hangar-server-command" x >/dev/null 2>&1
checa "variável sem +x não conta" "126" "$?"

caso; mkdir -p "$C/home/.hangar/versions/v1"; ln -s "$C/home/.hangar/versions/v1" "$C/home/.hangar/current"
falso "$C/home/.hangar/versions/v1/bin/hangar-server"; falso "$C/home/.hangar/bin/hangar-server"; falso "$C/root/$BUILD"
checa "current/bin vence, pelo caminho real" "$C/home/.hangar/versions/v1/bin/hangar-server" "$(quem)"

caso; mkdir -p "$C/home/.hangar/current.old"; falso "$C/home/.hangar/bin/hangar-server"
inicio=$(date +%s)
checa "só o current.old: espera e usa o baixado" "$C/home/.hangar/bin/hangar-server" "$(quem)"
(( $(date +%s) - inicio <= 3 )) || { echo "FALHA a espera do current.old passou de 1 s"; falhas=$((falhas + 1)); }

caso; falso "$C/root/$BUILD"; falso "$C/home/.hangar/bin/hangar-server"
touch -t 202601010000.10 "$C/root/$BUILD"; touch -t 202601010000.00 "$C/home/.hangar/bin/hangar-server"
checa "build da cópia mais novo" "$C/root/$BUILD" "$(quem)"
touch -t 202601010000.20 "$C/home/.hangar/bin/hangar-server"
checa "baixado mais novo" "$C/home/.hangar/bin/hangar-server" "$(quem)"
touch -t 202601010000.20 "$C/root/$BUILD"
checa "empate fica com a cópia" "$C/root/$BUILD" "$(quem)"

caso; mkdir -p "$C/path"; falso "$C/path/hangar-server"
saida=$(PATH="$C/path:$PATH" sh "$C/root/scripts/hangar-server-command" x 2>&1); rc=$?
checa "o PATH não conta: código" "126" "$rc"
checa "o PATH não conta: frase" "hangar: hangar-server não encontrado; instale o Hangar ou configure CP_RUST_SERVER_BIN" "$saida"

# O que cada atalho repassa ao binário escolhido.
caso; falso "$C/home/.hangar/bin/hangar-server"
sh "$C/root/scripts/hangar-send" alvo "oi a"
checa "hangar-send repassa a raiz" "$C/home/.hangar/bin/hangar-server [send] [--root] [$C/root] [alvo] [oi a]" "$(cat "$C/log")"
sh "$C/root/scripts/hangar-mcp-headers.sh"
checa "hangar-mcp-headers repassa a raiz" "$C/home/.hangar/bin/hangar-server [mcp-headers] [--root] [$C/root]" "$(cat "$C/log")"
sh "$C/root/scripts/hangar-mcp-stdio.sh"
checa "hangar-mcp-stdio repassa a raiz" "$C/home/.hangar/bin/hangar-server [mcp-stdio] [--root] [$C/root]" "$(cat "$C/log")"
sh "$C/root/scripts/hangar-engine" --list
checa "hangar-engine" "$C/home/.hangar/bin/hangar-server [engine] [--list]" "$(cat "$C/log")"
ln -s "$C/root/scripts/hangar-send" "$C/home/link-send"
sh "$C/home/link-send" --list
checa "pelo link, a raiz é a da cópia" "$C/home/.hangar/bin/hangar-server [send] [--root] [$C/root] [--list]" "$(cat "$C/log")"

FAKE_RC=2 sh "$C/root/scripts/hangar-conta" --new 'Nome ç' 2>/dev/null; rc=$?
checa "hangar-conta repassa argumentos" "$C/home/.hangar/bin/hangar-server [conta] [--new] [Nome ç]" "$(cat "$C/log")"
checa "hangar-conta repassa o código" "2" "$rc"
saida=$(FAKE_SIGNAL=9 sh "$C/root/scripts/hangar-conta" --list 2>&1); rc=$?
checa "hangar-conta: servidor morto por sinal sai 1" "1" "$rc"
# O bash como `sh` também anuncia o processo morto: vale a última linha.
checa "hangar-conta: servidor morto por sinal diz" "hangar-conta: o hangar-server parou pelo sinal 9 sem responder" "$(printf '%s\n' "$saida" | tail -n 1)"
saida=$(CP_RUST_SERVER_BIN=/nao/existe sh "$C/root/scripts/hangar-conta" --list 2>&1); rc=$?
checa "hangar-conta sem binário: código" "126" "$rc"
checa "hangar-conta sem binário: frase" "hangar-conta: hangar-server não encontrado; instale o Hangar ou aponte CP_RUST_SERVER_BIN para o binário" "$saida"

# hangar-open: sem binário é o CLI cru (100); binário antigo sem `open-cli` (sai 2) também.
saida=$(CP_RUST_SERVER_BIN=/nao/existe sh "$C/root/scripts/hangar-open" claude 2>&1); rc=$?
checa "hangar-open sem binário: código" "100" "$rc"
checa "hangar-open sem binário: frase" "hangar: hangar-server não encontrado; abrindo o claude sem o Hangar" "$saida"
saida=$(FAKE_RC=2 sh "$C/root/scripts/hangar-open" claude --model x 2>&1); rc=$?
checa "hangar-open repassa ao open-cli" "$C/home/.hangar/bin/hangar-server [open-cli] [claude] [--model] [x]" "$(cat "$C/log")"
checa "hangar-open com binário antigo: código" "100" "$rc"
checa "hangar-open com binário antigo: frase" "hangar: o hangar-server não abriu a sessão (saída 2); abrindo o claude sem o Hangar" "$saida"
FAKE_RC=7 sh "$C/root/scripts/hangar-open" claude >/dev/null 2>&1
checa "hangar-open repassa outro código" "7" "$?"

echo
if (( falhas )); then echo "$falhas falha(s)"; exit 1; fi
echo "tudo ok"
