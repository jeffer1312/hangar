#!/usr/bin/env bash
# Build do hangar-native com o perfil `dist` (LTO completo e uma unidade de código), o mesmo que a main
# publica. Ele sai em <target>/dist/, à parte do release comum, que continua rápido para as faixas.
# Uso: tools/build-otimizado.sh <destino-do-binario>   (CARGO_TARGET_DIR vale, se definido)
set -euo pipefail

destino=${1:?uso: $0 <destino-do-binario>}
# Relativos valem a partir de onde o script foi chamado, não de desktop-native/.
destino=$(realpath -m -- "$destino")
[[ -d $(dirname "$destino") ]] || { echo "pasta de $destino não existe" >&2; exit 1; }

cd "$(dirname "$0")/.."
cargo build --locked --profile dist
target=$(cargo metadata --locked --no-deps --format-version 1 | jq -r .target_directory)

# Cópia + rename: sobrescrever o binário em uso dá "Text file busy".
install -m 755 "$target/dist/hangar-native" "$destino.novo"
mv -f "$destino.novo" "$destino"
echo "$destino"
