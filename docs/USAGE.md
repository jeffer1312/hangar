# Como usar o claude-pocket

Guia de uso ponta-a-ponta: subir, conectar o celular (LAN ou Tailscale), instalar
como PWA e operar o chat. Pra arquitetura/API ver o [README](../README.md).

> **Modelo:** ferramenta pessoal, single-user, **LAN/VPN-only**. Roda o `claude` **como
> você** (bypass) → um host exposto é execução-remota-como-você. A trava é o **token**.
> NUNCA faça port-forward pra internet pública. Fora de casa = **VPN (Tailscale)**.

---

## 1. Pré-requisitos

- `tmux`, `claude` (Claude Code), Python 3.14 + [`uv`](https://docs.astral.sh/uv/), Node 20+.
- Celular na **mesma rede** do PC (Wi-Fi) **ou** ambos no **mesmo tailnet** (Tailscale).

## 2. Subir (3 partes)

**a) Claude dentro do tmux** (a sessão que o app vai espelhar):
```bash
tmux new -s cc        # rode `claude` dentro dela
```
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
- **Imagem:** 📎 no composer (upload) — ou cole no terminal do Claude que o app mostra o thumbnail.
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

## 5. Problemas comuns

| Sintoma | Causa / fix |
|---|---|
| Recusa subir ("Refusing to start") | token ainda é `change-me` + bind não-loopback. Gere `CP_AUTH_TOKEN`. |
| 401 / "lost input" no celular | token velho/rotacionado. Re-pareie (QR) ou limpe credenciais e logue de novo. |
| App "congelou" no último estado | conexão SSE morreu calada (mobile/background). O watchdog reconecta; senão recarregue (pull-to-refresh). |
| Não vejo código novo após mudar | PWA com service worker servindo JS velho → **hard reload** / limpar dados do site / re-adicionar o PWA. |
| Backend reiniciar | precisa do cwd=`backend` (`python -m app.main` acha `app`). Sem `--reload` (trava SSE no SIGTERM). |

## 6. Segurança (resumo)

- Bind só na LAN/VPN, **nunca** interface pública; **nunca** port-forward no roteador.
- O token é a senha — trate como senha de shell. TLS na frente (Caddy/Tailscale) antes de uso real.
- Fora de casa = VPN de volta pra LAN (Tailscale/WireGuard).
