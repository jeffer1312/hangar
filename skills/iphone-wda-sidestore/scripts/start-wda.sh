#!/usr/bin/env bash
# Liga túnel + WebDriverAgent + porta 8100 do iPhone, destacados do terminal (logs em $LOGS).
HERE=$(dirname "$(readlink -f "$0")")
# Mesmo kit que o wda.sh usa: as ferramentas (ios, certificado) moram nele, não junto deste script.
cd "${IOS_CTL:-$HOME/Applications/ios-ctl}" || exit 1
# Pasta só do usuário e de nome fixo, para quem for diagnosticar achar os logs.
LOGS="${XDG_RUNTIME_DIR:-$HOME/.cache}/ios-ctl"
mkdir -p -m 700 "$LOGS" || exit 1
B=$("$HERE/wda.sh" bundle) || exit 1
# Renova o perfil de 7 dias quando está perto de vencer; sem isso o iPhone recusa abrir o WebDriverAgent.
"$HERE/wda.sh" renew || { echo "não renovei o perfil do WebDriverAgent (veja acima); sigo com o que está no iPhone"; }
# Os processos aparecem como "./ios …" (o link), não "ios-amd64": casar pelo comando, senão se acumulam.
pkill -f '^\./ios(-amd64)? (tunnel start|runwda|forward 8100)'
sleep 1
setsid nohup ./ios tunnel start --userspace > "$LOGS/tunnel.log" 2>&1 &
for i in $(seq 1 60); do ./ios tunnel ls 2>/dev/null | grep -q address && break; sleep 1; done
./ios tunnel ls 2>/dev/null | grep -q address || { tail -3 "$LOGS/tunnel.log"; echo "túnel do iPhone não subiu em 60 s (iPhone no cabo e desbloqueado?)"; exit 1; }
SSL_CERT_FILE="$PWD/ca-com-apple.pem" ./ios image auto > "$LOGS/image.log" 2>&1 \
	|| echo "aviso: ios image auto falhou, veja $LOGS/image.log" >&2
setsid nohup ./ios runwda --bundleid=$B --testrunnerbundleid=$B --xctestconfig=WebDriverAgentRunner.xctest > "$LOGS/wda.log" 2>&1 &
setsid nohup ./ios forward 8100 8100 > "$LOGS/forward.log" 2>&1 &
# A ativação da automação no iPhone pode demorar bem mais que a subida do túnel; espera até 3 min.
for i in $(seq 1 180); do curl -s -m 2 http://127.0.0.1:8100/status | grep -q '"ready" : true' && break; sleep 1; done
curl -s -m 2 http://127.0.0.1:8100/status | grep -q '"ready" : true' || { echo "WebDriverAgent não respondeu; veja $LOGS/wda.log"; exit 1; }
# O WebDriverAgent desliga correção automática e predições do iPhone ao subir; religa pelos Ajustes.
python3 "$HERE/restore-keyboard.py"
echo pronto
