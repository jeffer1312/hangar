---
name: iphone-wda-sidestore
description: Instala e mantém, a partir do Linux, o SideStore e o WebDriverAgent (WDA) num iPhone com conta Apple gratuita, e renova o WDA antes de vencer (o perfil grátis dura 7 dias). Use sempre que pedirem para instalar, reinstalar ou renovar o SideStore, o WebDriverAgent, o LocalDevVPN ou um .ipa no iPhone pelo Linux; quando o WDA "sumiu", "venceu", "não abre", dá erro 103 ou a porta 8100 não responde; quando falarem em Impactor, plumesign, zsign, go-ios, ios-ctl, start-wda.sh ou ligar.sh; e para montar esse ambiente para outra pessoa, mesmo que ela não diga "SideStore" nem "WDA".
---

# iPhone pelo Linux: SideStore + WebDriverAgent

Conta Apple gratuita assina apps por **7 dias**. Duas peças resolvem isso:

- **SideStore** renova sozinho, no próprio iPhone, os apps comuns que ele instalou (o app do Hangar, por exemplo).
- **WebDriverAgent** não entra no SideStore. Ele renova pelo `scripts/wda.sh`, a partir do Linux, e o `start-wda.sh` já chama a renovação toda vez que sobe o WDA.

Por que o WDA fica de fora: o WDA tem um pacote de teste (`PlugIns/WebDriverAgentRunner.xctest`) e o SideStore 0.7 só aceita extensões `.appex` em `PlugIns/`. Ele recusa com "A provisioning profile for the app could not be found" ou "extension 'WebDriverAgentRunner.xctest' not found in targetAppBundle" (`InstallAppOperation.swift:301`). Mexer no IPA não contorna isso.

## Primeiro: ver o estado

```bash
~/Applications/ios-ctl/wda.sh status
```

A saída diz se há um iPhone no cabo, se o WDA está instalado, em quantos dias o perfil vence e se a sessão da Apple vale. Com isso você pula direto pro passo que falta. O kit fica em `~/Applications/ios-ctl` (troque com `IOS_CTL=`). Se o kit não existir, comece pela montagem.

## 1. Montar o kit (uma vez por máquina)

Junte numa pasta (`~/Applications/ios-ctl`):

| Arquivo | De onde vem | Para quê |
|---|---|---|
| `Impactor.AppImage` | release do github.com/khcrysalis/PlumeImpactor | Instalar o SideStore, só tem tela gráfica |
| `plumesign` | mesma release (`plumesign-linux-x86_64`), renomeado e com `chmod +x` | Login na Apple e perfil novo sem tela |
| `zsign` | github.com/zhlynn/zsign, release `zsign-linux-x86_64.tar.gz` | Assinar o WDA sem trocar o ID do pacote de teste |
| `ios` | github.com/danielpaulus/go-ios, release `go-ios-linux.zip` (`ios-amd64`, link `ios`) | Túnel, imagem de desenvolvedor e `runwda` |
| `wda-base.ipa` | github.com/appium/WebDriverAgent, release `WebDriverAgentRunner-Runner.zip`: ponha o `.app` dentro de `Payload/` e zipe como `.ipa` | O WDA sem assinatura |
| `ca-com-apple.pem` | os certificados do sistema mais o "Apple Root CA" (receita abaixo) | `ios image auto` baixar a imagem de desenvolvedor |
| `wda.sh`, `start-wda.sh`, `restore-keyboard.py` | links para os `scripts/` desta skill (vem com o Hangar) | Instalar, renovar e subir |

Instale também o `pymobiledevice3` (`pipx install pymobiledevice3`) e o `openssl`.

```bash
K=~/Applications/ios-ctl; S=~/.claude/skills/iphone-wda-sidestore/scripts
ln -sf $S/wda.sh $S/start-wda.sh $S/restore-keyboard.py $K/
# o Apple Root CA vem da Apple em DER; o ios image auto precisa dele junto dos certificados do sistema
curl -fsSLo $K/apple-root.cer https://www.apple.com/appleca/AppleIncRootCertificate.cer
openssl x509 -inform DER -in $K/apple-root.cer -out $K/apple-root.pem
cat /etc/ssl/certs/ca-certificates.crt $K/apple-root.pem > $K/ca-com-apple.pem
```

`restore-keyboard.py` procura os rótulos em português ("Correção Automática", "Texto Preditivo"). Com o iPhone em outra língua, ele só avisa e não religa nada.

## 2. SideStore no iPhone (pelo Impactor, com a pessoa)

Faça pelo Impactor, e não por outro instalador. Ele grava o próprio certificado dentro do SideStore (`ALTCertificate.p12`), e aí o SideStore, os apps dele e o WDA usam o mesmo certificado. Com certificados diferentes, um derruba o outro quando a Apple revoga o mais antigo.

1. Baixe o `.ipa` do SideStore nas releases de github.com/SideStore/SideStore.
2. iPhone no cabo e desbloqueado, "Confiar neste computador" aceito.
3. Abra o `Impactor.AppImage`, entre com o Apple ID, toque em **Import .ipa / .tipa**, escolha o SideStore e depois **Install**.
4. Na aba de utilitários do Impactor, toque em **Install Pairing** no SideStore. Isso grava o arquivo de pareamento que o SideStore usa para se renovar sozinho.
5. No iPhone, ligue o **Modo Desenvolvedor** (Ajustes > Privacidade e Segurança) e reinicie quando ele pedir.
6. Confie no desenvolvedor em Ajustes > Geral > VPN e Gerenciamento de Dispositivos.
7. Instale o **LocalDevVPN** pela App Store e deixe ligado. É ele que deixa o SideStore se renovar sem o computador. O iPhone só mantém uma VPN ligada por vez: com o LocalDevVPN ligado, o Tailscale desliga.
8. Abra o SideStore. Se ele pedir o Apple ID, use o mesmo do Impactor.

Apps comuns (o `.ipa` do Hangar, por exemplo) entram pelo SideStore: **My Apps > +**, escolhendo o arquivo no app Arquivos.

Limites da conta grátis: **3 apps assinados ao mesmo tempo** (SideStore + 2; com o Hangar e o WDA fecha a conta) e **10 App IDs a cada 7 dias**, que o SideStore mostra em "App IDs Remaining".

## 3. Login da Apple para o `plumesign`

O `wda.sh` pede perfil novo pelo `plumesign`, que reaproveita a sessão salva em `~/.config/PlumeImpactor/accounts.json`. É o mesmo arquivo do Impactor, com a mesma chave e o mesmo certificado, então ele não cria certificado novo. Quando a sessão vence, o `wda.sh` sai com código 3 e a mensagem "a sessão da Apple venceu". Aí é preciso entrar de novo, com a senha e, às vezes, o código de verificação.

- **Pessoa no terminal:** `~/Applications/ios-ctl/wda.sh login`. Digite a senha. Se o código não chegar no aparelho, digite `sms` e depois os 6 dígitos.
- **Agente com o cofre do Hangar:** a senha nunca passa pelo chat. Peça com `request_secret`, se ela não estiver no cofre (`vault_list`). Depois rode o login lendo de um FIFO e mande a senha por `vault_exec`:

  ```bash
  # no Bash: pasta privada e entrada só do usuário; para aqui se algo falhar. Anote o caminho impresso.
  P=$(mktemp -d) && mkfifo -m 600 "$P/login.in" && echo "$P"
  # no vault_exec (env APPLE_PW = entrada do cofre com a senha do Apple ID), com P=<caminho anotado>:
  setsid nohup sleep 900 > $P/login.in & echo $! > $P/login.espera; \
  setsid nohup ~/Applications/ios-ctl/plumesign account login -u <apple-id> < $P/login.in > $P/login.log 2>&1 & \
  printf '%s\n' "$APPLE_PW" > $P/login.in; sleep 20; cat $P/login.log
  ```

  Se o log pedir código, pergunte à pessoa e mande com `printf '<código>\n' > $P/login.in`. Se ela não recebeu, mande `sms` antes. Confira "Successfully logged in" no log e encerre a espera com `kill $(cat $P/login.espera)`. Não use `pkill -f "sleep 900"`, que mata também o shell que roda o pkill.

## 4. WebDriverAgent

```bash
~/Applications/ios-ctl/wda.sh install    # registra o iPhone na conta, pede perfil, assina com zsign e instala
~/Applications/ios-ctl/wda.sh renew      # só se faltar <= 3 dias (WDA_RENEW_DAYS); --force renova já
~/Applications/ios-ctl/start-wda.sh      # renova se preciso, sobe túnel + WDA + porta 8100 (logs em $XDG_RUNTIME_DIR/ios-ctl)
curl -s http://127.0.0.1:8100/status     # "ready" : true
```

- O `install` usa o zsign de propósito. O Impactor troca o ID do `WebDriverAgentRunner.xctest`, e aí o WDA instala mas não sobe (erro 103). O `plumesign` nem assina esse pacote. Por isso ele só serve para buscar o perfil. O zsign mantém `com.facebook.WebDriverAgentRunner`. O IPA base vai sem o `.dSYM`, que o script tira.
- O `renew` instala **só o perfil novo** no iPhone. O app continua o mesmo, assinado pelo mesmo certificado, e volta a abrir mesmo que já tenha vencido. Se depois disso o WDA não subir (certificado trocado, app apagado), rode `install`.
- Primeiro uso num iPhone: confie no desenvolvedor em Ajustes > Geral > VPN e Gerenciamento de Dispositivos.
- Com mais de um iPhone no cabo: `UDID=<udid> wda.sh ...`. O `start-wda.sh` não escolhe aparelho, então deixe só um conectado.
- Conta com mais de um time (pessoal + empresa): o `wda.sh` escolhe o time salvo na conta (`team_id`) na lista do `plumesign` e confere o perfil que volta.

## Erros já vistos

| Mensagem | O que é | Saída |
|---|---|---|
| `Developer API error 1100: Your session has expired` / `wda.sh` código 3 | Sessão da Apple vencida | Passo 3 |
| SideStore: "A provisioning profile for the app could not be found" ou "extension 'WebDriverAgentRunner.xctest' not found" | O SideStore não aceita o WDA | Use `wda.sh install` |
| WDA instala mas `runwda` falha com 103 | Pacote de teste com o ID trocado (instalado pelo Impactor ou pelo SideStore) | `wda.sh install` |
| `IO error: not a terminal` no `plumesign sign` | Lista de times sem terminal | Use o `wda.sh`, que já roda num terminal falso |
| App do SideStore some depois de uns dias | Assinado com outro certificado ou sem o LocalDevVPN | Reinstalar pelo Impactor (passo 2) e deixar o LocalDevVPN ligado |
