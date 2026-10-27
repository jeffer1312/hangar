# Como usar o hangar

Guia de uso ponta-a-ponta: instalar, conectar o celular (LAN ou Tailscale), instalar
como PWA e operar o chat. Site: [hangar.dev.br](https://hangar.dev.br). Pra arquitetura ver o
[README](../README.md).

> **Modelo:** ferramenta pessoal, single-user, **LAN/VPN-only**. Roda o `claude` **como
> você** (bypass) → um host exposto é execução-remota-como-você. A trava é o **token**.
> A porta principal (8765) NUNCA vai pra internet pública: nada de port-forward nem túnel
> público. Fora de casa = **VPN (Tailscale)**. A única porta que vai à internet é a do
> convidado (8766, publicada pelo Tailscale Funnel em 8443) quando você compartilha uma sessão
> ou aceita um par externo: ela recusa o token do dono e só alcança o que foi compartilhado.

---

## 1. Pré-requisitos

- `tmux`, Python 3.14 + [`uv`](https://docs.astral.sh/uv/), Node 20+ (o instalador põe o que
  faltar).
- Pelo menos um agente de código: Claude Code (o padrão), Codex, Pi, omp ou Kimi Code.
- Celular na **mesma rede** do PC (Wi-Fi) **ou** ambos no **mesmo tailnet** (Tailscale).

**Pelo app (Linux e Windows):** baixe o app de desktop na
[release `native-latest`](https://github.com/jeffer1312/hangar/releases/tag/native-latest) e abra.
Numa máquina sem Hangar ele oferece instalar o servidor em poucas telas, sem terminal. Detalhes
em [App de desktop](#app-de-desktop-nativo).

**Pelo terminal, é uma linha só.** No Linux e no Windows ela instala o **pacote pronto** do Hangar
(código, Python próprio e servidor Rust do mesmo commit) em `~/.hangar`, sem `git`, `uv` nem build
da tela, e roda o instalador que vem nele:

```bash
# Linux/macOS
curl -fsSL https://raw.githubusercontent.com/jeffer1312/hangar/main/bootstrap.sh | bash
curl -fsSL …/bootstrap.sh | bash -s -- --beta   # canal de teste
curl -fsSL …/bootstrap.sh | bash -s -- --dev    # pelo git, para editar o código (modo desenvolvedor)
```

Com `--dev` (ou dando uma pasta) ela clona o repositório em `~/hangar` e chama o instalador do
checkout, como antes; a máquina fica em modo desenvolvedor. Sistema sem pacote publicado (macOS,
hoje) segue pelo git e diz o motivo. Rodar de novo numa máquina já instalada pelo pacote só refaz o
instalador dela; atualizar é pelo botão Atualizar do app.

```powershell
# Windows
irm https://raw.githubusercontent.com/jeffer1312/hangar/main/bootstrap.ps1 | iex
$env:CP_CANAL = 'beta'; irm …/bootstrap.ps1 | iex   # canal de teste
$env:CP_DEV = '1'; irm …/bootstrap.ps1 | iex        # pelo git, para editar o código (modo desenvolvedor)
```

Outra pasta de destino, ou flags do `install.sh`, vão **depois de `-s --`** (sob `curl | bash` é
esse separador que impede o próprio bash de comê-las):

```bash
curl -fsSL …/bootstrap.sh | bash -s -- --dev ~/apps/hangar --no-frontend
curl -fsSL …/bootstrap.sh | bash -s -- --check      # só confere uma instalação existente e sai
```

No Windows as opções são variáveis definidas **antes** da linha do `irm` — sob `irm | iex` o script
chega como texto e não recebe argumento nenhum. `$env:CP_DESTINO = 'D:\hangar'` escolhe a pasta do
checkout e, como o `CP_DEV`, instala pelo git. Rodar de novo é seguro: pelo git, se a pasta já for
este repositório ele faz `git pull` em vez de clonar; se for outra coisa, ele **para** em vez de
mexer no que é seu.

Prefere ver o que está rodando antes? Clone na mão — dá no mesmo:

```bash
git clone https://github.com/jeffer1312/hangar && cd hangar
./install.sh                                       # ou --check pra só listar o que falta
powershell -ExecutionPolicy Bypass -File install.ps1   # Windows
powershell -ExecutionPolicy Bypass -File install.ps1 -SoChecar
```

Instale num **disco local, nunca numa pasta compartilhada por rede** (`\\servidor\...`, Samba/NFS
montado): o `uv sync` e o `npm ci` recriam `backend/.venv` e `frontend/node_modules` dentro da
pasta, e numa share esses dois são da máquina de ORIGEM — medido, o venv aponta pra
`/usr/bin/python3.14` dela e o `node_modules` traz o binário `@esbuild/linux-x64` dela. Instalar a
partir de uma segunda máquina quebra a instalação da primeira.

**No Windows** o instalador põe tudo de pé (multiplexador, Claude Code, Python, Node, `uv`), pede
o token, libera o firewall, oferece Tailscale e registra o backend pra subir no logon — terminando
com uma checagem que prova que o backend sobe de verdade.

Lá o multiplexador é o [psmux](https://github.com/psmux/psmux) (tmux nativo de Windows, sobre
ConPTY) — não existe `tmux` no Windows, e o WSL não é necessário. O `hangar-send` (recado/pareamento
entre sessões) e o `claude-conta` vão junto, via o bash do Git for Windows. O que **não** vai
está listado em "O que o Windows ainda não tem", mais abaixo.

O instalador faz duas perguntas no começo (a senha do celular e se você vai usar fora de
casa) e depois segue sozinho; só pede a senha de administrador avisando antes. No fim ele
mostra um QR: leia com a câmera do celular.
O instalador baixa também o app de desktop nativo da sua plataforma (release `native-latest`);
no Windows ele cria o atalho **Hangar** (para o `Hangar.exe`) no Menu Iniciar e na Área de
Trabalho, e no Linux o lançador "Hangar". Sem app para a máquina, ou se o download falhar, a
instalação segue e o Hangar abre no navegador.
No Windows, pode usar PowerShell comum ou **Executar como administrador**. A instalação elevada
configura backend e atualização para usar administrador. A comum pede UAC só para o que
precisar (firewall, Modo Desenvolvedor). Uma instalação elevada também exige PowerShell elevado
para atualizar manualmente; o botão Atualizar já herda a permissão do backend.
O backend continua no Agendador, sem serviço do Windows: a tarefa acompanha o Python e tenta
reiniciar até três vezes, com intervalo de um minuto, quando ele encerra com erro. A vigia
verifica a resposta HTTP a cada cinco minutos e recupera travamentos, respeitando a instalação
e a atualização. Fechar o app não encerra o backend; ainda é necessário estar logado.
Quer escolher cada extra? No checkout: `./install.sh --avancado` / `.\install.ps1 -Avancado`
(o `bootstrap.ps1` não repassa argumentos; o `bootstrap.sh` aceita `bash -s -- --avancado`).
Agentes de código: o instalador usa os que já estão no computador e só instala o Claude Code
quando não acha nenhum. Para escolher: `./install.sh --agentes=codex,pi` /
`.\install.ps1 -Agentes codex,pi` (aceita `claude`, `codex`, `pi`, `omp` e `kimi`); o
`--avancado` também pergunta. Os outros se instalam depois pelo painel Harnesses do app.
Algo não abriu? `hangar-doctor` diz o que falta e como consertar.

### hangar-doctor

Diagnóstico read-only, chamado por `--check`/`-SoChecar` e disponível a qualquer hora
(`hangar-doctor` no PATH depois do instalador, ou `uv run --no-sync python -m app.doctor`
dentro de `backend/`). Cada linha é um `ok`/`aviso`/`erro` com o conserto ao lado quando
falha:

- token de acesso definido
- Hangar respondendo na porta configurada
- multiplexador de terminal (tmux) no PATH
- pelo menos um agente de código; com o Claude Code instalado, ele precisa estar logado
- Tailscale: instalado / logado / publicado (três estados — cada um falha sozinho: pode
  estar instalado e sem login, ou logado e sem o `serve` publicado)
- endereço da rede local responde (celular no mesmo Wi-Fi)

### O que o Windows ainda não tem

- Wrappers do `pi`, do `omp` e do `kimi`, e a extensão `hangar-state.ts` do Pi. Sessão Pi, omp
  ou Kimi aberta por você no terminal não aparece; criada pelo app, funciona. (`claude`,
  `claude-conta` e `codex` têm wrapper no perfil do PowerShell.)
- Motor de modelo (Contas e provedores → Modelo e opções / `CP_ENGINE`) funciona: o
  `hangar-engine` roda o comando por subprocess no Windows (o `exec` com env crasha lá).
- Resurrect/continuum (sessões sobreviverem a reboot): são plugins de tmux em bash, e o
  psmux não roda plugin de tmux. Fechou o Windows, as sessões se foram.

### Atualizar, dados e modo desenvolvedor

Máquina migrada para o **pacote por versão** não usa mais `git pull` para atualizar. O botão
**Atualizar** baixa o pacote do seu sistema (código, Python próprio e servidor Rust do mesmo
commit), confere, instala ao lado do que está rodando em `~/.hangar/versions/<versão>`, troca o
apontador `~/.hangar/current` e reinicia. Se o servidor novo não subir, volta sozinho para a versão
anterior e mostra o motivo. Com as automações ligadas, ele procura versão nova de hora em hora. Não
precisa de `git`, `uv` nem `npm` na máquina; o Node continua necessário.

**Seus dados ficam em `~/.hangar/config/`** (no Windows, `%USERPROFILE%\.hangar\config\`): o
`.env` com o token, `peers.json`, `projects.json` e `launchers/`. As pastas de versão podem ser
trocadas e apagadas sem perder nada disso. Máquina ainda não migrada continua com esses arquivos em
`backend/`.

**Modo desenvolvedor** é a máquina que roda de um checkout git que você edita: o
`~/.hangar/current` aponta para o checkout, e o Atualizar faz `git pull --ff-only` na branch dele e
reinicia, sem recompilar nada. Com edição local ou commit à frente, ele recusa e diz o motivo. A
migração escolhe esse modo sozinha quando o checkout tem edição, arquivo novo, commit à frente, está
fora da `main` ou serve a tela pelo `vite`. Depois, para editar o código de uma máquina em pacote:
`hangar-server dev-mode on <pasta do checkout>`; para voltar à versão instalada mais nova:
`hangar-server dev-mode off`. Os dois reiniciam o servidor e, se ele não voltar, desfazem a troca.
Para voltar tudo ao checkout: `hangar-server migrate-package --undo` (o checkout nunca é apagado).

**Tela web neste servidor** (nas configurações do servidor, no app nativo e no app do celular)
liga ou desliga a página do Hangar no navegador por esta máquina. Vem ligada. Desligada, o
navegador e o PWA deixam de abrir por ela e a atualização não baixa os arquivos da tela; o app
nativo e o do celular continuam funcionando.

## 2. Subir na mão

O instalador já deixa tudo rodando como serviço (systemd de usuário no Linux, Agendador no
Windows). Isto é para quem roda do checkout sem ele.

**a) Claude ou Codex com terminal, gerenciado dentro do tmux**:
```bash
tmux new -s cc        # rode `claude` dentro dela
```

Com o wrapper recomendado instalado (`./scripts/install-claude-wrapper.sh`), basta executar
`claude` ou `codex` normalmente. O `codex` pede ao backend uma sessão gerenciada e anexa o terminal
ao tmux dela; a conversa aparece imediatamente no app. `command codex` ignora o wrapper.
Cores erradas (teal/pink) no tmux? Fix em [tmux-truecolor-setup.md](tmux-truecolor-setup.md).
Sobreviver a reboot/OOM? `./scripts/tmux-persist-setup.sh` ([doc](tmux-persistence-setup.md)).

**b) Tela do app** (PWA), buildada uma vez, na raiz do repositório:
```bash
npm ci --workspace=@hangar/core --workspace=frontend
npm run build -w frontend          # gera frontend/dist, servido pelo próprio backend
```

**c) Backend** (porta 8765):
```bash
cd backend
CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_LAN_BIND_IP=auto uv run python -m app.main
```
A porta 8765 é do `hangar-server` (Rust), que o backend Python sobe e deixa na frente; o Python
escuta atrás, numa porta de loopback. Sem o binário do `hangar-server` (o instalador baixa da
release `server-latest`), o Python atende a 8765 sozinho. A tela vem do `frontend/dist`; quem
mexe no front pode usar `npm --prefix frontend run dev` (Vite) no lugar do build.

No boot ele imprime um **QR** (URL + token) pra parear o celular. Variáveis (prefixo `CP_`,
ou em `backend/.env`):

| Var | Default | Pra quê |
|---|---|---|
| `CP_AUTH_TOKEN` | `change-me` | senha que protege TODA rota. Gere um forte. |
| `CP_LAN_BIND_IP` | `127.0.0.1` | `auto` = detecta o IP da LAN (pro celular alcançar). IP fixo também vale. |
| `CP_PORT` | `8765` | porta do backend |
| `CP_FRONT_PORT` | — | porta onde o PWA é servido (entra no QR). Vazio = o próprio backend, que serve o `frontend/dist` na raiz. Só quem mantém um `vite preview` separado grava `5173` aqui. |
| `CP_PUBLIC_URL` | — | sobrescreve a URL base do QR (ex: hostname Tailscale) |
| `CP_TERM_ORIGINS` | — | origens EXTRAS de onde o convidado com login pode abrir o terminal (csv). O dono não precisa: o acesso dele (`?token=`) abre o terminal de qualquer endereço. Vale quando o convidado usa um app servido de um host que não é este backend, nem a `CP_PUBLIC_URL`, nem um peer. |
| `CP_SCAN_ROOTS` | — | pastas que o seletor "Nova sessão" pode listar (csv) |
| `CP_TERMINAL` | — | emulador do botão **terminal nativo** (`wezterm`, `kitty`, `alacritty`, `konsole`, `gnome-terminal`, `xterm`). Vazio = procura nessa ordem no PATH. |

> Guarda de segurança: com `CP_AUTH_TOKEN=change-me` ele **recusa** subir num bind não-loopback.

## 3. Conectar o celular

### Opção A — LAN (mesma Wi-Fi)
1. `CP_LAN_BIND_IP=auto` no backend.
2. Escaneie o **QR** do terminal (ou abra `http://<ip-da-lan>:8765`).
3. URL + token preenchem sozinhos → conectado.

### Opção B — Tailscale (de qualquer lugar, com HTTPS)

VPN de volta pra sua rede — funciona em qualquer lugar (4G/outra Wi-Fi), sem expor nada à internet.

**1. Criar a conta:** vá em **https://tailscale.com** → *Get started* (ou **https://login.tailscale.com**)
e entre com Google/GitHub/Microsoft/e-mail. Cria seu **tailnet** (sua rede privada).

**2. Instalar nos dispositivos** (PC + celular, MESMA conta):
- PC (Linux): `curl -fsSL https://tailscale.com/install.sh | sh` → `sudo tailscale up`
- Celular: app **Tailscale** (App Store / Play Store) → login.
- Confira: `tailscale status` (os dois aparecem no tailnet).

**3. Habilitar HTTPS no tailnet** (necessário pro `tailscale serve` com HTTPS) — no
**admin console** (https://login.tailscale.com/admin), página **DNS**:
- Ative **MagicDNS**.
- Ative **HTTPS Certificates** (logo abaixo). Aceite que os nomes das máquinas + o nome
  DNS do tailnet vão pra um *ledger público* (Let's Encrypt). Cada máquina ganha um nome
  `<maquina>.<tailnet>.ts.net`.

**4. Expor o PWA** (rode no PC, na pasta do projeto):
```bash
tailscale serve --bg 8765      # publica o backend (tela + API) em https://<maquina>.<tailnet>.ts.net
tailscale serve status         # mostra a URL exata
```
**5. No celular** (com Tailscale ligado) abra `https://<maquina>.<tailnet>.ts.net` → cadeado
válido (Let's Encrypt) → escaneie o QR / preencha o token → **Adicionar à Tela de Início** (PWA).

> Fonte: [Tailscale — Set up HTTPS](https://tailscale.com/docs/how-to/set-up-https-certificates)
> · [tailscale serve](https://tailscale.com/docs/reference/tailscale-cli/serve). NÃO publique a
> 8765 com `tailscale funnel` (isso a expõe à internet pública). O único Funnel que o Hangar
> liga é o da porta do convidado, sozinho, enquanto houver convite ativo
> ([Compartilhar sessão](#compartilhar-sessão)).

> O app fala com o backend **cross-origin** quando preciso (multi-PC): ele aceita o token via
> header **e** via `?token=` (porque `EventSource`/`<img>` não mandam header). CORS já liberado
> (token-gated, sem cookies cross-site).

### Instalar como PWA (tela cheia)
- **iOS (Safari):** Compartilhar → **Adicionar à Tela de Início**. Abre standalone (sem barra do Safari).
- **Android (Chrome):** menu → **Instalar app**.

## 4. Operar o chat

### Idioma
- O app fala **português e inglês** e segue o idioma do sistema por padrão. Para trocar
  manualmente: **Configurações → Geral → Idioma · Language** (o app recarrega ao trocar).

### Sessões
- **Criar:** botão **＋ / Nova sessão** → escolha a pasta (cwd), o agente e como ele roda. Com
  terminal, o backend cria um tmux novo; sem terminal, cria um processo gerenciado pelo Hangar.
- **Modo padrão:** a última escolha do dono entre com e sem terminal fica no servidor. Novas
  aberturas sem modo explícito seguem essa preferência; `--terminal` e `--headless` prevalecem.
  Para Claude/Codex, sem terminal é o padrão inicial recomendado para usar só pelo Hangar.
  Convidados escolhem apenas para sua sessão, sem alterar a preferência do dono.
- **Sem terminal (Claude ou Codex):** na Nova sessão, em **Como rodar**, escolha **Sem terminal
  (processo do Hangar)**. Pelo terminal, use `hangar-send --new <nome> [cwd] --headless`; acrescente
  `--provider codex` para Codex. Combina com `--model`, `--effort` e `--permissao`. O agente roda
  fora do tmux: permissões e perguntas chegam direto no chat, e reiniciar o Hangar não corta o turno.
  Codex nasce em **Full Access**; use `--permissao` para escolher outro modo.
  - Não há painel de terminal nem espelho; `/btw` e os comandos que só existem na TUI
    (`/color`, `/doctor`, `/reload-plugins`) ficam fora.
  - O histórico continua no `.jsonl` do Claude ou no rollout do Codex e pode ser retomado depois.
  - Login e confiança na pasta precisam estar preparados para a conta escolhida; sem isso a sessão
    mostra no chat por que não conseguiu subir.
- **Numa worktree:** na Nova sessão, marque **Trabalhar numa cópia separada (worktree)** e escolha
  **+ Branch nova a partir de** uma base. O agente trabalha numa cópia da pasta, numa branch
  própria, e a sua pasta principal só muda quando você mesclar. A tela **Worktrees** lista cada
  uma (mesclada ou quantos commits à frente, o que não foi commitado) e apaga as mescladas e
  limpas; as conversas dela continuam e podem ser retomadas na pasta principal.
- **Worktrees de todos os computadores no app nativo:** a tela reúne os servidores próprios
  habilitados, separados por computador e repositório. Use **Todos os computadores** para filtrar
  uma máquina. Detalhes, criação, exclusão e abertura de sessão acontecem no computador da
  worktree. Se uma máquina não responder, as demais continuam visíveis e a falha permite tentar
  de novo. Para uma **Pasta sumida**, **Limpar registro** remove apenas aquela entrada do Git,
  conservando a branch, os commits e as conversas.
- **Limpeza em segundo plano no app nativo:** após confirmar a exclusão de uma worktree,
  a limpeza de um registro sem pasta ou um lote de mescladas, o diálogo fecha e a operação
  continua. A barra no topo, à esquerda da conta e da cota, mostra quantos itens foram
  processados. Clique nela para ver os detalhes; você pode trocar de conversa ou de servidor
  enquanto a limpeza continua no computador de origem. Ao concluir sem falhas, o indicador
  desaparece automaticamente. Falhas permanecem disponíveis até **Dispensar resultado**;
  **Abrir Conexão** permite conferir a credencial do computador de origem. Um item que falha
  não interrompe o restante do lote.
- **Preferências de worktrees no app nativo:** em **Configurações → Worktrees**, escolha a pasta
  das novas worktrees de cada servidor. Deixe vazia para usar a pasta de worktrees do Hangar;
  as existentes continuam onde estão. A limpeza automática começa desligada. É possível ligá-la
  por dias de inatividade, por integração na base ou por ausência de commits adicionais.
  A proteção de arquivos locais permite conservar alterações não commitadas, apenas arquivos
  versionados editados ou quaisquer arquivos locais, incluindo ignorados. Nas duas primeiras
  opções, arquivos ignorados como `.env` podem ser apagados. A limpeza automática conserva as
  branches e as conversas e não remove worktrees em uso ou bloqueadas.
- **Trocar:** toque no título (mobile) / clique na sidebar (desktop).
- **Renomear:** **toque longo** no nome (sidebar/desktop) → edita inline → Enter salva.
  Não quebra o histórico (resolve por `/proc`, não pelo nome).
- **Apagar:** × na linha (encerra o pane ou o processo sem terminal).

### Enviar
- **Texto:** digite e envie. **Multi-linha** funciona (Shift+Enter / colar — vai por bracketed paste).
- **Imagem / arquivo:** 📎 no campo de mensagem (upload) — ou cole no terminal do Claude que o app mostra o thumbnail.
- **Áudio (transcrição):** o botão de microfone grava (toque grava, outro toque para); ou anexe
  um arquivo de áudio. O texto aparece ao final, sem transcrição em tempo real. **Enviar encerra
  a gravação**, espera a transcrição e a organização do texto e envia uma mensagem com o texto
  digitado e os anexos. Isso vale para texto, imagem, arquivo ou apenas ditado, no desktop nativo
  e no PWA do celular. Se a transcrição falhar, o rascunho e os anexos continuam disponíveis.
  Configure em
  **Configurações → Voz → Transcrição**: a **Chave da transcrição** e, em **Usar outro serviço de
  transcrição**, endpoint e modelo. Endpoint e modelo vazios usam Groq e `whisper-large-v3`;
  a chave padrão também pode vir de `CP_GROQ_API_KEY`/`GROQ_API_KEY` no ambiente do backend. Para
  ter reserva, monte **Serviços de transcrição, em ordem** (compatível com OpenAI, ElevenLabs ou whisper.cpp local):
  o primeiro transcreve e, se falhar ou ficar sem cota, o próximo assume — o aviso do ditado diz
  qual transcreveu. Com a lista em uso, a transcrição usa só ela. Sem chave nem lista, a gravação
  funciona, mas a transcrição responde 503. Em **Compatível com OpenAI**, informe URL base e
  modelo; a chave é opcional para serviços que dispensam autenticação. Para OpenRouter, use
  `https://openrouter.ai/api/v1`, sua chave e um modelo de transcrição do catálogo dele.
  **Testar com áudio** usa a configuração salva e mostra o texto ou o erro; o serviço pode cobrar
  essa chamada.
- **Whisper local:** instale o [whisper.cpp](https://github.com/ggml-org/whisper.cpp) e obtenha um
  modelo pelos canais oficiais. No serviço local do Hangar, informe os caminhos do executável
  `whisper-server` e do modelo, o idioma e, se necessário, o caminho do FFmpeg. Esses arquivos
  ficam na máquina do **servidor selecionado**, inclusive ao configurar pelo celular. O Hangar
  inicia o processo em loopback, reutiliza-o e o encerra junto com o servidor; não baixa nem
  instala programas ou modelos. WAV PCM mono de 16 kHz passa diretamente; WebM/Opus, M4A/AAC e
  outros formatos usam o FFmpeg configurado ou encontrado no PATH. Falta de arquivo ou conversor
  aparece como erro. Só há reserva externa se você a acrescentar à lista. A transcrição requer
  o `hangar-server` Rust; o modo Python não contém um segundo motor.
- **Organização do ditado no desktop nativo:** em **Configurações → Voz → Organização do texto**,
  escolha **Sem organização** (padrão), **Harness da conversa** ou **API externa**. Sem organização
  entrega o texto da transcrição e dispensa um serviço de organização. No harness, escolha o modelo
  disponível na conta da conversa aberta; Claude e Codex têm preferências separadas. A chamada é
  independente da conversa principal; o Hangar fornece transcrição e prompt de estilo. Claude usa
  a autenticação original da conta de destino, inclusive a assinatura por login OAuth, e seu SDK
  acrescenta metadados de ambiente e identificação da conta. O histórico e as instruções globais
  ou do projeto ficam fora da chamada.
  Com organização ligada, **Só limpar**, **Reorganizar** e **Briefing** mantêm as travas contra
  invenção e perda de conteúdo. Frase com menos de cinco palavras ou começando com `/` é preservada.
  Falha de serviço, modelo indisponível ou saída recusada conserva o cru com aviso, sem recorrer a
  outra conta. Os botões de versão permitem recuperar **Cru** e mudar o estilo sem repetir o STT.
  Áudio anexado como arquivo continua sem organização. No celular (PWA e app), a escolha de estilo
  só aparece com a organização ligada no servidor; desligada, o campo avisa que o texto vai cru.
- **Referência de grafia opcional:** com organização ligada, **Usar as três últimas mensagens**
  envia somente texto de usuário e assistente da conversa de destino, em ordem, limitado a 2.000
  caracteres por mensagem. Nasce desligado. Serve para conferir nomes e termos, sem completar a
  fala com conteúdo do diálogo; ferramentas e pensamento ficam fora. Ao trocar o estilo, o app
  reutiliza a referência capturada na primeira organização.
- **Voz ao vivo (Beta):** uma conversa falada com um assistente que lê e acompanha as sessões,
  troca de sessão pela fala, manda pedidos a elas e, no modo **Planejar**, monta um plano antes de
  mandar. Aparece sozinha em todo servidor com o `codex` instalado e ao menos uma conta Codex; não
  há nada para ativar. O microfone só abre quando você liga a conversa, e ela gasta a conta Codex
  desse servidor.
  - **No PC (app nativo):** a pílula da voz fica na barra de cima. Ela atende também os pedidos de
    tela (trocar de sessão, ler a tela, clicar, abrir telas do Hangar).
  - **No celular (PWA):** o botão de microfone **Voz ao vivo** aparece no topo da lista de sessões e
    da conversa quando o servidor ativo tem a voz disponível. Um toque começa a conversa nesse servidor;
    com ela aberta, outro toque abre o painel (silenciar, modo **Direto**/**Planejar**, ajustes,
    **Servidor da voz**, **Encerrar**). No celular a voz só troca de sessão; ler a tela e clicar
    ficam para o PC.
  - **O que mais ela faz pela fala:** reabre a conversa que você fechou sem querer ("reabre a
    auditoria-migracao", "reabre a última que eu fechei"), que volta com o nome que tinha e abre na
    tela; diz o que você fechou e acha conversa pelo assunto; lista as pastas onde dá para abrir sessão
    e abre sessão nova com o agente, modelo, conta ou modo que você disser; roda os atalhos do Hangar
    ("abre a VM"); diz quais máquinas estão respondendo; interrompe uma sessão. Explica o que uma
    sessão fez a partir do seu pedido a ela ("o que a web fez com o que eu pedi?"), mesmo fora da tela
    e mesmo depois de muitas ferramentas, e volta a pedidos anteriores se você quiser. Usa também o
    resto do Hangar (qualquer rota da API, em qualquer máquina, e as ferramentas do MCP `hangar`:
    recado, sessão nova, grupo, navegador da sessão, página na conversa), sem nunca ver o token. Fechar
    e interromper sessão, apagar, parar ou publicar pela API, atalho marcado para confirmar, atalho de
    comando do repositório e comando que apaga ou publica algo sempre esperam o seu "sim" falado. Ela usa os botões
    da tela só quando não há outro jeito.
  - **Ajustes** (conta Codex, voz, modelo, esforço e velocidade de cada modo) ficam salvos no
    servidor e valem para qualquer aparelho.
  - **Uma conversa por servidor.** Abrir a voz em outro aparelho passa a conversa para ele; o
    anterior mostra "A conversa passou para outro aparelho." Se o aparelho sair (app fechado, rede
    caiu), reabrir em até 2 minutos retoma a mesma conversa do assistente; depois disso ela termina.
  - **Peça silêncio até voltar.** "Vou sair uns minutos, não me avisa nada até eu voltar": as respostas
    das sessões ficam guardadas, nada é falado e o trabalho continua. O silêncio não acaba sozinho; quando
    você disser "voltei" ou "me dá o resumo", ela resume o que chegou, uma resposta por vez.
  - **O que a voz lembra.** Ela guarda poucos fatos entre uma conversa e outra (quem você é, como
    gosta de ser respondido, onde ficam seus projetos) e começa cada conversa já sabendo disso. Na
    primeira conversa, logo ao conectar, ela se apresenta e pede licença para ler, só leitura, seu
    `~/.claude/CLAUDE.md` e seus `~/.codex/AGENTS*.md`, e pergunta o que você faz e como prefere as
    respostas. Depois de ler, ela conta o que entendeu ("Entendi que você … — confere?") e pergunta
    como você quer usar a voz (acompanhar sessões, mandar pedidos, planejar); só guarda depois que você
    confirmar ou corrigir, e nunca senha, token ou IP. Em seguida pede outra licença para
    olhar só os nomes das suas pastas principais (o primeiro nível de `~/Projetos`, por exemplo) e
    propõe poucas referências, como "`~/Projetos/hangar` parece ser o Hangar — confere?", guardadas só
    depois do seu sim; não lê código nem arquivos ocultos. Se você recusar algum dos dois, ela guarda isso e não pergunta de novo. Essa lista é a
    memória da voz; a memória do próprio Codex não entra nela. Só grava ou apaga num turno em que você falou. O que você
    pede vale desde já e manda no jeito padrão dela (por exemplo, "ao entrar numa sessão, me resume o que ela fez"
    faz a troca de sessão resumir sem perguntar), menos nas confirmações de segurança. Cada memória tem até 240
    caracteres; se passar, ela avisa que cortou. Para ver ou apagar,
    abra os ajustes da voz e procure **O que a voz lembra**; com a lista aberta durante a conversa, o que a voz grava
    ou apaga aparece na hora. A lista fica no arquivo
    `~/.hangar/voz/memoria.json`, no servidor, mas o texto dela segue para o provedor do assistente
    (a OpenAI, pelo Codex) a cada conversa. Na primeira conversa, o conteúdo dos arquivos que ela
    lê (`~/.claude/CLAUDE.md`, os `AGENTS*.md`, os nomes de pasta) também vai para esse provedor.
  - O áudio vai direto do aparelho ao serviço de voz da OpenAI; o servidor só conduz a conversa. Só o dono usa a voz:
    num servidor aberto por convite o botão não aparece. O app Expo ainda não tem a voz.
- **Ditado mãos-livres:** chave **Enviar transcrição automaticamente** em Configurações → Voz,
  guardada **só neste aparelho** (não vai
  pro servidor; se você ligar no celular, o desktop continua sem). Com ela ligada, um toque no 🎤
  começa a gravar e **2 segundos de silêncio** encerram sozinhos — não precisa tocar em ⏹. Depois da
  transcrição, uma contagem de **3 segundos** aparece antes do envio; um toque em qualquer lugar da
  tela ou esconder o app (trocar de app, apagar a tela) cancela e deixa o texto pronto no campo, sem
  mandar. Só o silêncio dispara o envio automático: parar pelo botão, estourar o teto de 3 minutos de
  gravação (transcreve normal e avisa que não identificou silêncio — provável barulho de fundo —, mas
  não manda sozinho), dar erro na transcrição, a limpeza falhar (fica o aviso e o texto cru) ou já
  haver um rascunho digitado no campo — nenhum desses envia sozinho, sempre fica o texto pra você
  revisar e mandar na mão. Depois de um envio automático
  o microfone **não** volta a gravar sozinho — pra falar de novo, toque nele; foi decisão deliberada
  pra não ficar transcrevendo (e cobrando) cada silêncio do carro à toa. Também toca um som curto ao
  enviar e outro mais grave quando não deu pra enviar, pra dar pra saber sem olhar pra tela.
- **Se estiver ouvindo uma resposta em voz** e você tocar o 🎤, a leitura para sozinha antes da
  gravação começar — sem isso o microfone captaria a própria voz do app.
- **API externa para organizar:** selecione esse modo e preencha **Endpoint da organização**,
  **Chave da organização** e **Modelo da organização**. Endpoint e modelo vazios usam o serviço
  padrão compatível com OpenAI; a chave de organização é própria, inclusive no padrão. O Briefing
  pode ter endpoint, chave e modelo próprios. Configuração antiga de LLM não liga a organização
  automaticamente. A leitura em voz continua sendo uma capacidade configurada separadamente.
- **Furar a fila (só Kimi):** mensagem mandada com a sessão trabalhando fica na fila **do Kimi** —
  ele processa quando o turno atual acabar. Enquanto houver fila, a fileira de cima do campo de mensagem
  mostra **⏳ N na fila · mandar agora**; tocar manda o `ctrl-s` do Kimi e a fila **inteira** entra no
  turno em curso, sem esperar. Não tocar = espera, como sempre.
- **Painel "Na fila" (Claude e Codex):** mensagem mandada com a sessão trabalhando espera no
  Hangar, num painel acima do campo de mensagem (app nativo, PWA no celular e app), na ordem de entrega; ela
  sai sozinha quando o agente para. Em Claude e Codex, com ou sem terminal, as mensagens comuns
  que já esperam são reunidas num único prompt, com uma quebra de linha entre elas. Mensagens
  que chegarem depois da reserva esperam o próximo turno. Recados de sessões, comandos e respostas
  a perguntas mantêm seus canais próprios. O lote vira uma única bolha na conversa. Em cada linha:
  **Apagar** tira a mensagem da fila (em todos os aparelhos abertos); **Editar** leva o texto ao campo
  de mensagem, onde o envio vira **Atualizar** (Esc ou Cancelar sai sem mudar; anexo e microfone ficam
  desligados enquanto edita); a alça (ou as setas) **reordena**; **Orientar** manda aquela mensagem
  agora, no meio do turno. No app as ações ficam no toque longo da linha. Se a mensagem sair da
  fila enquanto você a edita, o campo de mensagem volta ao rascunho e o app avisa uma vez. Pi, omp, Kimi
  mostram o painel só para leitura.
- **Orientar no Codex:** durante um turno, o botão de envio habitual coloca a mensagem no painel
  "Na fila". **Orientar** (no campo de mensagem) envia o texto ao turno em andamento; para uma mensagem já
  na fila, use o **Orientar** da linha dela. Se o turno terminar antes da entrega, o texto é
  preservado e o app informa a falha; a orientação não inicia outro turno por conta própria.
- **Slash commands:** `/` abre a lista (`/clear`, `/compact`, …). `/clear` limpa de verdade (zera a
  fila). No Claude com terminal, com mensagens esperando no painel, comando de barra é recusado com
  aviso: oriente ou apague as mensagens antes, para o comando não passar na frente delas nem apagá-las.
- **Modelo/esforço:** toque na pill (ex `Opus4.8·1M·high`) → escolhe modelo + esforço (só na sessão).
- **Codex — modo e skills:** o botão **Normal/Planejar**, ou **Shift+Tab** no campo de mensagem,
  muda o modo nativo da sessão. A troca preserva modelo, esforço e permissões e vale para o próximo
  turno. Modelo e esforço acompanham alterações feitas no terminal. Digitar `/` lista as skills
  habilitadas no próprio Codex; selecionar preenche `/nome ` para você acrescentar os argumentos.
- **Claude — modo:** o seletor mostra o modo atual. **Shift+Tab** no campo de mensagem ou
  **Alt+Shift+P** percorre todos os modos disponíveis naquela sessão, na ordem do terminal.
  Modos indisponíveis não entram no ciclo. O seletor fica na linha inferior; quando falta espaço,
  ocupa a primeira linha existente, tanto no Claude quanto no Codex.
- **Citar trecho da resposta:** selecione um trecho de uma resposta já terminada do agente e
  toque em **Citar** (no app nativo, o botão aparece junto da seleção e também no menu de contexto
  da mensagem; no PWA do celular, rente ao campo de mensagem; no app do celular, no menu da
  seleção). Abre **Responder ao trecho**, com o trecho e um campo para a sua resposta ou
  comentário. **Comentar e continuar** (Enter) guarda a citação acima do campo de mensagem e deixa
  a conversa onde estava, para citar o próximo trecho; **Finalizar** (Ctrl+Enter) guarda, leva ao
  campo de mensagem e rola até o fim, sem enviar. Serve para responder item por item uma resposta
  longa. O comentário continua editável na bandeja, e o X tira a citação. Ao enviar, cada trecho
  vai com o seu comentário, marcado para o agente como referência e não como pedido novo; o que
  você digita no campo é a instrução em volta. Na conversa a citação aparece em destaque, com o
  comentário fora dela, e tocar nela volta ao trecho de origem (se a origem sumiu, a citação
  continua legível). Até 8000 caracteres por trecho.
- **Pergunta interativa do Claude** (AskUserQuestion/permissão): as opções viram **botões** —
  toque. (Se não renderizar como botão, responda com o **número** em texto.)

### Acompanhar
- **Streaming ao vivo:** enquanto o Claude escreve, aparece um **preview** da prosa (box contido,
  marcado com hairline). Vira a mensagem final (markdown limpo: tabelas, listas, código) quando fecha.
- **Estado:** spinner com o label do Claude (`Forging…`), firme (com debounce anti-flicker).
- **Diagramas Mermaid:** um bloco ` ```mermaid ` de resposta terminada aparece desenhado, no tema
  do app (no app nativo, no PWA do celular e no app do celular). Enquanto a resposta chega, fica como
  código. Diagrama com erro mostra a mensagem do Mermaid e o código. Tocar abre em tela cheia com
  zoom; copiar a mensagem e a busca continuam usando o texto do diagrama. O desenho é feito pelo
  servidor do Hangar, com o Chromium que ele já usa para as páginas da conversa e sem acesso à rede;
  sem Chromium no servidor, o diagrama fica como código com o aviso e **Tentar de novo**.
- **Atividade / Workflows:** ícone de atividade no topo (pulsa quando há workflow/agente rodando) →
  abre o painel: tarefas + workflows → fases/agentes → prompt+resultado de cada agente (3 níveis).
- **Interromper:** botão **⏹ stop** (manda `Esc`).
- **Pele Terminal** (no nativo, **Aparência → Chamadas de ferramenta → Terminal**; no web, a pele
  Terminal): fica parecida com o Claude Code. O texto do agente abre com uma bolinha e cada chamada
  com a bolinha do estado dela (cor de destaque rodando, verde concluída, vermelha com erro); edição
  aparece como **Update** e ferramenta de MCP como `servidor - ferramenta (MCP)`, e as chamadas dos
  servidores criados por plugin ficam à vista, fora da dobra. Enquanto o modelo pensa, a linha
  mostra o cérebro e **Pensando** com um brilho que percorre o texto (parado com movimento
  reduzido); o raciocínio terminado também abre com o cérebro, em todas as peles.
- **Página em tela cheia:** a página HTML que o agente mostra na conversa tem o botão de tela cheia
  (no nativo e na PWA do celular; no app, abre numa tela própria). O diálogo tem zoom de 50% a 200%
  pelos botões, **Ctrl+roda** ou **Ctrl+=/-/0** (no app, pinça); a página continua viva e
  clicável no zoom escolhido. **Esc** fecha.

### Mods do Claude Code (faixa e painéis)
Sessões com terminal espelham o que os mods do Claude Code desenham, sem código de mod nenhum
no Hangar:
- **Faixa acima do prompt** (barra de progresso, acompanhamento de review…): aparece acima do campo
  de mensagem e se atualiza na hora, mesmo com a sessão parada.
- **Painel que um mod abre:** no app de desktop, coluna à direita da conversa quando o terminal o
  ancora e há espaço para os dois; senão, e sempre no celular, bloco acima do campo de mensagem, com o ✕
  que fecha o painel no terminal também.
- **Botões dos mods clicam pelo app:** o clique vira clique de mouse no terminal da sessão, e o
  Hangar abre as sessões Claude em tela cheia, o modo em que o Claude Code liga o mouse. Se o botão
  não estiver na tela, aparecer duas vezes ou o terminal estiver em modo de rolagem, o app avisa e
  não clica.
- **Copiar e abrir link** num botão clicado pelo app acontecem no aparelho de quem clicou, não na
  máquina do terminal: o texto vai para a área de transferência dele e o link abre no navegador
  dele. Clique feito no próprio terminal segue como sempre.

### Multi-PC
Cada PC tem o próprio Hangar e o **próprio** token. Em **Configurações → Servidores → Adicionar
servidor**, informe o endereço (ou cole o link de pareamento do QR) e o token dela; **Buscar no
Tailscale** acha as máquinas da tailnet que respondem. Escolha se a máquina entra na lista deste
aparelho (**Mostrar as sessões dele**) e/ou se as sessões das duas trocam recados: isso grava
endereço e token de uma no `backend/peers.json` da outra, e cada uma precisa de `CP_SERVER_ID`.
As sessões de todas aparecem numa lista só.

### Compartilhar sessão

Manda UMA sessão para outra pessoa que também usa o Hangar. Ela vê a sessão na lista dela, dentro de
um servidor "Convite · <você>", e pode tudo ali: conversar, trocar modelo e permissão, terminal.

- **Quem entra pode tudo nesta máquina, como você.** A sessão roda comandos sem pedir permissão e o
  terminal alcança suas outras sessões. Compartilhe só com quem você confia.
- **Pré-requisitos, uma vez só:** `sudo tailscale set --operator=$USER` (só no Linux; o `install.sh`
  já roda) e o Funnel liberado na política da tailnet. Quando falta algo, o diálogo mostra dois botões:
  **"Autorizar"** (app nativo no Linux, com o servidor desta máquina) pede a sua senha e libera o
  operador; no web o comando aparece com "Copiar". **"Liberar no Tailscale"** abre a página do
  Tailscale que libera o Funnel e fica conferindo por 5 min: liberou, o aviso some e o "Gerar link de
  convite" volta.
- **Compartilhar:** menu da sessão → "Compartilhar sessão" → "Gerar link de convite". Enquanto o
  servidor confere que o link já abre pela internet (até ~3 min), aparece "Preparando o acesso de
  fora…"; se não abrir, o diálogo diz em que etapa parou (DNS ou Funnel) e você gera de novo. O link
  `https://<máquina>.ts.net:8443/convite/<código>` vale 24 h e serve uma vez só. Mande pelo WhatsApp.
  Compartilhar precisa do servidor Rust do Hangar rodando na máquina: sem ele a porta do convite
  fica fechada e o diálogo avisa.
  Na mesma rede, sem Tailscale: "Gerar link local" dá `http://<ip-da-rede>:8766/convite/<código>`,
  que só abre de quem está naquela rede (a máquina precisa de `CP_LAN_BIND_IP=0.0.0.0`). No PWA
  aberto por `https` o navegador bloqueia o link `http`; use o app nativo ou o desktop.
- **Receber:** no app nativo, clique no link (ou "Entrar em sessão compartilhada" na página
  Servidores e cole). No web/PWA, Configurações → Servidores → "Colar convite". No Linux, abrir o app pelo `hangar://`
  depende da versão nativa nova (o `install-linux.sh` do pacote registra o esquema); no Windows o
  registro vem do `.ps1` do repositório.
- **Encerrar:** "Revogar" num aparelho, "Encerrar todos", ou fechar a sessão. Conexões abertas caem
  em cerca de 5 s. Sem convite ativo o Funnel da porta 8443 desliga sozinho.
- **O que o convidado não faz:** ele não conta como o seu app aberto (você segue recebendo as
  notificações) e a lista dele não mostra o nome das suas outras sessões. Servidor de convite que
  responde "encerrado" (ou 401) vira "compartilhamento encerrado"; erro 503, 502 ou conexão recusada
  é passageiro (você trocando o modo da sessão, tmux ou túnel fora, o servidor do Hangar
  reiniciando) e ele tenta de novo, o código não foi gasto.

### Parear com a sessão de outra pessoa

Junta uma sessão sua com a de outra pessoa que usa o Hangar: as duas trocam recados e cada um vê a
sessão do outro dentro do próprio grupo. Usa os mesmos pré-requisitos do compartilhamento.

- **Convidar:** menu da sessão → "Compartilhar sessão" → "Convidar para parear" (no app do
  celular, "Convidar para parear" direto no menu) → escolha o acesso que a outra pessoa terá:
  **"Ver e mandar recado"** (lê a conversa e manda recados) ou **"Acesso completo"** (escreve nas
  sessões do grupo, inclusive o terminal desta máquina). O link
  `…ts.net:8443/par/…` só aparece depois de conferido pela internet, como no compartilhamento.
- **Aceitar:** "Parear por convite…" no menu da sessão (ou em Máquinas, no PWA e no app; no nativo,
  basta clicar no link), cole o link, escolha a sua sessão e o acesso que VOCÊ dá. O aviso diz o que a outra pessoa te
  deu. Nos dois lados, a sessão de fora aparece dentro do grupo, marcada "de <dono>", com o
  cabeçalho `sua-sessão ↔ apelido::sessão (dono)`.
- **O nível vale para o grupo inteiro.** Quem você puser no grupo pela tela, com par em acesso
  completo, pede confirmação e já entra liberado. Quem entrar por agente (`hangar-send --pair`,
  MCP) fica com "ver e mandar recado" e a linha mostra "Acesso completo pendente" com o botão
  "Liberar para <dono>". Grupo de orquestração não aceita sessão de fora.
- **Sair:** na sessão de fora, "Sair do pareamento com <dono>" (a sessão dela continua rodando;
  vocês param de se ver). Na sua sessão, "Desfazer par com <dono>". Tirar do grupo a sessão que
  fez o par, ou fechá-la, desfaz o par para o grupo inteiro; tirar outra só corta o acesso a ela.
- Do outro lado com Hangar antigo, o par funciona com "ver e mandar recado" dos dois lados.

### Opções do Claude Code

Em **Configurações → Harnesses → Claude Code → Opções**, a preferência **Atualizar barra de
status** permite ao instalador do Hangar configurar a barra do Claude Code. Ela vem ligada por
padrão. Para manter uma barra personalizada, desligue a opção e clique em **Salvar**.

A escolha fica no servidor selecionado e vale nas instalações e atualizações seguintes, em
Linux e Windows, mesmo com o backend parado. Salvar a preferência não troca nem restaura a barra
atual; desligá-la preserva o comando que já está configurado. No desktop, as opções abrem em
modal; no celular, em uma folha.

### Contexto estendido do Codex

Em **Configurações → Harnesses → Codex → Opções**, habilite **Contexto estendido (até 1M)** e
clique em **Salvar**. A opção usa `model_context_window` na configuração oficial do Codex e vale
para novas sessões. Desativar restaura o valor anterior; o limite personalizado de compactação,
quando existir, continua valendo e aparece na tela.

O Codex limita a janela ao máximo permitido pelo modelo. O painel mostra os limites anunciados
pelo catálogo local; em 07/09/2026, o Codex 0.153.4 desta instalação anunciava 872.000 tokens para
Astra e Sol, embora a documentação dos modelos informe contexto de até 1.050.000 tokens. A opção
não altera o catálogo nem promete uma janela maior que a aceita pelo CLI.

Nessa versão, janela utilizável e compactação são valores distintos: a janela utilizável é 95%
da janela configurada e o gatilho padrão de compactação é 90%. Assim, o padrão de 272.000 vira
258.400 utilizáveis e compacta a partir de 244.800; com 872.000, são 828.400 utilizáveis e gatilho
de 784.800. Um limite personalizado de compactação menor continua antecipando esse gatilho.

### Contas Codex (ChatGPT)

Para cadastrar uma conta de assinatura, abra **Configurações → Servidor → Contas e provedores →
+ Nova conta → Conta por assinatura → Conta do ChatGPT (Codex)**. Dê um nome à conta e conclua o
login OAuth nativo por código de dispositivo: o Hangar mostra um endereço HTTPS e um código, e só
marca o login como concluído depois da confirmação do Codex. **Chave de API** segue o caminho
separado de cadastro de chave para provedores; ela não é login OAuth da assinatura ChatGPT.

A conta padrão usa o `CODEX_HOME` atual. Cada conta adicional recebe seu próprio `CODEX_HOME` e,
ao ser preparada, herda seletivamente preferências, referências de agentes/skills/hooks e plugins
da conta padrão. A autenticação, o histórico, as sessões, os rollouts, os bancos locais, caches e
o estado de confiança ficam no diretório da conta escolhida. Uma preparação parcial ou com erro
mostra as pendências; o Hangar não aprova hooks automaticamente. Se aparecer o aviso de confiança,
confirme os itens no próprio Codex antes de usar a conta.

Os plugins continuam instalados por conta. O Hangar não liga o cache da secundária ao da padrão por
symlink: o Codex exige que o catálogo embutido pertença ao `CODEX_HOME` atual, e compartilhar o cache
permitiria que uma atualização numa conta alterasse o código executado pela outra.

Ao criar uma sessão pelo Hangar, escolha **Codex** e a conta no campo **Conta do Codex**. A conta
padrão vem marcada quando está disponível, mas a seleção é explícita e acompanha a criação até o
processo do Codex. O formulário bloqueia uma conta sem login confirmado ou sem preparação pronta.
OAuth e chave de API aparecem apenas como método de autenticação; a origem da conversa é a conta
Codex selecionada.

Em **Contas e provedores**, cada janela de cota mostra quando reinicia. Quando o Codex informar
redefinições guardadas, a conta mostra quantidade e expiração. O botão só libera quando a janela
semanal chegar a 100%; antes de gastar, a tela confirma que as janelas elegíveis serão restauradas
e que a data do reset semanal mudará.

No **Arquivo**, a retomada usa a conta registrada nos metadados da conversa e não troca de conta
por cota ou por conveniência. Se a origem ficar ambígua, a retomada é recusada até haver uma única
conta identificada. Esse fluxo não apaga, rotaciona nem migra credenciais ou conversas entre contas.

### Continuar uma sessão Claude no Codex (em validação)

O código já está na versão principal, mas o recurso segue em validação: ainda não foi aceito no
uso completo (ver a prova abaixo). A seleção existe só no desktop nativo; estes passos descrevem
esse fluxo, sem seletor no PWA ou no app móvel.

1. Abra o **anel de contas** da sessão Claude e escolha uma conta **Codex**.
2. Confira **conta, modelo e esforço**, nos mesmos controles da Nova sessão. **Padrão** deixa o
   Codex resolver a configuração da conta. Mudar de conta recarrega seu catálogo; escolhas
   incompatíveis deixam de valer. Carregamento, falta de login, catálogo vazio e erro impedem a
   confirmação.
3. Confirme a transferência. Fechar ou cancelar o diálogo antes de confirmar mantém o Claude.
   A transferência exige a sessão parada, sem ferramenta, pergunta, permissão ou entrega
   pendente. Não inicia um turno nem pede ao Claude que resuma a conversa.

A troca conserva nome, cartão, chave, pasta, modo com/sem terminal, pareamento, grupo e ação
encadeada. O contexto disponível do ramo Claude entra na thread Codex pela API nativa, sem
resumo substituto nem cortes para caber. No Hangar, a conversa anterior vem da origem
preservada e os turnos seguintes vêm do Codex. A TUI Codex mostra somente os turnos novos.
As próximas ferramentas são as do Codex; `Read`, `Edit` e `Bash` antigos continuam como
histórico e não são executados novamente. Só Claude → Codex está incluído.

Capacidade desconhecida, contexto acima da estimativa permitida, mídia incompatível ou
conteúdo necessário que não possa ser preservado causam recusa explícita. A estimativa usa
bytes UTF-8 e reservas para instruções, ferramentas, imagens e continuação; não é uma
contagem pelo tokenizer e pode recusar uma conversa que caberia. Texto de pensamento
disponível é guardado como histórico identificado, sem importar a assinatura como raciocínio
Codex. Conteúdo redigido, criptografado ou já cortado na origem não pode ser recuperado.

Se a preparação falhar depois de parar o Claude, o backend tenta restaurá-lo pelo transcript
original. Se não puder confirmar a restauração ou a saída dos processos preparados, mantém o
erro e o histórico da origem, com **Recarregar** para tentar a recuperação. Uma queda dura
sem registro suficiente dos processos pode continuar nesse estado; a recuperação automática
não é garantida. O terminal direto não obedece à trava de entrada do backend: nesta
implementação, o estado é conferido novamente logo antes de parar a origem.

A prova disponível usou Codex CLI 0.159.3 em stdio com fonte artificial e resposta simulada em
loopback: conferiu bytes importados após reinício e recusa de contexto maior, sem inferência
ou custo real. Modelo real, fonte real, interface/foco, guardas com processos reais, modo Plano,
permissões, WebSocket/TUI, Arquivo/recarga completos e Windows permanecem pendentes. Testes
automatizados não rodaram. A [medição](decisoes/harnesses.md#transferência-claude--codex-captura-nativa-em-validação)
detalha o alcance dessa prova.

### App de desktop (nativo)

No computador, o Hangar é o **app nativo** (`desktop-native/`, em Rust, desenhado na GPU, sem
navegador por baixo). O instalador já o baixa; para baixar à mão, use a
[release `native-latest`](https://github.com/jeffer1312/hangar/releases/tag/native-latest):

| Plataforma | Arquivo |
|---|---|
| Windows x64 | `Hangar-windows-x86_64.zip` (contém o `Hangar.exe`) |
| Linux x86_64 | `Hangar-linux-x86_64.tar.gz`, `.deb` ou `.rpm` |
| macOS Apple Silicon | `Hangar-macos-aarch64.zip` (instale o servidor pelo terminal antes) |

- **Sem assinatura digital.** No Windows o SmartScreen avisa na primeira vez: **Mais informações →
  Executar assim mesmo**. No macOS, clique com o botão direito → **Abrir** na primeira vez.
- **Instala o servidor se faltar (Linux e Windows).** Aberto numa máquina onde não acha o Hangar,
  o app oferece um assistente que instala o servidor em poucas telas, sem terminal; se achar, só
  conecta.
- **Atualiza sozinho.** Quando há versão nova do app, a barra de cima oferece **Atualizar**: ele
  baixa, confere o sha256 da release e reinicia; se a versão nova não abrir, a anterior volta.

**Bandeja (Linux e Windows):** **Configurações → Geral → Manter na bandeja ao fechar** põe um
ícone do Hangar na bandeja do sistema. Ligada, fechar a janela esconde o app em vez de encerrar:
ele continua aberto e avisando. Clique no ícone para mostrar ou esconder a janela; o menu dele
tem **Abrir Hangar** e **Sair**. Sem bandeja no sistema, fechar encerra como sempre.

**Vista de desktop do navegador (congelada):** abrindo a URL do Hangar num navegador largo
(≥820px), ainda aparece o shell de duas colunas (sidebar + chat), com board e canvas. Ele continua
funcionando, mas não ganha recurso novo: o desktop evolui no app nativo e a web, no celular (PWA).
O Electron antigo saiu do instalador. Os ajustes de **Aparência** dessa vista:

- **Barra lateral aberta** — mantém a lista aberta o tempo todo. Desligada (padrão), ela fica no
  trilho de iniciais e só abre enquanto o mouse está por cima.
- **Altura da barra** — aparece quando a de cima está ligada: **altura total** (de ponta a ponta) ou
  **só o conteúdo** (a barra encolhe até onde as sessões terminam e fica flutuando, centralizada).

### Busca e custos

- **Buscar em todas as conversas:** no app nativo, **Ctrl+K** (⌘K no macOS) busca por sessões e
  pelo texto das conversas, vivas e fechadas, em todas as suas máquinas (convites ficam de fora);
  **Retomar a conversa** abre uma sessão nova com ela. No PWA, a busca fica no seletor de sessões.
- **Custos e uso:** a tela **Custos** (no nativo também **Ctrl+Alt+C**) mostra tokens e custo
  estimado por dia, por conta/provedor, por CLI e por projeto. É estimativa com tarifa de API,
  não a fatura do Claude ou do ChatGPT. A cota de cada conta fica em **Contas e provedores**.

### Terminal de verdade

O ícone de terminal no topo do chat abre um **terminal de verdade** — não é uma foto da tela: é a
sessão tmux anexada, com cor, seleção de texto e teclado completo. No desktop ele fica no rodapé:
arraste o canto pra mudar a altura, ou use **⤢** pra maximizar; o **✕** fecha. No celular é o
mesmo terminal (xterm) em tela cheia, com tamanho de fonte, campo de texto e uma barra de teclas.
No Windows ele roda sobre o ConPTY.

- **Um painel por sessão.** Abrir o painel da mesma sessão noutra aba/navegador derruba o primeiro,
  que mostra **desconectado · reconectar** em vez de congelar calado. Enquanto o painel está aberto,
  responder opção/pergunta **pelo chat** é recusado com um aviso na tela ("Terminal aberto nesta
  sessão. Feche o painel pra responder por aqui") — com o terminal anexado a janela do tmux fica no
  tamanho dele, e quem conta linhas na tela escolheria a opção errada. Feche o painel e responda.
- **Aba `+` (shell).** Ao lado da aba do agente, abre um **shell separado** no mesmo diretório da
  sessão — pra rodar `git`, `ls`, o que for, sem atrapalhar o agente. Ele é uma sessão tmux
  escondida (`term-<nome>`): não vira card na lista, no board nem no canvas. Encerrar a sessão do
  agente pelo app encerra o shell junto; **renomear** a sessão renomeia o shell junto — o que estiver
  rodando nele (um `npm run dev`) continua rodando, no mesmo diretório. Só se o nome de destino já
  estiver ocupado por um shell antigo é que o seu é encerrado, pra não virar sessão órfã.
- **Terminal nativo.** O botão de janela abre a MESMA sessão tmux numa janela de verdade do seu
  sistema (`tmux attach`), e fecha o painel embutido — os dois anexados brigariam pelo tamanho.
  Fechar o painel não desanexa essa janela. **Reiniciar o serviço do backend, porém, só é inofensivo
  quando o `systemd-run --user --scope` funciona nesta máquina** — é ele que põe o emulador num cgroup
  próprio. Quando o systemd do usuário recusa criar scope transiente (acontece; o backend loga
  `systemd-run --user --scope indisponivel` ao criar a primeira sessão), a janela nasce no cgroup do serviço e
  um `systemctl --user restart` a derruba junto. Sem emulador conhecido no PATH ele diz isso; pra
  escolher qual usar, `CP_TERMINAL` (tabela da seção 2).

### Navegador embutido

No app nativo, a entrada **Navegador** do painel lateral abre um **navegador de verdade dentro do
app**, um por sessão: no Windows é o WebView2; no Linux, um Chromium sem janela pintado no painel
(precisa do Google Chrome ou do Chromium instalado). Digite o endereço na barra e a página abre
ali: serve pra ver o `localhost:3000` do projeto sem sair do Hangar. O agente daquela sessão dirige
o MESMO navegador pelo `hangar-preview` (`open`, clicar, preencher, tirar print) — quando ele abre
uma página, o painel aparece sozinho na sua tela. No app nativo não há abas: é um navegador por
sessão.

**Escolher elemento (app nativo):** o botão **Escolher elemento** da barra do navegador liga o modo
de escolha: o navegador realça o que está sob o ponteiro e o clique não chega à página. Clique num
elemento para mandá-lo ao campo de mensagem; **Shift+clique** soma mais de um. Esc ou o mesmo botão
cancelam. Cada elemento entra na bandeja das citações, com comentário editável, e um recorte da tela
dele vai como anexo. O agente recebe a página, o endereço, o papel e o nome acessível (os mesmos do
snapshot do `hangar-preview`), a tag, um seletor CSS e o HTML do elemento, marcados como referência.

**Pelo celular:** o mesmo navegador da sessão pode ser visto e mexido do PWA — o toque chega no
mesmo lugar que o clique do agente, e arrastar rola a página.

**Abas (só no Electron antigo):** quem ainda usa o Electron tem a faixa de abas, até 8 por sessão.
A aba ativa é uma só, sua e do agente ao mesmo tempo; quando ele precisa mexer numa página sem
tirar da sua frente a que você está olhando, usa uma aba escondida.

#### Usar seu login no navegador embutido

O navegador embutido começa sem login em nada. Para não precisar entrar de novo em cada site, ele
copia do seu Chrome os **cookies** (só eles: senhas, histórico e favoritos ficam no Chrome).

**Como fazer (Linux e Windows):**

1. Abra o site no navegador embutido (por exemplo `github.com`).
2. Clique em **Trazer meu login**, à direita da barra de endereço. Se a página pedir senha,
   aparece também uma linha "Esta página pede login…" com o botão **Usar meu login**: é o mesmo
   caminho.
3. No diálogo **Usar meu login em github.com**, clique em **Trazer login**. A página
   recarrega já logada, e a linha embaixo da barra diz quantos cookies vieram.

**Linux:** o Hangar lê o perfil do Chrome direto do disco, sem diálogo no Chrome e com ele aberto
ou fechado. Vale para Chrome (também Beta, Dev, Flatpak), Chromium (também Snap e Flatpak), Brave e
Firefox. Com mais de um perfil, aparece um botão por perfil ("Chrome — Pessoal") para você escolher.
Precisa do programa `sqlite3` (no Arch: `sudo pacman -S sqlite`) e do chaveiro desbloqueado (o
GNOME Keyring ou a KWallet abre junto com a sua sessão). Sem eles, a linha embaixo da barra diz
o que falta.

**Windows:** o Windows não deixa outro programa ler os cookies do Chrome no disco, então o Hangar os
pede ao próprio Chrome (ou Edge) pela depuração remota. Na primeira vez:

1. Deixe o Chrome (ou o Edge) aberto e clique em **Trazer meu login** → **Trazer login**.
2. O Hangar abre o navegador que você usou por último (Chrome ou Edge) e copia o endereço da página
   que liga a depuração (`chrome://inspect/#remote-debugging`; no Edge,
   `edge://inspect/#remote-debugging`). O navegador não abre essa página sozinho: nele, **Ctrl+L**,
   **Ctrl+V**, **Enter**.
3. Marque **Allow remote debugging for this browser instance** (a opção aparece em inglês, mesmo no
   Chrome em português). Isso fica ligado; não precisa repetir.
4. Volte ao Hangar e clique de novo em **Trazer meu login** → **Trazer login**.
5. O Chrome pergunta **Permitir a depuração remota?**: clique em **Permitir** em até 60 segundos.
   Recusou ou passou do tempo, o Hangar não insiste; é só clicar de novo.

O mesmo vale no Linux quando não há perfil legível no disco.

**Um site ou todos.** O normal é trazer só o site aberto: entram os cookies dele e dos domínios
acima (em `gist.github.com`, também os de `.github.com`). Marcando **Todos os sites**, entram os de
todos — prático, mas **os agentes que usam o navegador embutido ficam logados em tudo em que você
está logado**.

**O agente pode pedir.** O agente da sessão pode chamar `hangar-preview login github.com` quando
precisar entrar como você. Aparece no painel **"O agente pede o seu login do navegador em github.com"**
com **Permitir** e **Recusar**; só um clique seu, no app, aprova (nem o agente nem o celular
alcançam esses botões). O agente fica sabendo só quantos cookies vieram, nunca o valor deles.

**Segurança.** O navegador embutido é **um só para todas as sessões** desta máquina: um login
trazido vale no navegador de qualquer sessão e para qualquer agente que o use. Traga só o site de
que o trabalho precisa.

**O que não funciona.** O **Google** (Gmail, Drive, YouTube logado) recusa o navegador embutido:
digitando a senha aparece "Esse navegador ou app pode não ser seguro", e os cookies trazidos não
abrem a conta. Para esses, use o Chrome normal.

**Desfazer.**

- Um site: saia da conta nele, no próprio navegador embutido.
- Tudo: feche o app e apague a pasta do perfil do navegador embutido — no Linux
  `~/.config/hangar-native/chromium`, no Windows `%APPDATA%\hangar-native\browser`. Ela volta vazia
  na próxima abertura. Não há botão para isso no app.
- A depuração remota do Chrome (Windows): desmarque a opção na mesma página `chrome://inspect`.

### Checkpoints de código (Pi e OMP)

Requer **Git 2.32 ou superior**, para isolar a configuração global durante as operações.

Com a extensão do Hangar carregada, cada pedido em uma árvore Git recebe um checkpoint antes
da atuação do agente. No **OMP**, use `/hangar-rewind`; no **Pi**, use `/rewind`.
Escolha o checkpoint do ramo atual e um dos três modos:

- **Código e conversa:** repõe os arquivos e reposiciona a conversa.
- **Somente conversa:** mantém os arquivos como estão.
- **Somente código:** mantém a conversa como está.

Arquivos modificados ou apagados voltam ao estado capturado. **Arquivos criados depois são
preservados**, e o índice, a branch e os commits do seu repositório não são alterados.
As exclusões do Git são respeitadas; os objetos ficam em `<agentDir>/checkpoints`, onde
`agentDir` vem de `PI_CODING_AGENT_DIR` ou da pasta padrão do harness.

Retomar ou ramificar uma sessão preserva a referência aos objetos originais. Se eles não
estiverem disponíveis ou pertencerem a outro projeto, a restauração do código é recusada.
Mudança de sessão/estado durante a escolha exige uma nova seleção. Se os arquivos voltarem
mas a conversa falhar, o aviso informa a conclusão parcial. No OMP, uma captura que exceda
25 segundos interrompe o pedido, sem publicar um checkpoint tardio.

### Marketplaces e plugins no OMP

A importação de marketplaces do Claude usa o gerenciador nativo do OMP. É genérica: não
depende do nome do catálogo ou do plugin. Catálogos já registrados com a mesma origem não
são importados novamente; um nome ocupado por outra origem é preservado e informado como
conflito. Origens inválidas ou não representáveis são informadas, sem mudar configurações.

**Importar um marketplace não instala todos os seus plugins nem converte instalações Git
existentes.** Os plugins instalados por marketplace continuam sob responsabilidade do OMP.
Na opção nativa **Marketplace Auto-Update**, `notify` (padrão) verifica e avisa na abertura
da sessão; `auto` também instala as atualizações. A importação não altera essa preferência.

Para plugins Git diretos elegíveis, a integração compara origem e revisão, não apenas o
nome ou a versão textual. Arquivos/preferências alterados manualmente suspendem a gestão.
Plugins sem prova suficiente são diagnosticados, não instalados por suposição. A inspeção
`dry_run` não executa instaladores nem escreve no perfil pessoal.
Registro de propriedade inválido interrompe a passagem sem remover plugins. Se duas origens
disputarem o mesmo nome de pacote, nenhuma vence pela ordem do cadastro; a instalação
existente é preservada e o conflito é informado. Os diagnósticos não incluem linhas brutas
de arquivos de configuração ou credenciais.

**Execução periódica (opcional):** configure `CP_OMP_PLUGIN_SYNC_ENABLED=1` no ambiente do
backend. `CP_OMP_PLUGIN_SYNC_INTERVAL` define o intervalo em segundos (padrão **300**; deve
ser positivo e finito). Sem habilitação explícita, nenhuma passagem é iniciada. O controle
global de automações também precisa estar habilitado; a integração não o liga por conta própria.

A primeira passagem acontece na subida; as seguintes começam após o intervalo contado do
fim da anterior, sem sobreposição. A API permanece disponível durante o trabalho. No
encerramento, o backend sinaliza parada e aguarda a operação em andamento antes de sair.

Consulte **`GET /api/omp/plugin-sync`**, com a autenticação normal da API, para ver
`disabled` (desligado), `paused` (automações pausadas), `running` (executando), `updated`
(houve ações), `unchanged` (sem mudanças), `suspended` (conflito/alteração manual), `error`
(falha) ou `stopped` (encerrado). O relatório detalha cada catálogo/plugin; `updated` não
significa que candidatos sem prova foram instalados. Erros não encerram o ciclo periódico.

O diretório global de plugins não é derivado do diretório do agente. A integração respeita
`PI_CONFIG_DIR`, o perfil OMP selecionado e o layout XDG já existente. Um agente em diretório
personalizado não desloca sozinho os plugins. Caso a configuração passe a apontar para
outra raiz, um registro de propriedade antigo é preservado e diagnosticado, não migrado
automaticamente.

### Contexto CLAUDE.md no OMP

Com `CP_OMP_CLAUDE_CONTEXT_ENABLED=1`, a subida do backend configura somente o OMP:

- Usa o `CLAUDE.md` global existente por um link `APPEND_SYSTEM.md`, sem copiar seu conteúdo
  para o repositório e sem substituir um arquivo/link personalizado.
- Instala ou reutiliza a regra que exige ler o `CLAUDE.md` do projeto antes do trabalho.
- Acrescenta os dois identificadores de contexto AGENTS à lista de recursos desativados,
  preservando as outras entradas. Nenhum arquivo `AGENTS.md` é apagado.

Se o projeto não tiver `CLAUDE.md`, a regra exige informar a ausência, não fingir que o
arquivo foi carregado. Conflitos com contexto personalizado são informados, sem sobrescrita.
Uma regra desativada, restrita a agentes/condições ou bloqueada nas configurações pessoais
não é reativada por conta própria. Regras equivalentes `.md` e `.mdc` no nível direto são
reutilizadas; arquivos em subdiretórios não substituem uma regra que o OMP precisa descobrir.
Mudanças concorrentes nos arquivos ou no diretório de regras geram diagnóstico, sem gravar
em um destino externo.
A alteração usa o CLI nativo de configuração, que pode normalizar formatos legados sem
mudar a preferência efetiva. Reabra a sessão OMP para carregar a política recém-configurada.

### Git

O ícone de branch abre o **modal de git** da sessão — o mesmo nas duas views: no desktop ele é um
modal centrado, no celular uma folha que sobe. O cabeçalho diz de qual repositório é (nome da sessão
e branch atual), porque o modal também abre pela linha da lista, sem abrir o chat.

Três abas, com a contagem no rótulo:

- **Mudanças** — uma lista só dos arquivos alterados. Cada linha tem o checkbox (entra no commit), o
  caminho (abre o diff) e o **⟲** (descarta as mudanças daquele arquivo, com confirmação em dois
  passos). **todos**/**nenhum** marcam tudo ou nada; a sua escolha manual não é refeita pelo poll.
  Abaixo da lista, a caixa de commit: o select **mensagens recentes…** reaproveita as últimas 10
  mensagens daquela sessão, **reescrever o último commit (amend)** traz a mensagem preenchida (com
  amend o botão Commit & Push some — push de amend exigiria `--force`), e dá pra **commitar numa
  branch nova**, criada a partir da atual. Repo limpo diz que está limpo.
- **Histórico** — busca, lista de commits com grafo, mensagem completa do commit (assunto **e**
  corpo) e os arquivos dele. No desktop os painéis convivem empilhados; no celular é drill-down
  (lista → commit → diff) e o botão voltar sobe um nível. A busca filtra pelo texto da mensagem
  (`git log --grep`, ignora maiúsculas) e esconde o grafo enquanto está ativa — os commits do meio
  saem da lista e as linhas não teriam onde ligar.
- **Branches** — locais e remotas com a atual no topo, e o filtro por nome, que agora existe nas duas
  views e aparece sempre.

Cada aba lembra em que nível estava: trocar de aba e voltar não perde o lugar.

- **Ações por commit:** o botão **⋯** (na lista ou no painel de arquivos) abre o menu do commit:
  diff completo num único texto, comparar o commit com a working tree, copiar hash/mensagem/detalhes
  completos, ver as branches que contêm aquele commit, criar branch ou tag naquele ponto,
  cherry-pick, revert (cria um commit novo desfazendo) e reset até ali (soft/mixed/hard — o hard
  pede confirmação dupla).
- **Ações do repositório:** o **⋯** do cabeçalho traz status, log, fetch, pull, push, stash e pop.
- **Faixa do rodapé:** mostra o erro do git e a saída do último comando, visível de qualquer aba. Se
  um cherry-pick/revert der conflito, o aviso e o botão **abortar** ficam ali até você resolver —
  fechar e reabrir o modal não perde o estado, que é lido do próprio repositório.
- Sessão cujo diretório não é repositório git diz isso em uma frase, sem despejar a saída do git.

### Rodar uma sessão em outro modelo (Kimi, gateway próprio, …)

Dá para abrir uma sessão que roda em outro provedor de modelo sem criar perfil novo e sem
desconectar sua conta Anthropic. A sessão continua no **mesmo** `~/.claude`: skills, hooks,
`CLAUDE.md`, plugins, statusline e histórico, tudo igual — só muda um punhado de variáveis de
ambiente no processo daquela sessão.

**Configurar:** menu da conta → **Configurações** → **Contas e provedores** → **+ Nova conta**. Preencha o
endereço e a chave e toque em **Testar e listar modelos**: os ids e a janela de contexto vêm do seu
provedor, com a sua chave — nada de tabela chumbada que envelhece. O mesmo botão serve de checagem
de conectividade/chave: chave errada volta com a mensagem do próprio provedor, não um "não
respondeu" genérico. Subagentes, janela de contexto e as opções avançadas ficam em **Modelo e
opções**, dentro do card da chave, depois de criada.

- O endereço vai **sem o `/v1`** no fim (o Claude Code monta o caminho).
- **Kimi Code** é `https://api.kimi.com/coding` — e **não** é a mesma coisa que a plataforma aberta
  da Moonshot: chave de uma dá `Invalid Authentication` na outra. Os ids de modelo também são
  próprios da Kimi Code (`k3`, `k3-256k`, `kimi-for-coding`, `kimi-for-coding-highspeed`) — não
  `kimi-k3`.
- **Modelo dos subagentes** é opcional: em branco, subagentes rodam no mesmo modelo principal. Como
  eles fazem muita busca mecânica, apontar um modelo mais barato aqui é economia real sem tocar o
  modelo da sessão.
- A janela de contexto depende da sua **faixa de assinatura**: o mesmo `k3` já reportou 262144 num
  plano Moderato onde a documentação da Kimi fala em "até 1M". É por isso que o valor vem do
  provedor a cada teste, e não de uma tabela na documentação. **Depois de cadastrar um motor novo,
  confira a janela real com `/context` na sessão** — errar essa variável custa capacidade de
  contexto em silêncio (ver adiante).

**Abrir pelo celular:** na criação de sessão, escolha o motor no seletor **Motor**. Sessão de motor
aparece na lista com o chip `⚙ <nome>`. **Retomar uma conversa do Arquivo também oferece o
seletor de motor** — o app não tem como saber qual motor gerou aquele transcript originalmente (o
processo que rodava morreu, e o transcript grava o nome do modelo, não qual motor serviu ele), então
a escolha é sempre sua, de novo, a cada resume.

**Abrir pelo terminal:**

```bash
claude-engine            # lista os motores configurados
claude-engine kimi       # abre uma sessão no motor "kimi"
claude                   # continua na sua conta Anthropic, como sempre
```

(`hangar-engine --env` existe, mas é só diagnóstico interno — ele imprime a chave em texto puro no
stdout. Use `claude-engine`.)

**O que muda numa sessão de motor:**

- O cabeçalho mostra `<modelo> · API Usage Billing`: o consumo vai para a conta do provedor, não
  para a sua assinatura Anthropic.
- **O valor em `💵` não aparece**, de propósito: o preço que o Claude Code calcula é tabela Anthropic
  e não corresponde ao seu provedor (a statusline também para de gravar o sidecar de custo). Veja o
  consumo no painel dele. As barras `⚡5h`/`📅7d` também somem — são um dado que só a Anthropic manda.
  O esforço (`(high✦)` etc.) continua aparecendo normalmente: não é fingido, e em provedores como o
  Kimi o "pensando" por trás dele é real — só que alguns gateways ignoram o esforço pedido no request
  e escolhem pelo sufixo do id do modelo, então nem todo provedor obedece o que você pede ali.
- Connectors MCP vindos do claude.ai ficam desativados (o Claude Code avisa). MCP local funciona.
- Todo o seu harness vai em cada turno (num teste real, 81k tokens de input num prompt de uma linha).
  Em provedor cobrado por token, isso pesa por turno.
- Cuidado com `/model` + Enter: numa sessão de motor isso **não muda nada visível** e troca o tier
  default das suas sessões da conta Anthropic. Aperte `s` no seletor para valer só na sessão atual.
- Um hook ou skill que rode `claude` dentro de uma sessão de motor herda o motor, e é cobrado nele.
- Editar um motor não afeta sessões já abertas: elas seguem no valor antigo até serem retomadas.

**Gateway só-OpenAI** (OpenAI ou Gemini direto): rode um proxy tradutor (LiteLLM ou
`anthropic-proxy`) em `127.0.0.1` e cadastre o motor apontando para o proxy. OmniRoute e Kimi Code
**não** precisam disso — falam a Messages API nativamente.

### Ouvir em voz alta (TTS)

Qualquer resposta do assistente — ou só um trecho selecionado — pode ser ouvida em voz alta, útil
pra acompanhar um plano longo sem ficar rolando a tela.

**Ligar:** menu da conta → **Configurações** → **Voz** → **Ler em voz alta** → **ElevenLabs: vozes
e ajustes** → cole a **Chave da ElevenLabs**. Sem chave, o `🔊` fica sem efeito (o servidor recusa
com uma mensagem explicando que falta configurar). Na mesma tela: **Voz** (carrega as vozes da sua conta e deixa escolher
uma, ou "Padrão do servidor"), **confirmar leitura acima de** (quantos caracteres pedem confirmação
antes de gerar o áudio — custo de verdade, cobrado na sua conta ElevenLabs) e o **consumo do mês**.

**Usar:**

- **Mensagem inteira** — toque no `🔊` no rodapé da bolha do assistente, ao lado de copiar e
  encaminhar.
- **Um trecho** — selecione texto dentro de uma bolha: nasce um botão `🔊 Ouvir · N car.` (pill perto
  da seleção no desktop, barra rente ao campo de mensagem no celular). Tocar nele lê só o que foi selecionado.

Seleção grande (acima do limite configurado) pede confirmação antes de gastar; acima do teto duro do
modelo, é recusada mesmo confirmando. O áudio já gerado fica em cache (mesmo texto + mesma voz nunca
paga duas vezes) e uma barra de player aparece com posição e velocidade.

**Motor local (opcional):** em vez da ElevenLabs, aponte **Comando de voz local** (mesma tela, em
**Leitor de voz instalado**) para um programa que recebe o texto no stdin e devolve áudio (mp3 ou wav) no stdout — Kokoro,
piper, ou o que você tiver instalado na máquina do servidor. Com a chave da ElevenLabs vazia e o
comando configurado, a leitura usa o motor local automaticamente.

## 5. Sessões-irmãs, pareamento e orquestração (hangar-send)

**Histórico de orquestrações:** no Rust, abra o relógio da barra superior; no PWA, abra
**Orquestração**. As execuções continuam disponíveis depois que a sessão sai da lista, com nome
do plano, projeto, duração e tarefas. O detalhe reaproveita o painel da execução e indica quando
o consumo é parcial. O resumo de uso da tela inicial é uma visão separada, por período e modelo.

As sessões conversam entre si pelo backend via `scripts/hangar-send`, qualquer que seja o agente
(Claude, Codex, Pi, omp, Kimi). Sessão de outra máquina cadastrada é `servidor::sessao`:

```bash
hangar-send --list                    # sessões vivas (nome, estado, harness, cwd), locais e remotas
hangar-send api-fix "mensagem"        # manda prompt pra outra sessão (fila se ocupada)
hangar-send casa::api-fix "mensagem"  # idem, numa sessão de outro servidor
hangar-send --pair api-fix "tarefa"   # pareia ESTA sessão com outra num grupo de trabalho
hangar-send --pair casa::api "tarefa" # idem com sessão de outra máquina: o grupo junta as duas
hangar-send --group "terminei"        # aviso de marco pro grupo todo, em todas as máquinas dele
hangar-send --contrato                # lê o contrato do grupo (de qualquer máquina do grupo)
hangar-send --contrato --gravar c.md --mtime <valor>  # grava; o mtime vem da leitura
hangar-send --close api-fix           # fecha OUTRA sessão desta máquina
hangar-send --aceitar-par <link>      # pareia com a sessão de outra pessoa (link …ts.net:8443/par/…)
hangar-send --new front ~/repo/front  # cria sessão nova gerenciada pelo app (visível na UI)
hangar-send --new front ~/repo/front --headless  # Claude sem terminal
hangar-send --new front ~/repo/front --terminal  # força terminal, sem alterar o padrão
hangar-send --new api ~/repo/api --provider codex --headless  # Codex sem terminal
hangar-send --new rev ~/repo --model <id> --effort high --permissao <modo>  # nasce já configurada
hangar-send --new rev ~/repo --conta <nome>   # outra conta (--conta auto: a de mais folga)
hangar-send --new rev ~/repo --engine kimi    # num motor de ~/.claude/engines.json
hangar-send --new rev ~/repo --jev            # liga o Jev (hangar-preview objetivo) na sessão
```

A sessão criada herda desta o que você omitir (conta, modo de permissão, com/sem terminal).
Referência completa e sempre atual: `hangar-send --help`.

**Instalar** (uma vez por máquina; o passo 7/8 do `install.sh` também oferece):

```bash
./scripts/install-hangar-send.sh
```

O installer symlinka o `hangar-send` em `~/.local/bin`, adiciona o bloco "Sessões-irmãs"
no `~/.claude/CLAUDE.md` global (toda sessão Claude nova passa a conhecer a ferramenta),
symlinka as skills do repo (`skills/*`) em `~/.claude/skills/` e registra o MCP `hangar`.

**MCP `hangar`:** o mesmo `hangar-send` e o `hangar-preview` como ferramentas tipadas, sem shell.
O instalador o registra no Claude Code (`~/.claude.json` de cada conta) e no Codex (`config.toml`
de cada `CODEX_HOME`); o token não entra no ambiente da sessão. Ferramentas: `who_am_i`,
`sessions`, `send`, `group`, `pair`, `unpair`, `contract`, `new_session`, `close_session`, `browser_open`,
`browser`, `browser_batch` e `html_render` (mostra uma página HTML dentro da conversa). O que não
tem ferramenta (`--aceitar-par`, `hangar-preview objetivo`…) continua no CLI. Sessão aberta
antes do registro, Pi, omp e Kimi usam só o CLI.

**Pareamento:** `--pair` registra um grupo no app (badge 🤝 na lista, PairSheet com a
conversa do par + contrato compartilhado em markdown) e injeta o protocolo de
colaboração em cada membro — cada sessão mexe só no próprio repo, recados 1:1 por
iniciativa própria dentro da tarefa, push/merge continuam com o usuário. Pareando
N sessões uma a uma os grupos se fundem num só.

**Grupo entre máquinas:** parear com `servidor::sessao` (máquinas do `backend/peers.json`, com
`CP_SERVER_ID` em cada uma) junta sessões de várias máquinas num grupo só. A máquina onde o grupo
nasceu é a **dona**: guarda a lista de membros e o contrato. Avisos de grupo chegam a todas as
máquinas; quem está em outra máquina aparece como `servidor::sessao` e lê ou grava o contrato
pela dona (`hangar-send --contrato` ou a ferramenta `contract`). Máquina desligada recebe a saída
e a troca de nome de membros quando volta; o aviso de grupo não espera e a resposta diz quem
ficou sem ele. A sessão de outra pessoa pareada com uma das suas máquinas entra no grupo, mas só
alcança as sessões daquela máquina. Um par antigo entre duas máquinas vira grupo sozinho quando
as duas estão atualizadas.

**Skill `orquestrar`:** conduz um trabalho, em um ou vários repositórios, com revisão
independente. Só roda quando você pede ("orquestra", "monta o time"). Junto com você, o
**planejador** escreve o plano e escolhe a rota: **`audit`** (quem planejou escreve e uma revisão
de contexto limpo confere o diff inteiro) ou **`full`** (um **árbitro** abre o time; para cada Task
um **executor** escreve e um **revisor** independente aprova antes da próxima, e uma revisão final
confere a branch). Cada papel é uma sessão própria, até em modelos diferentes; push continua
dependendo de você. A **`orquestrar-auto`** (só pelo nome) é a mesma esteira com um orquestrador
sem modelo soltando as Tasks e abrindo executor e revisor; o árbitro só acorda para decidir.

**Painel/tray no desktop (só Hyprland + Quickshell):** painel flutuante de sessões
(SUPER+SHIFT+U) + ícone na bandeja. O passo 7/8 do `install.sh` oferece quando detecta
o ambiente; manual: `./scripts/install-hangar-panel.sh`. Nos outros desktops, a bandeja é a do
próprio app nativo ([App de desktop](#app-de-desktop-nativo)).

## 6. Sincronização entre aparelhos (opcional)

A sincronização permite cadastrar suas máquinas uma vez e recuperar a lista e os
tokens de acesso em outros celulares, PCs e notebooks. As conversas continuam nas
máquinas onde as sessões rodam. O servidor principal pode ser qualquer computador
com Hangar; escolha um que costume ficar ligado e acessível pelos outros aparelhos.

**Ativar e criar o acesso pela tela:**

1. Abra **Configurações → Sincronização** e selecione a máquina que será a principal.
   Você precisa já ter acesso a ela pelo token normal do Hangar.
2. Preencha usuário, senha e confirmação. Clique em **Ativar sincronização**.
3. O Hangar cria o acesso e salva sua lista atual criptografada nessa máquina.
   Não é preciso editar arquivos, usar um token de ativação ou reiniciar o servidor.
4. Use **Abrir Hangar sincronizado**. Nos outros aparelhos, abra o endereço mostrado
   na tela e entre com o mesmo usuário e senha. Existe um único cadastro por servidor principal.

Abra por HTTPS para o navegador poder proteger a senha; HTTP só permite essa
criptografia em `localhost`/loopback. A senha não é enviada ao servidor, e a lista
de acessos é cifrada no navegador. Guarde a senha: não há recuperação pelo Hangar.

**Desativar e reativar:**

Na mesma tela, clique em **Desativar sincronização** e confirme. Isso interrompe o
serviço de sincronização dessa máquina imediatamente, preservando a conta, a lista
cifrada e as sessões. Os acessos já salvos em cada aparelho continuam disponíveis.
Ao reativar, o Hangar reutiliza o mesmo cadastro e senha; não cria outra conta.

**Sem sincronização:** cada navegador/PWA mantém sua lista local. A abertura lembra
o modo conhecido desse endereço e verifica alterações em segundo plano. No primeiro
acesso, a verificação tem prazo e mostra uma opção de tentar novamente se a conexão
falhar, em vez de deixar apenas o papel de parede.

Instalações antigas com `CP_SYNC=1` e `CP_SYNC_BOOTSTRAP` continuam compatíveis.
As escolhas feitas na tela ficam salvas na configuração do Hangar.

### Configuração compartilhada entre máquinas

Configurações → Servidor → **Configuração compartilhada** leva a configuração do Claude Code e do
Codex de uma máquina para outras: skills, agents, hooks (com os arquivos que eles usam), barra de
status, plugins, MCPs, variáveis de ambiente, motores, preferências do Hangar e o `AGENTS.md` e o
`config.toml` do Codex. Também leva as contas Claude e a aparência do app nativo:

- **Contas Claude**: cada `~/.claude-<nome>` criada pelo Hangar chega ao destino com o mesmo nome,
  o apelido e as chaves do `settings.json` que só ela tem. Conta nova nasce sem login (o destino
  avisa e cada uma entra por Configurações → Contas e provedores); conta que já existe lá continua logada como
  estava. Pasta `~/.claude-<nome>` que não é conta do Hangar fica intocada.
- **Aparência do app nativo**: tema, cores, fundo (com a imagem), fontes e o jeito da conversa. O
  app aberto no destino aplica sozinho em poucos segundos. Ficam em cada máquina os tamanhos
  arrastados de painel, idioma, moeda, bandeja, preenchimento de senha e o aviso antes de comando
  destrutivo. A aparência do web/PWA mora em cada navegador e não viaja.

1. Escolha a **origem** (qualquer máquina cadastrada), os **destinos** e o que levar.
2. **Comparar** mostra, por destino, o que é novo, o que mudou, o que já é igual e o que só
   existe lá (isso fica).
3. **Enviar** baixa o pacote da origem uma vez e aplica em cada destino. Quem envia vence; o
   destino guarda uma cópia do que trocou em `~/.hangar/config-sync/backups/<data-hora>/`.

Os caminhos são resolvidos no destino (inclusive Windows), e o programa de um hook que não existe
lá (o `node` de outra versão, por exemplo) é trocado pelo que o destino tem no PATH. Não vão:
credenciais (`.credentials.json`, `auth.json`), o login do `.claude.json` (nem o das contas), do
`settings.json` de cada conta as credenciais (`env`, `apiKeyHelper`), hooks, plugins e permissões,
o `CLAUDE.local.md`, os hooks e skills do próprio Hangar e o MCP `hangar` de cada máquina. As pastas `.venv`,
`node_modules` e `.git` de uma skill ficam de fora e, no destino, as que já existiam continuam.
Máquina com Hangar anterior a esta tela aparece como "atualize o Hangar lá".

## 7. Problemas comuns

### Onde ficam os logs

- **Windows:** `%LOCALAPPDATA%\hangar\logs\`.
- **Linux/macOS:** `~/.hangar/logs/`.

O botão **Baixar diagnóstico** exporta o diário de `diario/`, com etapas de login, contas,
conexões, sessões e falhas internas. Os sete dias mais recentes ajudam a ligar o que aconteceu
na tela ao motivo registrado pelo servidor. Diários do local antigo continuam no download.

`privado/` contém os logs técnicos completos, incluindo `backend.log` e os logs da instalação
e dos hooks. Esses arquivos podem conter dados sensíveis e não entram no download do diário.
O log do backend conserva até três arquivos anteriores por rotação. No Windows, atualizar
o instalador ajusta também os logs dos lançadores e da vigia para essa pasta.

| Sintoma | Causa / fix |
|---|---|
| Recusa subir ("Refusing to start") | token ainda é `change-me` + bind não-loopback. Gere `CP_AUTH_TOKEN`. |
| 401 / "lost input" no celular | token velho/rotacionado. Re-pareie (QR) ou limpe credenciais e logue de novo. |
| App "congelou" no último estado | conexão SSE morreu calada (mobile/background). O watchdog reconecta; senão recarregue (pull-to-refresh). |
| Não vejo código novo após mudar | PWA com service worker servindo JS velho → **hard reload** / limpar dados do site / re-adicionar o PWA. |
| Backend reiniciar | precisa do cwd=`backend` (`python -m app.main` acha `app`). Sem `--reload` (trava SSE no SIGTERM). |
| Pane de sessão de motor morre na hora, sem chat nenhum | `hangar-engine` não está no PATH do **servidor tmux** (a sessão nasce via `hangar-engine --exec`). Garanta que o PATH usado pelo tmux enxerga `hangar-engine` (mesmo instalado pelo `install-claude-wrapper.sh`). |
| Contas e provedores avisa que não conseguiu ler o `engines.json` | `~/.claude/engines.json` foi editado à mão e ficou com JSON inválido — corrija-o (ou restaure um backup) antes de adicionar um motor novo; o app se recusa a gravar por cima de um arquivo que não conseguiu ler, pra não apagar os motores que já estavam lá. |

## 8. Segurança (resumo)

- Bind só na LAN/VPN, **nunca** interface pública; **nunca** port-forward no roteador nem túnel
  público para a 8765.
- Só a porta do convidado (8766, via Funnel em 8443) vai à internet, enquanto houver convite ou par
  externo ativo; ela recusa o token do dono.
- O token é a senha — trate como senha de shell. TLS na frente (Caddy/Tailscale) antes de uso real.
- Fora de casa = VPN de volta pra LAN (Tailscale/WireGuard).
