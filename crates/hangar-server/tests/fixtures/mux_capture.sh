#!/bin/sh
# O programa é imutável; cada teste guarda seus dados ao lado do link que o chama.
root=${0%/*}
case "$1" in
    has-session)
        printf 'has\n' >> "$root/calls"
        code=$(cat "$root/has-rc")
        if [ "$code" = -1 ]; then
            mkfifo "$root/blocked"
            IFS= read -r ignored < "$root/blocked"
        fi
        exit "$code"
        ;;
    capture-pane)
        cat "$root/frame"
        exit "$(cat "$root/capture-rc")"
        ;;
esac
exit 99
