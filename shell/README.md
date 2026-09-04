# Shell Electron

Janela nativa que hospeda o cockpit com a área de trabalho aparecendo atrás.

## Rodar

```bash
cd shell && npm install     # primeira vez
npm start
npm test                    # settings.cjs (leitura/gravação do userData)
```

Ele carrega `http://127.0.0.1:8765` — o backend, que serve a interface. `COCKPIT_URL` aponta
outro endereço. Endereço fora do ar cai no `http://127.0.0.1:8765` antes de perguntar qualquer
coisa; só quando os dois falham é que o app pede o endereço. `Ctrl+Shift+U` reabre essa pergunta.

## Atalho no lançador

`npm start` morre junto com o terminal que o abriu. Pra abrir pelo lançador do desktop, instale o
`.desktop` daqui trocando o marcador pelo caminho real:

```bash
mkdir -p ~/.local/share/applications
sed "s|__SHELL_DIR__|$PWD|g" hangar.desktop > ~/.local/share/applications/hangar.desktop
update-desktop-database ~/.local/share/applications
```

Ele chama o binário do Electron direto, e não `npm`: quem abre pelo lançador não herda o PATH do
shell interativo, então um `npm` instalado por fnm/nvm não seria encontrado.

Pra deixar a janela viva sem ficar presa a um terminal, um serviço transiente do systemd resolve:

```bash
systemd-run --user --collect --unit=cockpit-shell \
  --working-directory=$PWD --setenv=WAYLAND_DISPLAY=$WAYLAND_DISPLAY \
  --setenv=XDG_RUNTIME_DIR=$XDG_RUNTIME_DIR $(which npm) start
systemctl --user stop cockpit-shell     # fechar
```

## Login do Chrome no navegador embutido

O navegador embutido guarda cookies numa partição persistente (`persist:nav`): site que aceita o
login continua logado nas próximas aberturas. O que barra é a **tela de login** do Google e afins,
que recusa navegador com depuração ligada. O botão 🍪 do painel traz os cookies do seu Chrome real
por CDP — já decifrados, sem ler o arquivo `Cookies` do perfil —, TODOS de uma vez (uma conexão =
um diálogo "Permitir depuração remota?" no Chrome; por domínio seria um diálogo por site).

E as SENHAS salvas: ao abrir uma página cujo domínio tem senha no Chrome, o shell preenche
usuário+senha (`senhas_chrome.cjs` lê `Login Data`, decifra `password_value` — AES-128-CBC v10 com
a chave "peanuts" ou a do chaveiro "Chrome Safe Storage"). A senha em claro só existe no processo
MAIN e no campo da página; nunca vai a disco nem a outra máquina. Cookie e senha são coisas
diferentes: cookie = login já feito (sessão); senha = os campos preenchidos. Site sem senha salva
(ex.: um X que você nunca salvou) não tem o que preencher. Precisa da depuração remota ligada no Chrome, e o Chrome 136+ **não aceita mais
`--remote-debugging-port` no perfil padrão** (o flag entra na linha de comando e a porta nunca sobe):
o que liga é o toggle em `chrome://inspect/#remote-debugging` ("Permitir depuração remota para este
navegador"), que grava `DevToolsActivePort` na raiz do perfil — é esse arquivo que o shell lê. O
painel oferece **Abrir a página no Chrome** pra chegar lá. Plano B pra Chromium/Brave antigos: porta
fixa em `settings.json` do userData, campo `chromeCdpPort`; um Chrome headless de automação na porta
(agent-browser na 9222) é recusado. Cookie do Google expira e rotaciona: quando cair, clique de novo.
Nunca lance o Chrome direto do processo do shell sem fechar os descritores: o filho herda o socket do
CDP do shell (9223) e o segura depois de o shell fechar.

## Desfoque atrás da janela

A transparência é do app; o **desfoque** é do compositor, e o app não mexe na sua configuração.

**Hyprland** — a regra casa pelo **título**, que é `hangar` nos dois modos. A classe muda
conforme como o app foi aberto (medido em 05/08/2026): rodando por `npm start` ela é `electron`,
genérica e compartilhada com qualquer outro app Electron da máquina; no AppImage o
`electron-builder` a define como `hangar`. Por isso o título é o critério confiável — ele
vale nos dois. A sintaxe depende de como sua config está escrita:

```
# config .conf (Hyprland clássico)
windowrulev2 = noblur off, title:^(hangar)$
```

```lua
-- config .lua (end-4/dots-hyprland e afins)
hl.window_rule({ match = { title = "^(hangar)$" }, no_blur = false })
```

Medido em 05/08/2026 (Hyprland 0.55.4): **não existe** um rule `blur` pra ligar — só `noblur` pra
desligar, e `blur = true` devolve `unknown field`. Rices que desligam o desfoque de toda janela
(`class = ".*", no_blur = true`, o caso do end-4) exigem que a regra acima venha **depois** da
padrão: no `hyprland.lua` o `custom/` é carregado por último, então é lá que ela vai. E numa config
Lua o `hyprctl keyword` não funciona (`keyword can't work with non-legacy parsers`) — pra testar sem
editar arquivo, use `hyprctl eval`.

A intensidade é global (`decoration:blur:size`/`passes`), não por janela: só dá pra ligar e
desligar. Uma receita calibrada pra barra fina costuma ficar pesada demais numa janela inteira.

**macOS e Windows 11** fazem sozinhos, sem configuração.

**GNOME/KDE** dependem do compositor; sem suporte, a janela fica transparente sem desfoque.

## Empacotar como AppImage

Pra instalar numa máquina Linux que não tem este repositório clonado, gera um AppImage
autocontido (janela + Electron; o backend Python continua de fora, por decisão do projeto):

```bash
cd shell && npm install     # primeira vez
npm run dist                # gera dist/Hangar-<versão>.AppImage
```

O ícone vem de `frontend/public/icons/icon-512.png` (copiado pra `shell/build/icon.png` — não é
gerado nem versionado à parte). `shell/dist/` é saída de build e não deve ser commitado.

**O AppImage continua sendo só a janela.** Rodando numa máquina nova, ele precisa de um cockpit
(backend `uv run python -m app.main`) alcançável do endereço configurado — local (`COCKPIT_URL`
ou o padrão `http://127.0.0.1:8765`) ou via Tailscale/LAN. Sem isso ele cai na tela de recuperação
pedindo o endereço, do mesmo jeito que `npm start` cairia.

## Release (Linux, Windows, macOS)

Empurrar uma tag `v*` (`git tag v0.1.0 && git push origin v0.1.0`) dispara
`.github/workflows/release.yml`: builda o shell nas 3 plataformas
(`ubuntu-latest`, `windows-latest`, `macos-latest`) e anexa AppImage, `.exe`
(NSIS) e `.dmg` na Release do GitHub que corresponde à tag.

**Os binários de Windows e macOS não são assinados** — sem certificado de
code signing, o SmartScreen do Windows e o Gatekeeper do macOS vão avisar que
o app é de origem desconhecida antes de deixar abrir. E o comportamento da
janela nesses dois sistemas (`shell/main.cjs`) foi escrito a partir da
documentação e **nunca rodou de fato** em Windows ou macOS — só o caminho
Linux/AppImage foi testado.

## Fundo

Dentro do shell, a tela de Aparência ganha a opção **Desktop**. Ela usa os mesmos sliders de
Transparência e Solidez das caixas. O slider "Desfoque do fundo" não aparece nesse modo — ali quem
borra é o sistema.
