#!/usr/bin/env bash
# Liga túnel + WebDriverAgent + porta 8100 do iPhone, destacados do terminal (logs em /tmp/ios-*.log).
cd "$(dirname "$0")"
B=$(./wda.sh bundle) || exit 1
# Renova o perfil de 7 dias quando está perto de vencer; sem isso o iPhone recusa abrir o WebDriverAgent.
./wda.sh renovar || { echo "não renovei o perfil do WebDriverAgent (veja acima); sigo com o que está no iPhone"; }
# Os processos aparecem como "./ios …" (o link), não "ios-amd64": casar pelo comando, senão se acumulam.
pkill -f '^\./ios(-amd64)? (tunnel start|runwda|forward 8100)'
sleep 1
setsid nohup ./ios tunnel start --userspace > /tmp/ios-tunnel.log 2>&1 &
until ./ios tunnel ls 2>/dev/null | grep -q address; do sleep 1; done
SSL_CERT_FILE="$PWD/ca-com-apple.pem" ./ios image auto > /tmp/ios-image.log 2>&1
setsid nohup ./ios runwda --bundleid=$B --testrunnerbundleid=$B --xctestconfig=WebDriverAgentRunner.xctest > /tmp/ios-wda.log 2>&1 &
setsid nohup ./ios forward 8100 8100 > /tmp/ios-forward.log 2>&1 &
# A ativação da automação no iPhone passa de 1 min às vezes; espera até 3.
for i in $(seq 1 180); do curl -s -m 2 http://127.0.0.1:8100/status | grep -q '"ready" : true' && break; sleep 1; done
curl -s -m 2 http://127.0.0.1:8100/status | grep -q '"ready" : true' || { echo "WebDriverAgent não respondeu; veja /tmp/ios-wda.log"; exit 1; }
# O WebDriverAgent desliga correção automática e predições do iPhone ao subir; religa pelos Ajustes.
python3 ./religar-teclado.py
echo pronto
