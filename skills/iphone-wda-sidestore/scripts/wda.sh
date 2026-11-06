#!/usr/bin/env bash
# WebDriverAgent no iPhone a partir do Linux: estado, login na Apple, instalar e renovar o perfil de 7 dias.
# Uso: wda.sh status | login [apple-id] | install | renew [--force] | bundle
set -euo pipefail
# Sem isso o `die` dentro de $(...) não para quem chamou: o erro seguia adiante como saída vazia.
shopt -s inherit_errexit

USAGE="uso: wda.sh status | login [apple-id] | install | renew [--force] | bundle"
KIT="${IOS_CTL:-$HOME/Applications/ios-ctl}"
CFG="${XDG_CONFIG_HOME:-$HOME/.config}/PlumeImpactor"
BASE_IPA="${WDA_IPA:-$KIT/wda-base.ipa}"
RENEW_DAYS="${WDA_RENEW_DAYS:-3}"
PLUME="$KIT/plumesign"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

die() { echo "erro: $*" >&2; exit 1; }

account_field() { # campo da conta selecionada no accounts.json do Impactor/plumesign
	[ -f "$CFG/accounts.json" ] || die "sem conta salva em $CFG/accounts.json; rode: wda.sh login <apple-id>"
	python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["accounts"][d["selected_account"]][sys.argv[2]])' "$CFG/accounts.json" "$1" \
		|| die "não consegui ler '$1' de $CFG/accounts.json (erro acima)"
}

# pymobiledevice3 com o erro dele visível: falha de usbmuxd, aparelho bloqueado ou sem confiança não pode
# virar "não instalado" nem JSON vazio.
pmd() {
	pymobiledevice3 "$@" 2>"$TMP/pmd.err" || { tail -3 "$TMP/pmd.err" >&2; die "pymobiledevice3 $1 $2 falhou (iPhone desbloqueado e confiando neste computador?)"; }
}

device_udid() {
	if [ -n "${UDID:-}" ]; then echo "$UDID"; return; fi
	local json list n
	json=$(pmd usbmux list)
	list=$(printf '%s' "$json" | python3 -I -c 'import json,sys; [print(d["Identifier"]) for d in json.load(sys.stdin) if d.get("ConnectionType")=="USB"]')
	n=$(printf '%s' "$list" | grep -c . || true)
	[ "$n" = 1 ] || die "preciso de exatamente 1 iPhone no cabo (achei $n); ou defina UDID=<udid>"
	echo "$list"
}

wda_installed() {
	local out
	# `exit` explícito: dentro de `if` o set -e não vale, e a falha viraria "não instalado".
	out=$(pmd apps query "$BUNDLE" --udid "$1") || exit 1
	printf '%s' "$out" | grep -q CFBundleIdentifier
}

# Validade (em dias, com fração) do perfil mais novo do WDA instalado no aparelho; vazio se não houver.
profile_days_left() {
	local json
	json=$(pmd provision list --udid "$1")
	printf '%s' "$json" | python3 -I -c '
import json,sys,datetime
app=sys.argv[1]
ends=[datetime.datetime.fromisoformat(str(p["ExpirationDate"])) for p in json.load(sys.stdin)
      if p.get("Entitlements",{}).get("application-identifier")==app]
if ends:
    m=max(ends); m=m if m.tzinfo else m.replace(tzinfo=datetime.timezone.utc)
    print(round((m-datetime.datetime.now(datetime.timezone.utc)).total_seconds()/86400,1))' "$TEAM.$BUNDLE"
}

# "LOGIN_PROMPT" é escrito pelo pty abaixo quando o plumesign pede senha ou código.
session_expired() { grep -qiE 'session has expired|error 1100|no account|not logged|LOGIN_PROMPT' "$1"; }

ask_login() {
	echo "a sessão da Apple venceu: rode 'wda.sh login' (senha do Apple ID e, se pedir, o código)" >&2
	exit 3
}

# Pede à Apple um perfil novo para o App ID do WDA e o grava em $TMP/wda.mobileprovision.
fetch_profile() {
	[ -f "$BASE_IPA" ] || die "IPA base do WDA não achado em $BASE_IPA (defina WDA_IPA)"
	# Com mais de um time na conta o plumesign abre uma lista que só responde num terminal:
	# roda num pty e desce a seta até a linha do nosso time. Pedido de senha ou código não tem
	# resposta aqui: encerra na hora em vez de esperar o prazo.
	python3 -I - "$TEAM" "$TMP/plume.log" "$PLUME" sign --apple-id -p "$BASE_IPA" -o "$TMP/signed.ipa" <<-'EOF' || true
	import os,pty,sys,re,select
	team,log,cmd=sys.argv[1],sys.argv[2],sys.argv[3:]
	pid,fd=pty.fork()
	if pid==0: os.execv(cmd[0],cmd)
	buf=b""; picked=False
	with open(log,"wb") as out:
	    while True:
	        r,_,_=select.select([fd],[],[],180)
	        if not r: os.kill(pid,9); break
	        try: data=os.read(fd,65536)
	        except OSError: break
	        if not data: break
	        out.write(data); buf=(buf+data)[-20000:]
	        plain=re.sub(rb"\x1b\[[0-9;?]*[a-zA-Z]",b"",buf)
	        if re.search(rb"Enter (password|2FA|the code)",plain):
	            out.write(b"\nLOGIN_PROMPT\n"); os.kill(pid,9); break
	        if not picked:
	            items=re.findall(rb"^[ >] (.+?) \(([A-Z0-9]{10})\)\r?$",plain,re.M)
	            ids=list(dict.fromkeys(i[1].decode() for i in items))
	            if team in ids:
	                os.write(fd,b"\x1b[B"*ids.index(team)+b"\r"); picked=True
	os.waitpid(pid,0)
	EOF
	session_expired "$TMP/plume.log" && ask_login
	[ -f "$TMP/signed.ipa" ] || { tail -5 "$TMP/plume.log" >&2; die "o plumesign não gerou o perfil"; }
	python3 -I - "$TMP/signed.ipa" "$TMP/wda.mobileprovision" "$TEAM.$BUNDLE" "$udid" <<-'EOF'
	import zipfile,sys,plistlib
	ipa,out,app,udid=sys.argv[1:]
	z=zipfile.ZipFile(ipa)
	raw=z.read(next(n for n in z.namelist() if n.endswith(".app/embedded.mobileprovision")))
	p=plistlib.loads(raw[raw.index(b"<?xml"):raw.index(b"</plist>")+8])
	got=p["Entitlements"]["application-identifier"]
	if got!=app: sys.exit(f"erro: perfil saiu para {got}, esperado {app} (time errado na lista do plumesign?)")
	if udid not in p.get("ProvisionedDevices",[]): sys.exit(f"erro: o perfil não inclui o aparelho {udid}")
	open(out,"wb").write(raw)
	print("perfil novo vale até", p["ExpirationDate"])
	EOF
}

# Certificado do perfil que casa com a chave do Impactor/plumesign: é com ele que tudo é assinado.
signing_cert() {
	local key="$CFG/keys/$TEAM/key.pem" pub
	[ -f "$key" ] || die "chave não achada em $key (o Impactor cria ao instalar o primeiro app)"
	pub=$(openssl pkey -in "$key" -pubout)
	python3 -I - "$TMP/wda.mobileprovision" "$TMP" <<-'EOF'
	import sys,plistlib
	raw=open(sys.argv[1],"rb").read()
	p=plistlib.loads(raw[raw.index(b"<?xml"):raw.index(b"</plist>")+8])
	for i,c in enumerate(p["DeveloperCertificates"]): open(f"{sys.argv[2]}/cert{i}.der","wb").write(c)
	EOF
	for c in "$TMP"/cert*.der; do
		if [ "$(openssl x509 -inform DER -in "$c" -pubkey -noout)" = "$pub" ]; then echo "$c"; return; fi
	done
	die "nenhum certificado do perfil casa com $key"
}

cmd="${1:-status}"
case "$cmd" in bundle|login|status|install|renew) ;; *) die "$USAGE" ;; esac

if [ "$cmd" = login ]; then
	email="${2:-}"
	[ -n "$email" ] || email=$(account_field email 2>/dev/null) || true
	[ -n "$email" ] || die "uso: wda.sh login <apple-id>"
	"$PLUME" account login -u "$email"
	exit
fi

TEAM=$(account_field team_id)
BUNDLE="com.facebook.WebDriverAgentRunner.xctrunner.$TEAM"

case "$cmd" in
bundle) echo "$BUNDLE" ;;
status)
	udid=$(device_udid)
	echo "aparelho: $udid  time: $TEAM"
	if wda_installed "$udid"; then
		d=$(profile_days_left "$udid"); echo "WDA instalado; perfil vence em ${d:-?} dias"
	else
		echo "WDA não instalado: rode 'wda.sh install'"
	fi
	if "$PLUME" account certificates -t "$TEAM" >"$TMP/session.log" 2>&1; then echo "sessão da Apple: ok"
	elif session_expired "$TMP/session.log"; then echo "sessão da Apple: vencida (rode 'wda.sh login')"
	else echo "sessão da Apple: erro ao consultar:"; grep -v DEBUG "$TMP/session.log" | tail -2; fi
	;;
install)
	udid=$(device_udid)
	# A Apple devolve o aparelho já registrado sem erro, então registrar sempre é seguro.
	name=$(pmd lockdown info --udid "$udid" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("DeviceName","iPhone"))')
	"$PLUME" account register-device -t "$TEAM" -u "$udid" -n "$name" >"$TMP/register.log" 2>&1 \
		|| { session_expired "$TMP/register.log" && ask_login; tail -5 "$TMP/register.log" >&2; die "não consegui registrar o aparelho na conta"; }
	fetch_profile
	cert=$(signing_cert)
	# O .dSYM dentro de PlugIns quebra a instalação; sai antes de assinar.
	python3 -I - "$BASE_IPA" "$TMP/base.ipa" <<-'EOF'
	import zipfile,sys
	src,dst=sys.argv[1:]
	with zipfile.ZipFile(src) as a, zipfile.ZipFile(dst,"w",zipfile.ZIP_DEFLATED) as b:
	    for i in a.infolist():
	        if ".dSYM/" not in i.filename and not i.filename.endswith(".dSYM"): b.writestr(i,a.read(i))
	EOF
	# zsign e não o Impactor: ele troca o ID do WebDriverAgentRunner.xctest e o WDA cai com erro 103;
	# o plumesign nem assina esse pacote, só serve para buscar o perfil.
	"$KIT/zsign" -k "$CFG/keys/$TEAM/key.pem" -c "$cert" -m "$TMP/wda.mobileprovision" -b "$BUNDLE" -o "$TMP/wda.ipa" "$TMP/base.ipa" >"$TMP/zsign.log" 2>&1 \
		|| { tail -5 "$TMP/zsign.log" >&2; die "zsign falhou"; }
	pymobiledevice3 apps install "$TMP/wda.ipa" --udid "$udid"
	echo "WDA instalado ($BUNDLE). No primeiro uso: Ajustes > Geral > VPN e Gerenciamento de Dispositivos > confiar no desenvolvedor."
	;;
renew)
	[[ $RENEW_DAYS =~ ^[0-9]+([.][0-9]+)?$ ]] || die "WDA_RENEW_DAYS precisa ser um número de dias (está '$RENEW_DAYS')"
	udid=$(device_udid)
	wda_installed "$udid" || die "WDA não instalado: rode 'wda.sh install'"
	d=$(profile_days_left "$udid")
	if [ "${2:-}" != --force ] && [ -n "$d" ] && python3 -I -c 'import sys; sys.exit(0 if float(sys.argv[1]) > float(sys.argv[2]) else 1)' "$d" "$RENEW_DAYS"; then
		echo "perfil do WDA ok: vence em $d dias"; exit 0
	fi
	fetch_profile
	# Só o perfil: o app instalado continua o mesmo, assinado pelo mesmo certificado, e volta a abrir.
	pymobiledevice3 provision install "$TMP/wda.mobileprovision" --udid "$udid"
	echo "perfil renovado no aparelho"
	;;
esac
