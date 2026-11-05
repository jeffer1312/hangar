#!/usr/bin/env bash
# Trava o que o hangar-send manda ao backend — o POST /api/sessions do `--new`, o POST /input do
# recado com HANGAR_SEND_PAINEL=1 e o GET/PUT do `--contrato` —, contra um backend FALSO que só grava o corpo. É shell, a suíte
# pytest não alcança; e o script de verdade é copiado (não colado aqui), pra o teste quebrar junto
# com ele.
#
# Existe pelo --headless: só a tela de criar sessão abria sessão sem terminal, e o flag faltando no
# script fazia quem orquestra por hangar-send cair calado numa sessão com terminal.
#
# Uso: ./scripts/test-hangar-send-new.sh
set -uo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TMP="$(mktemp -d)"
falhas=0
# O hangar-send prefere ~/.hangar/config: com a HOME real, o teste falava com o servidor de verdade.
export HOME="$TMP/home"

# O hangar-send chama `python3`. No Windows ele pode ser o atalho da Microsoft Store, que existe no
# PATH e não executa nada: acha um Python que RODA e o expõe como python3 só pra este teste.
PY=""
for c in python3 python py; do
    if "$c" -c 'pass' >/dev/null 2>&1; then PY="$(command -v "$c")"; break; fi
done
[[ -n "$PY" ]] || { echo "FALHA: nenhum Python que execute no PATH"; exit 1; }
mkdir -p "$TMP/bin"
printf '#!/bin/sh\nexec "%s" "$@"\n' "$PY" > "$TMP/bin/python3"
chmod +x "$TMP/bin/python3"
export PATH="$TMP/bin:$PATH"

PORTA=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')
mkdir -p "$TMP/scripts" "$TMP/backend"
cp "$REPO/scripts/hangar-send" "$TMP/scripts/hangar-send"
printf 'CP_AUTH_TOKEN=teste\nCP_PORT=%s\n' "$PORTA" > "$TMP/backend/.env"

python3 - "$PORTA" "$TMP/corpo.json" <<'PY' &
import http.server, sys
porta, destino = int(sys.argv[1]), sys.argv[2]
class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers()
        if self.path.startswith("/api/claude-configs"):
            self.wfile.write(b'[{"path":"/home/la/.claude","label":"default"}]')
            return
        if self.path.endswith("/pair/contract"):
            self.wfile.write('{"peers":["lab::s2"],"path":"","content":"decisão A\\n","owner":"lab","mtime":"111"}'.encode())
            return
        if self.path == "/api/sessions":
            self.wfile.write(b'[{"name":"viva"}]')
            return
        self.wfile.write(b'{"claude":{"disponivel":true,"default":true},"codex":{"disponivel":true,"default":false}}')
    def do_POST(self):
        corpo = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        open(destino, "wb").write(corpo)
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(b'{"avisos":["aviso-teste"],"provider":"claude","provider_source":"inherited"}')
    def do_PUT(self):
        corpo = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        open(destino, "wb").write(corpo)
        open(destino + ".rota", "w").write(self.path)
        self.send_response(200); self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(b'{"mtime":"222"}')
    def do_DELETE(self):
        open(destino + ".delete", "w").write(self.path)
        open(destino + ".auth", "w").write(self.headers.get("Authorization", ""))
        falhou = self.headers.get("Authorization") == "Bearer tok-falha"
        self.send_response(500 if falhou else 200); self.send_header("Content-Type", "application/json"); self.end_headers()
        self.wfile.write(b'{"detail":"falhou"}' if falhou else b'{"ok":true}')
    def log_message(self, *a): pass
http.server.HTTPServer(("127.0.0.1", porta), H).serve_forever()
PY
SERVIDOR=$!
trap 'kill "$SERVIDOR" 2>/dev/null; rm -rf "$TMP"' EXIT
no_ar=""
for _ in $(seq 1 50); do
    python3 -c 'import socket,sys; socket.create_connection(("127.0.0.1", int(sys.argv[1])), 0.2)' "$PORTA" 2>/dev/null && { no_ar=1; break; }
    sleep 0.1
done
# Sem isto a falha aparecia como "valor diferente" nos casos, e não como o que é.
[[ -n "$no_ar" ]] || { echo "FALHA: o backend falso não subiu na porta $PORTA"; exit 1; }

campo() { python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(json.dumps(d.get(sys.argv[2])))' "$TMP/corpo.json" "$1"; }
checa() { # <caso> <esperado> <obtido>
    if [[ "$2" == "$3" ]]; then printf 'ok   %s -> %s\n' "$1" "$3"
    else printf 'FALHA %s -> esperava %s, veio %s\n' "$1" "$2" "$3"; falhas=$((falhas + 1)); fi
}

# 1. --headless vai no corpo, junto com o modelo.
rm -f "$TMP/corpo.json"
bash "$TMP/scripts/hangar-send" --new hl-x /tmp --headless --model haiku >/dev/null 2>&1
checa "--headless no corpo" 'true' "$(campo headless 2>/dev/null)"
checa "--model junto" '"haiku"' "$(campo model 2>/dev/null)"

# 2. Sem o flag, a chave nem vai (o default do backend é false).
rm -f "$TMP/corpo.json"
bash "$TMP/scripts/hangar-send" --new sem-flag /tmp >/dev/null 2>&1
checa "sem --headless, sem a chave" 'null' "$(campo headless 2>/dev/null)"

# Auxiliares herdam o padrão; gravá-lo exige escolha explícita.
checa "sem flag, não grava provedor" 'null' "$(campo remember_provider 2>/dev/null)"
resultado=$(bash "$TMP/scripts/hangar-send" --new lembrar /tmp --provider codex --remember-provider 2>&1)
checa "--remember-provider no corpo" 'true' "$(campo remember_provider 2>/dev/null)"
[[ "$resultado" == *"aviso-teste"* ]]
checa "aviso da criação aparece" '0' "$?"
[[ "$resultado" == *"agente herdado: claude"* ]]
checa "o agente herdado aparece na criação" '0' "$?"

# 2b. Chamado de uma sessão, sem --provider: o agente fica para o backend herdar da criadora; o
# padrão lembrado do servidor só vale de fora de uma sessão.
mkdir -p "$TMP/hl" && printf '{"name":"criadora","key":"k-criadora"}' > "$TMP/hl/criadora.json"
rm -f "$TMP/corpo.json"
CP_SESSION_KEY=k-criadora HANGAR_HEADLESS_DIR="$TMP/hl" bash "$TMP/scripts/hangar-send" --new herda /tmp >/dev/null 2>&1
checa "de uma sessão: sem provider no corpo" 'null' "$(campo provider 2>/dev/null)"
checa "de uma sessão: a criadora vai" '"criadora"' "$(campo creator 2>/dev/null)"
rm -f "$TMP/corpo.json"
env -u TMUX -u TMUX_PANE -u CP_SESSION_KEY -u HANGAR_CANO_KEY -u CP_SESSION_NAME bash "$TMP/scripts/hangar-send" --new de-fora /tmp >/dev/null 2>&1
checa "de fora: o padrão do servidor" '"claude"' "$(campo provider 2>/dev/null)"

# 2c. Retomar noutra máquina: sem pasta, a pasta vem da conversa (lá), nunca a home de lá.
printf '{"la":{"base_url":"http://127.0.0.1:%s","token":"teste"}}' "$PORTA" > "$TMP/backend/peers.json"
rm -f "$TMP/corpo.json"
bash "$TMP/scripts/hangar-send" --new la::retoma --resume abc123 >/dev/null 2>&1
checa "--resume noutra máquina: código" '0' "$?"
checa "--resume noutra máquina: sem cwd no corpo" 'null' "$(campo cwd 2>/dev/null)"
rm -f "$TMP/backend/peers.json"

# 3. Codex também aceita o modo sem terminal.
rm -f "$TMP/corpo.json"
bash "$TMP/scripts/hangar-send" --new hl-codex /tmp --headless --provider codex --permissao "Full Access" >/dev/null 2>&1
checa "--headless com codex: código" '0' "$?"
checa "--headless com codex: provider" '"codex"' "$(campo provider 2>/dev/null)"
checa "--headless com codex: flag" 'true' "$(campo headless 2>/dev/null)"
checa "--headless com codex: permissão" '"Full Access"' "$(campo permission_mode 2>/dev/null)"

# 4. Repetido é erro, como os outros flags.
bash "$TMP/scripts/hangar-send" --new hl-2x /tmp --headless --headless >/dev/null 2>&1
checa "--headless duas vezes: código" '2' "$?"

# 5. O --help documenta o flag (a ajuda é o cabeçalho do script).
bash "$REPO/scripts/hangar-send" --help 2>/dev/null | grep -q -- '--headless'
checa "--help menciona --headless" '0' "$?"

# 6. HANGAR_SEND_PAINEL=1: aviso de programa vai como está, sem [de: …] e sem procurar o remetente.
rm -f "$TMP/corpo.json"
HANGAR_SEND_PAINEL=1 bash "$TMP/scripts/hangar-send" alvo "[painel: orquestrador g1] oi" >/dev/null 2>&1
checa "painel: código" '0' "$?"
checa "painel: texto intacto" '"[painel: orquestrador g1] oi"' "$(campo text 2>/dev/null)"
checa "painel: orienta o turno" 'true' "$(campo steer 2>/dev/null)"

# 7. Sem o rótulo de painel, ou para outro servidor, recusa sem enviar nada.
rm -f "$TMP/corpo.json"
HANGAR_SEND_PAINEL=1 bash "$TMP/scripts/hangar-send" alvo "oi" >/dev/null 2>&1
checa "painel sem [painel:]: código" '2' "$?"
checa "painel sem [painel:]: nada enviado" 'ausente' "$([[ -e "$TMP/corpo.json" ]] && echo enviado || echo ausente)"
HANGAR_SEND_PAINEL=1 bash "$TMP/scripts/hangar-send" srv::alvo "[painel: x] oi" >/dev/null 2>&1
checa "painel para outro servidor: código" '2' "$?"

# 8. O --help documenta a variável (a ajuda é o cabeçalho do script).
bash "$REPO/scripts/hangar-send" --help 2>/dev/null | grep -q HANGAR_SEND_PAINEL
checa "--help menciona HANGAR_SEND_PAINEL" '0' "$?"

# 9. --contrato: o conteúdo sai limpo no stdout (cabe num arquivo), o mtime e o comando de gravar no
# stderr; --gravar manda o arquivo inteiro com o mtime da leitura, e sem --mtime só cria (null).
sessao=(env CP_SESSION_KEY=k-criadora HANGAR_HEADLESS_DIR="$TMP/hl")
lido=$("${sessao[@]}" bash "$TMP/scripts/hangar-send" --contrato 2>"$TMP/contrato.err")
checa "--contrato: conteúdo no stdout" '"decisão A"' "$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1], ensure_ascii=False))' "$lido")"
grep -q -- '--gravar <arquivo> --mtime 111' "$TMP/contrato.err"
checa "--contrato: comando de gravar com o mtime" '0' "$?"
printf 'decisão B\n' > "$TMP/novo.md"
rm -f "$TMP/corpo.json"
"${sessao[@]}" bash "$TMP/scripts/hangar-send" --contrato --gravar "$TMP/novo.md" --mtime 111 >/dev/null 2>&1
checa "--gravar: código" '0' "$?"
checa "--gravar: rota da sessão" '/api/sessions/criadora/pair/contract' "$(cat "$TMP/corpo.json.rota" 2>/dev/null)"
checa "--gravar: conteúdo" '"decisão B\n"' "$(python3 -c 'import json,sys; print(json.dumps(json.load(open(sys.argv[1]))["content"], ensure_ascii=False))' "$TMP/corpo.json" 2>/dev/null)"
checa "--gravar: mtime da leitura" '"111"' "$(campo mtime 2>/dev/null)"
rm -f "$TMP/corpo.json"
"${sessao[@]}" bash "$TMP/scripts/hangar-send" --contrato --gravar "$TMP/novo.md" >/dev/null 2>&1
checa "--gravar sem --mtime: só cria" 'null' "$(campo mtime 2>/dev/null || echo 'nada enviado')"
"${sessao[@]}" bash "$TMP/scripts/hangar-send" --contrato --mtime 111 >/dev/null 2>&1
checa "--mtime sem --gravar: código" '2' "$?"
bash "$REPO/scripts/hangar-send" --help 2>/dev/null | grep -q -- '--contrato'
checa "--help menciona --contrato" '0' "$?"

# 10. --close servidor::sessao: confere a lista de lá e fecha lá, com o token de lá.
printf '{"la":{"base_url":"http://127.0.0.1:%s","token":"tok-la"},"falha":{"base_url":"http://127.0.0.1:%s","token":"tok-falha"}}' "$PORTA" "$PORTA" > "$TMP/backend/peers.json"
printf 'CP_AUTH_TOKEN=teste\nCP_PORT=1\nCP_SERVER_ID=aqui\n' > "$TMP/backend/.env"
saida=$(bash "$TMP/scripts/hangar-send" --close la::viva 2>&1)
checa "--close remoto: código" '0' "$?"
checa "--close remoto: saída" 'fechada: la::viva' "$saida"
checa "--close remoto: rota de lá" '/api/sessions/viva' "$(cat "$TMP/corpo.json.delete" 2>/dev/null)"
checa "--close remoto: token de lá" 'Bearer tok-la' "$(cat "$TMP/corpo.json.auth" 2>/dev/null)"
rm -f "$TMP/corpo.json.delete"
saida=$(bash "$TMP/scripts/hangar-send" --close la::sumiu 2>&1)
checa "--close remoto sem a sessão: código" '1' "$?"
[[ "$saida" == *"la::sumiu"* ]]
checa "--close remoto sem a sessão: nomeia o alvo" '0' "$?"
checa "--close remoto sem a sessão: nada fechado" 'ausente' "$([[ -e "$TMP/corpo.json.delete" ]] && echo fechado || echo ausente)"
saida=$(bash "$TMP/scripts/hangar-send" --close falha::viva 2>&1)
checa "--close remoto com falha no DELETE: código" '1' "$?"
[[ "$saida" == *"pode ter fechado mesmo assim"* && "$saida" != *"fechada:"* ]]
checa "--close remoto com falha no DELETE: avisa, sem dizer fechada" '0' "$?"
rm -f "$TMP/corpo.json.delete"
bash "$TMP/scripts/hangar-send" --close vps::viva >/dev/null 2>&1
checa "--close servidor desconhecido: código" '1' "$?"
bash "$TMP/scripts/hangar-send" --close a::b::c >/dev/null 2>&1
checa "--close com dois '::': código" '2' "$?"
# O id desta máquina fica aqui: a recusa de fechar a si mesma vale, e nada vai à API.
CP_SESSION_NAME=viva bash "$TMP/scripts/hangar-send" --close aqui::viva >/dev/null 2>&1
checa "--close <id daqui>::eu: recusa" '2' "$?"
checa "--close <id daqui>::eu: nada fechado" 'ausente' "$([[ -e "$TMP/corpo.json.delete" ]] && echo fechado || echo ausente)"

if [[ $falhas -gt 0 ]]; then echo "$falhas falha(s)"; exit 1; fi
echo "tudo ok"
