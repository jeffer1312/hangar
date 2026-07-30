# Como usar o claude-cockpit

Guia de uso ponta-a-ponta: subir, conectar o celular (LAN ou Tailscale), instalar
como PWA e operar o chat. Pra arquitetura/API ver o [README](../README.md).

> **Modelo:** ferramenta pessoal, single-user, **LAN/VPN-only**. Roda o `claude` **como
> você** (bypass) → um host exposto é execução-remota-como-você. A trava é o **token**.
> NUNCA faça port-forward pra internet pública. Fora de casa = **VPN (Tailscale)**.

---

## 1. Pré-requisitos

- `tmux`, `claude` (Claude Code), Python 3.14 + [`uv`](https://docs.astral.sh/uv/), Node 20+.
- Celular na **mesma rede** do PC (Wi-Fi) **ou** ambos no **mesmo tailnet** (Tailscale).

**Instalar é uma linha só** — ela clona o repositório em `~/claude-cockpit` e chama o instalador:

```bash
# Linux/macOS
curl -fsSL https://raw.githubusercontent.com/jeffer1312/claude-cockpit/main/bootstrap.sh | bash
```

```powershell
# Windows
irm https://raw.githubusercontent.com/jeffer1312/claude-cockpit/main/bootstrap.ps1 | iex
```

Outra pasta de destino, ou flags do `install.sh`, vão **depois de `-s --`** (sob `curl | bash` é
esse separador que impede o próprio bash de comê-las):

```bash
curl -fsSL …/bootstrap.sh | bash -s -- ~/apps/claude-cockpit --no-frontend
curl -fsSL …/bootstrap.sh | bash -s -- --check      # só confere dependências e sai
```

No Windows, defina `$env:CP_DESTINO = 'D:\claude-cockpit'` **antes** da linha do `irm` — sob
`irm | iex` o script chega como texto e não recebe argumento nenhum. Rodar de novo é seguro: se a
pasta já for este repositório ele faz `git pull` em vez de clonar; se for outra coisa, ele **para**
em vez de mexer no que é seu.

Prefere ver o que está rodando antes? Clone na mão — dá no mesmo:

```bash
git clone https://github.com/jeffer1312/claude-cockpit && cd claude-cockpit
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
ConPTY) — não existe `tmux` no Windows, e o WSL não é necessário. Três coisas **não** vão junto,
porque são shell script: `cp-send` (recado/pareamento entre sessões), o wrapper do `codex`, e os
plugins de persistência entre reboots. Sessão do Codex, só criada pelo app.

## 2. Subir (3 partes)

**a) Claude ou Codex gerenciado dentro do tmux** (a sessão que o app vai espelhar):
```bash
tmux new -s cc        # rode `claude` dentro dela
```

Com o wrapper recomendado instalado (`./scripts/install-claude-wrapper.sh`), basta executar
`claude` ou `codex` normalmente. O `codex` pede ao backend uma sessão gerenciada e anexa o terminal
ao tmux dela; a conversa aparece imediatamente no app. `command codex` ignora o wrapper.
Cores erradas (teal/pink) no tmux? Fix em [tmux-truecolor-setup.md](tmux-truecolor-setup.md).
Sobreviver a reboot/OOM? `./scripts/tmux-persist-setup.sh` ([doc](tmux-persistence-setup.md)).

**b) Backend** (FastAPI, porta 8765):
```bash
cd backend
CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_LAN_BIND_IP=auto uv run python -m app.main
```
No boot ele imprime um **QR** (URL + token) pra parear o celular. Variáveis (prefixo `CP_`,
ou em `backend/.env`):

| Var | Default | Pra quê |
|---|---|---|
| `CP_AUTH_TOKEN` | `change-me` | senha que protege TODA rota. Gere um forte. |
| `CP_LAN_BIND_IP` | `127.0.0.1` | `auto` = detecta o IP da LAN (pro celular alcançar). IP fixo também vale. |
| `CP_PORT` | `8765` | porta do backend |
| `CP_FRONT_PORT` | `5173` | porta onde o PWA é servido (entra no QR) |
| `CP_PUBLIC_URL` | — | sobrescreve a URL base do QR (ex: hostname Tailscale) |
| `CP_SCAN_ROOTS` | — | pastas que o seletor "Nova sessão" pode listar (csv) |

> Guarda de segurança: com `CP_AUTH_TOKEN=change-me` ele **recusa** subir num bind não-loopback.

**c) Frontend** (PWA, Vite):
```bash
cd frontend
npm install
npm run dev -- --host      # serve em http://<ip>:5173
```

## 3. Conectar o celular

### Opção A — LAN (mesma Wi-Fi)
1. `CP_LAN_BIND_IP=auto` no backend.
2. Escaneie o **QR** do terminal (ou abra `http://<ip-da-lan>:5173`).
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
tailscale serve --bg 5173      # publica o vite (5173) em https://<maquina>.<tailnet>.ts.net
tailscale serve status         # mostra a URL exata
```
**5. No celular** (com Tailscale ligado) abra `https://<maquina>.<tailnet>.ts.net` → cadeado
válido (Let's Encrypt) → escaneie o QR / preencha o token → **Adicionar à Tela de Início** (PWA).

> Fonte: [Tailscale — Set up HTTPS](https://tailscale.com/docs/how-to/set-up-https-certificates)
> · [tailscale serve](https://tailscale.com/docs/reference/tailscale-cli/serve). NÃO use
> `tailscale funnel` (isso expõe à internet pública — fora do modelo LAN/VPN-only).

> O app fala com o backend **cross-origin** quando preciso (multi-PC): ele aceita o token via
> header **e** via `?token=` (porque `EventSource`/`<img>` não mandam header). CORS já liberado
> (token-gated, sem cookies cross-site).

### Instalar como PWA (tela cheia)
- **iOS (Safari):** Compartilhar → **Adicionar à Tela de Início**. Abre standalone (sem barra do Safari).
- **Android (Chrome):** menu → **Instalar app**.

## 4. Operar o chat

### Sessões
- **Criar:** botão **＋ / Nova sessão** → escolha a pasta (cwd). O backend roda
  `claude --session-id <novo>` num tmux novo → vem **limpa** (resolve o transcript pelo
  processo, não pelo mais recente).
- **Trocar:** toque no título (mobile) / clique na sidebar (desktop).
- **Renomear:** **toque longo** no nome (sidebar/desktop) → edita inline → Enter salva.
  Não quebra o histórico (resolve por `/proc`, não pelo nome).
- **Apagar:** × na linha (mata o tmux).

### Enviar
- **Texto:** digite e envie. **Multi-linha** funciona (Shift+Enter / colar — vai por bracketed paste).
- **Imagem / arquivo:** 📎 no composer (upload) — ou cole no terminal do Claude que o app mostra o thumbnail.
- **Áudio:** 🎤 no composer grava pelo microfone (toque grava, toque ⏹ para); ou anexe um arquivo de
  áudio pelo 📎. No envio o áudio é **transcrito** (Groq / whisper-large-v3-turbo) e vai como texto
  + o áudio anexado. **Requer a chave da Groq:** `CP_GROQ_API_KEY=<sua-chave>` no `backend/.env`
  (ou `GROQ_API_KEY` no ambiente do backend) e reinício do backend. Sem chave, o envio de áudio
  responde 503. Pegue a chave grátis em <https://console.groq.com>.
- **Slash commands:** `/` abre a lista (`/clear`, `/compact`, …). `/clear` limpa de verdade (zera a fila).
- **Modelo/esforço:** toque na pill (ex `Opus4.8·1M·high`) → escolhe modelo + esforço (só na sessão).
- **Pergunta interativa do Claude** (AskUserQuestion/permissão): as opções viram **botões** —
  toque. (Se não renderizar como botão, responda com o **número** em texto.)

### Acompanhar
- **Streaming ao vivo:** enquanto o Claude escreve, aparece um **preview** da prosa (box contido,
  marcado com hairline). Vira a mensagem final (markdown limpo: tabelas, listas, código) quando fecha.
- **Estado:** spinner com o label do Claude (`Forging…`), firme (com debounce anti-flicker).
- **Atividade / Workflows:** ícone de atividade no topo (pulsa quando há workflow/agente rodando) →
  abre o painel: tarefas + workflows → fases/agentes → prompt+resultado de cada agente (3 níveis).
- **Interromper:** botão **⏹ stop** (manda `Esc`).

### Multi-PC
Cada PC roda backend+vite+`tailscale serve` com o **mesmo** `CP_AUTH_TOKEN`. O app guarda **N
servidores** e troca entre eles (switcher) — útil pra dirigir o Claude de máquinas diferentes do
mesmo celular.

### Desktop (≥820px)
Abrindo a mesma URL num monitor largo, vira **shell de duas colunas**: sidebar de sessões +
chat largo. O fluxo mobile fica intacto abaixo de 820px.

A barra lateral tem dois ajustes em **Aparência** (menu da conta), só no desktop:

- **Barra lateral aberta** — mantém a lista aberta o tempo todo. Desligada (padrão), ela fica no
  trilho de iniciais e só abre enquanto o mouse está por cima.
- **Altura da barra** — aparece quando a de cima está ligada: **altura total** (de ponta a ponta) ou
  **só o conteúdo** (a barra encolhe até onde as sessões terminam e fica flutuando, centralizada).

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

### Motores de modelo (Kimi, gateway próprio, …)

Dá para abrir uma sessão que roda em outro provedor de modelo sem criar perfil novo e sem
desconectar sua conta Anthropic. A sessão continua no **mesmo** `~/.claude`: skills, hooks,
`CLAUDE.md`, plugins, statusline e histórico, tudo igual — só muda um punhado de variáveis de
ambiente no processo daquela sessão.

**Configurar:** menu da conta → **Configurações** → **Motores de modelo** → Adicionar. Preencha o
endereço e a chave e toque em **Testar e listar modelos**: os ids e a janela de contexto vêm do seu
provedor, com a sua chave — nada de tabela chumbada que envelhece. O mesmo botão serve de checagem
de conectividade/chave: chave errada volta com a mensagem do próprio provedor, não um "não
respondeu" genérico.

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

(`cp-engine --env` existe, mas é só diagnóstico interno — ele imprime a chave em texto puro no
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

## 5. Sessões-irmãs, pareamento e orquestração (cp-send)

Sessões Claude da mesma máquina conversam entre si pelo backend via `scripts/cp-send`:

```bash
cp-send --list                    # sessões vivas (nome, estado, cwd)
cp-send api-fix "mensagem"        # manda prompt pra outra sessão (fila se ocupada)
cp-send --pair api-fix "tarefa"   # pareia ESTA sessão com outra num grupo de trabalho
cp-send --group "terminei"        # aviso de marco pro grupo todo (unidirecional)
cp-send --new front ~/repo/front  # cria sessão nova gerenciada pelo app (visível na UI)
```

**Instalar** (uma vez por máquina; o passo 6/6 do `install.sh` também oferece):

```bash
./scripts/install-cp-send.sh
```

O installer symlinka o `cp-send` em `~/.local/bin`, adiciona o bloco "Sessões-irmãs"
no `~/.claude/CLAUDE.md` global (toda sessão Claude nova passa a conhecer a ferramenta)
e symlinka as skills do repo (`skills/*`) em `~/.claude/skills/`.

**Pareamento:** `--pair` registra um grupo no app (badge 🤝 na lista, PairSheet com a
conversa do par + contrato compartilhado em markdown) e injeta o protocolo de
colaboração em cada membro — cada sessão mexe só no próprio repo, recados 1:1 por
iniciativa própria dentro da tarefa, push/merge continuam com o usuário. Pareando
N sessões uma a uma os grupos se fundem num só.

**Skill `orquestrar`:** pra tarefa que atravessa vários repos. A sessão em que você
pedir "orquestra a tarefa X nos repos A e B" vira a **líder**: cria/pareia uma sessão
visível por repo, escreve o contrato do grupo, distribui o escopo, acompanha os
reportes de teste e consolida o painel final — você só aprova os marcos (push, MR).

**Painel/tray no desktop (só Hyprland + Quickshell):** painel flutuante de sessões
(SUPER+SHIFT+U) + ícone na bandeja. O passo 7/7 do `install.sh` oferece quando detecta
o ambiente; manual: `./scripts/install-cp-panel.sh`. Outros desktops ainda não têm painel
— use a view board/canvas do app no navegador.

## 6. Sync na nuvem (opcional)

Para sincronizar a lista de servidores entre múltiplos PCs no mesmo celular, ative o hub de sincronização na nuvem:

**Ativar:**
Defina `CP_SYNC=1` no backend. Na primeira execução, defina também `CP_SYNC_BOOTSTRAP=<secret>`:

```bash
cd backend
CP_AUTH_TOKEN=$(openssl rand -hex 24) CP_SYNC=1 CP_SYNC_BOOTSTRAP=$(openssl rand -hex 24) uv run python -m app.main
```

**Primeira vez ("Criar acesso"):**

1. Abra a PWA naquele host.
2. Aparece **"Criar acesso"** em vez de "Adicionar servidor".
3. Escolha um **nome de usuário** e uma **master password** (forte!) — cole o token bootstrap.
4. Pronto — a lista de servidores fica criptografada no hub.

**⚠ Aviso: Zero-knowledge (sem recuperação):**
- A master password **nunca sai do seu celular**.
- O hub armazena apenas salt + verificador de autenticação + ciphertext AES-GCM. **Nunca vê a senha nem os tokens dos servidores.**
- **Se esquecer a master password, os dados sincronizados ficam irrecuperáveis.**
- Não há "recuperar senha" ou reset — guarde-a bem.

**HTTPS obrigatório (produção):**
- Localmente (LAN): HTTP funciona.
- Fora de casa: o hub **deve estar em HTTPS** (Tailscale, Caddy, …). O cookie de sessão só fica seguro sob TLS.

**Padrão (desativado):**
- Sem `CP_SYNC=1`: o app funciona como antes — servidor único + token/QR, zero sincronização.

## 7. Problemas comuns

| Sintoma | Causa / fix |
|---|---|
| Recusa subir ("Refusing to start") | token ainda é `change-me` + bind não-loopback. Gere `CP_AUTH_TOKEN`. |
| 401 / "lost input" no celular | token velho/rotacionado. Re-pareie (QR) ou limpe credenciais e logue de novo. |
| App "congelou" no último estado | conexão SSE morreu calada (mobile/background). O watchdog reconecta; senão recarregue (pull-to-refresh). |
| Não vejo código novo após mudar | PWA com service worker servindo JS velho → **hard reload** / limpar dados do site / re-adicionar o PWA. |
| Backend reiniciar | precisa do cwd=`backend` (`python -m app.main` acha `app`). Sem `--reload` (trava SSE no SIGTERM). |
| Pane de sessão de motor morre na hora, sem chat nenhum | `cp-engine` não está no PATH do **servidor tmux** (a sessão nasce via `cp-engine --exec`). Garanta que o PATH usado pelo tmux enxerga `cp-engine` (mesmo instalado pelo `install-claude-wrapper.sh`). |
| Tela de Motores de modelo diz que não conseguiu ler o arquivo | `~/.claude/engines.json` foi editado à mão e ficou com JSON inválido — corrija-o (ou restaure um backup) antes de adicionar um motor novo; o app se recusa a gravar por cima de um arquivo que não conseguiu ler, pra não apagar os motores que já estavam lá. |

## 8. Segurança (resumo)

- Bind só na LAN/VPN, **nunca** interface pública; **nunca** port-forward no roteador.
- O token é a senha — trate como senha de shell. TLS na frente (Caddy/Tailscale) antes de uso real.
- Fora de casa = VPN de volta pra LAN (Tailscale/WireGuard).
