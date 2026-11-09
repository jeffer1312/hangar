#!/usr/bin/env bash
# Liga o WebDriverAgent do iPhone, destacado do terminal (logs em $LOGS). No cabo: túnel do go-ios +
# porta 8100 local. Sem cabo: túnel do tunneld (Wi-Fi) e o WDA no IP do iPhone. O endereço sai em $LOGS/wda-url.
HERE=$(dirname "$(readlink -f "$0")")
# Mesmo kit que o wda.sh usa: as ferramentas (ios, certificado) moram nele, não junto deste script.
cd "${IOS_CTL:-$HOME/Applications/ios-ctl}" || exit 1
# Pasta só do usuário e de nome fixo, para quem for diagnosticar achar os logs.
LOGS="${XDG_RUNTIME_DIR:-$HOME/.cache}/ios-ctl"
mkdir -p -m 700 "$LOGS" || exit 1
TUNNELD="${PMD3_TUNNELD:-127.0.0.1:49151}"
B=$("$HERE/wda.sh" bundle) || exit 1
D=$("$HERE/wda.sh" device) || exit 1
read -r VIA U <<<"$D"
# Renova o perfil de 7 dias quando está perto de vencer; sem isso o iPhone recusa abrir o WebDriverAgent.
"$HERE/wda.sh" renew || { echo "não renovei o perfil do WebDriverAgent (veja acima); sigo com o que está no iPhone"; }
# Os processos aparecem como "./ios …" (o link), não "ios-amd64": casar pelo comando, senão se acumulam.
pkill -f '^\./ios(-amd64)? (tunnel start|runwda|forward 8100)'
pkill -f 'pymobiledevice3 developer dvt xcuitest'
sleep 1
if [ "$VIA" = tunnel ]; then
	# O runner pelo xcuitest e não pelo `developer wda`: este procura o iPhone no usbmuxd, que sem cabo não existe.
	pymobiledevice3 mounter auto-mount --tunnel "$U" > "$LOGS/image.log" 2>&1 \
		|| echo "aviso: montar a imagem de desenvolvedor falhou, veja $LOGS/image.log" >&2
	setsid nohup pymobiledevice3 developer dvt xcuitest --tunnel "$U" "$B" > "$LOGS/wda.log" 2>&1 &
	IP=$(curl -s -m 3 "http://$TUNNELD/" | python3 -I -c 'import json,sys; print(json.load(sys.stdin)[sys.argv[1]][0]["interface"])' "$U") \
		|| { echo "o tunneld não deu o IP do iPhone"; exit 1; }
	case "$IP" in *:*) IP="[$IP]" ;; esac
	WDA_URL="http://$IP:8100"
else
	setsid nohup ./ios tunnel start --userspace > "$LOGS/tunnel.log" 2>&1 &
	for i in $(seq 1 60); do ./ios tunnel ls 2>/dev/null | grep -q address && break; sleep 1; done
	./ios tunnel ls 2>/dev/null | grep -q address || { tail -3 "$LOGS/tunnel.log"; echo "túnel do iPhone não subiu em 60 s (iPhone no cabo e desbloqueado?)"; exit 1; }
	SSL_CERT_FILE="$PWD/ca-com-apple.pem" ./ios image auto > "$LOGS/image.log" 2>&1 \
		|| echo "aviso: ios image auto falhou, veja $LOGS/image.log" >&2
	setsid nohup ./ios runwda --bundleid=$B --testrunnerbundleid=$B --xctestconfig=WebDriverAgentRunner.xctest > "$LOGS/wda.log" 2>&1 &
	setsid nohup ./ios forward 8100 8100 > "$LOGS/forward.log" 2>&1 &
	WDA_URL="http://127.0.0.1:8100"
fi
# A ativação da automação no iPhone pode demorar bem mais que a subida do túnel; espera até 3 min.
for i in $(seq 1 180); do curl -s -m 2 "$WDA_URL/status" | grep -q '"ready" : true' && break; sleep 1; done
curl -s -m 2 "$WDA_URL/status" | grep -q '"ready" : true' || { echo "WebDriverAgent não respondeu em $WDA_URL; veja $LOGS/wda.log"; exit 1; }
echo "$WDA_URL" > "$LOGS/wda-url"
# O WebDriverAgent desliga correção automática e predições do iPhone ao subir; religa pelos Ajustes.
WDA_URL="$WDA_URL" python3 "$HERE/restore-keyboard.py"
echo "pronto: $WDA_URL"
