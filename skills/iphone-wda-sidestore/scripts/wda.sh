#!/usr/bin/env bash
# WebDriverAgent no iPhone a partir do Linux: estado, login na Apple, instalar e renovar o perfil de 7 dias.
# Uso: wda.sh status | login | instalar | renovar [--forcar]
set -euo pipefail

KIT="${IOS_CTL:-$HOME/Applications/ios-ctl}"
CFG="${XDG_CONFIG_HOME:-$HOME/.config}/PlumeImpactor"
BASE_IPA="${WDA_IPA:-$KIT/wda-base.ipa}"
DIAS_ANTES="${WDA_RENOVAR_DIAS:-3}"
PLUME="$KIT/plumesign"
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

die() { echo "erro: $*" >&2; exit 1; }

conta() { # campo da conta selecionada no accounts.json do Impactor/plumesign
	[ -f "$CFG/accounts.json" ] || die "sem conta salva em $CFG/accounts.json; rode: wda.sh login"
	python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); print(d["accounts"][d["selected_account"]][sys.argv[2]])' "$CFG/accounts.json" "$1" \
		|| die "não consegui ler '$1' de $CFG/accounts.json (erro acima)"
}

# pymobiledevice3 com o erro dele visível: falha de usbmuxd, aparelho bloqueado ou sem confiança não pode
# virar "não instalado" nem JSON vazio.
pmd() {
	pymobiledevice3 "$@" 2>"$TMP/pmd.err" || { tail -3 "$TMP/pmd.err" >&2; die "pymobiledevice3 $1 $2 falhou (iPhone desbloqueado e confiando neste computador?)"; }
}

aparelho() {
	if [ -n "${UDID:-}" ]; then echo "$UDID"; return; fi
	local json lista n
	json=$(pmd usbmux list)
	lista=$(printf '%s' "$json" | python3 -I -c 'import json,sys; [print(d["Identifier"]) for d in json.load(sys.stdin) if d.get("ConnectionType")=="USB"]')
	n=$(printf '%s' "$lista" | grep -c . || true)
	[ "$n" = 1 ] || die "preciso de exatamente 1 iPhone no cabo (achei $n); ou defina UDID=<udid>"
	echo "$lista"
}

wda_instalado() {
	local out
	# `exit` explícito: dentro de `if` o set -e não vale, e a falha viraria "não instalado".
	out=$(pmd apps query "$BUNDLE" --udid "$1") || exit 1
	printf '%s' "$out" | grep -q CFBundleIdentifier
}

# Validade (em dias, com fração) do perfil mais novo do WDA instalado no aparelho; vazio se não houver.
dias_perfil() {
	local json
	json=$(pmd provision list --udid "$1")
	printf '%s' "$json" | python3 -I -c '
import json,sys,datetime
app=sys.argv[1]
fim=[datetime.datetime.fromisoformat(str(p["ExpirationDate"])) for p in json.load(sys.stdin)
     if p.get("Entitlements",{}).get("application-identifier")==app]
if fim:
    m=max(fim); m=m if m.tzinfo else m.replace(tzinfo=datetime.timezone.utc)
    print(round((m-datetime.datetime.now(datetime.timezone.utc)).total_seconds()/86400,1))' "$TEAM.$BUNDLE"
}

# Pede à Apple um perfil novo para o App ID do WDA e o grava em $TMP/wda.mobileprovision.
perfil_novo() {
	[ -f "$BASE_IPA" ] || die "IPA base do WDA não achado em $BASE_IPA (defina WDA_IPA)"
	# Com mais de um time na conta o plumesign abre uma lista que só responde num terminal:
	# roda num pty e desce a seta até a linha do nosso time.
	python3 -I - "$TEAM" "$TMP/plume.log" "$PLUME" sign --apple-id -p "$BASE_IPA" -o "$TMP/assinado.ipa" <<-'EOF' || true
	import os,pty,sys,re,select
	team,log,cmd=sys.argv[1],sys.argv[2],sys.argv[3:]
	pid,fd=pty.fork()
	if pid==0: os.execv(cmd[0],cmd)
	buf=b""; escolhido=False
	with open(log,"wb") as out:
	    while True:
	        r,_,_=select.select([fd],[],[],180)
	        if not r: os.kill(pid,9); break
	        try: data=os.read(fd,65536)
	        except OSError: break
	        if not data: break
	        out.write(data); buf=(buf+data)[-20000:]
	        if not escolhido:
	            itens=re.findall(rb"^[ >] (.+?) \(([A-Z0-9]{10})\)\r?$",re.sub(rb"\x1b\[[0-9;?]*[a-zA-Z]",b"",buf),re.M)
	            ids=list(dict.fromkeys(i[1].decode() for i in itens))
	            if team in ids:
	                os.write(fd,b"\x1b[B"*ids.index(team)+b"\r"); escolhido=True
	os.waitpid(pid,0)
	EOF
	if grep -qiE 'session has expired|error 1100|no account|not logged' "$TMP/plume.log"; then
		echo "a sessão da Apple venceu: rode 'wda.sh login' (senha do Apple ID e, se pedir, o código)" >&2
		exit 3
	fi
	[ -f "$TMP/assinado.ipa" ] || { tail -5 "$TMP/plume.log" >&2; die "o plumesign não gerou o perfil"; }
	python3 -I - "$TMP/assinado.ipa" "$TMP/wda.mobileprovision" "$TEAM.$BUNDLE" "$UDID_" <<-'EOF'
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
certificado() {
	local chave="$CFG/keys/$TEAM/key.pem" i=0 pub
	[ -f "$chave" ] || die "chave não achada em $chave (o Impactor cria ao instalar o primeiro app)"
	pub=$(openssl pkey -in "$chave" -pubout)
	python3 -I - "$TMP/wda.mobileprovision" "$TMP" <<-'EOF'
	import sys,plistlib
	raw=open(sys.argv[1],"rb").read()
	p=plistlib.loads(raw[raw.index(b"<?xml"):raw.index(b"</plist>")+8])
	for i,c in enumerate(p["DeveloperCertificates"]): open(f"{sys.argv[2]}/cert{i}.der","wb").write(c)
	EOF
	for c in "$TMP"/cert*.der; do
		if [ "$(openssl x509 -inform DER -in "$c" -pubkey -noout)" = "$pub" ]; then echo "$c"; return; fi
	done
	die "nenhum certificado do perfil casa com $chave"
}

cmd="${1:-status}"
case "$cmd" in bundle|login|status|instalar|renovar) ;; *) die "uso: wda.sh status | login [apple-id] | instalar | renovar [--forcar] | bundle" ;; esac
[[ $DIAS_ANTES =~ ^[0-9]+([.][0-9]+)?$ ]] || die "WDA_RENOVAR_DIAS precisa ser um número de dias (está '$DIAS_ANTES')"
[ "$cmd" = login ] || { TEAM=$(conta team_id); BUNDLE="com.facebook.WebDriverAgentRunner.xctrunner.$TEAM"; }

case "$cmd" in
bundle) echo "$BUNDLE" ;;
login)
	email="${2:-}"
	[ -n "$email" ] || email=$(conta email 2>/dev/null || true)
	[ -n "$email" ] || die "uso: wda.sh login <apple-id>"
	exec "$PLUME" account login -u "$email"
	;;
status)
	UDID_=$(aparelho)
	echo "aparelho: $UDID_  time: $TEAM"
	if wda_instalado "$UDID_"; then
		d=$(dias_perfil "$UDID_"); echo "WDA instalado; perfil vence em ${d:-?} dias"
	else
		echo "WDA não instalado: rode 'wda.sh instalar'"
	fi
	if "$PLUME" account certificates -t "$TEAM" >"$TMP/s.log" 2>&1; then echo "sessão da Apple: ok"
	elif grep -qiE 'session has expired|error 1100|no account|not logged' "$TMP/s.log"; then echo "sessão da Apple: vencida (rode 'wda.sh login')"
	else echo "sessão da Apple: erro ao consultar:"; grep -v DEBUG "$TMP/s.log" | tail -2; fi
	;;
instalar)
	UDID_=$(aparelho)
	# A Apple devolve o aparelho já registrado sem erro, então registrar sempre é seguro.
	nome=$(pmd lockdown info --udid "$UDID_" | python3 -I -c 'import json,sys; print(json.load(sys.stdin).get("DeviceName","iPhone"))')
	"$PLUME" account register-device -t "$TEAM" -u "$UDID_" -n "$nome" >"$TMP/registro.log" 2>&1 \
		|| { tail -5 "$TMP/registro.log" >&2; die "não consegui registrar o aparelho na conta"; }
	perfil_novo
	cert=$(certificado)
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
	pymobiledevice3 apps install "$TMP/wda.ipa" --udid "$UDID_"
	echo "WDA instalado ($BUNDLE). No primeiro uso: Ajustes > Geral > VPN e Gerenciamento de Dispositivos > confiar no desenvolvedor."
	;;
renovar)
	UDID_=$(aparelho)
	wda_instalado "$UDID_" || die "WDA não instalado: rode 'wda.sh instalar'"
	d=$(dias_perfil "$UDID_")
	if [ "${2:-}" != --forcar ] && [ -n "$d" ] && python3 -I -c 'import sys; sys.exit(0 if float(sys.argv[1]) > float(sys.argv[2]) else 1)' "$d" "$DIAS_ANTES"; then
		echo "perfil do WDA ok: vence em $d dias"; exit 0
	fi
	perfil_novo
	# Só o perfil: o app instalado continua o mesmo, assinado pelo mesmo certificado, e volta a abrir.
	pymobiledevice3 provision install "$TMP/wda.mobileprovision" --udid "$UDID_"
	echo "perfil renovado no aparelho"
	;;
*) die "uso: wda.sh status | login [apple-id] | instalar | renovar [--forcar]" ;;
esac
