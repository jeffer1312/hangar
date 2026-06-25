# claude-pocket — Design

> Continuar uma sessão do Claude Code que já está rodando no PC, a partir do iPhone,
> numa UI de chat bonita, falando direto com o terminal aberto — tudo na rede local / VPN WireGuard, sem nuvem de terceiro.

- **Data**: 2026-06-25
- **Autor**: jefferson
- **Status**: aprovado; revisado pós-spike (2026-06-25)

---

## Revisão pós-spike (2026-06-25) — AUTORITATIVO

O spike (Task 1) e o feedback do usuário mudaram o design. Onde as seções abaixo divergirem, **vale esta nota** (e o plano `plans/2026-06-25-claude-pocket-backend.md`):

- **Sem feature de aprovação de permissão.** Usuário roda em bypass e não quer aprovar. Não existe `awaiting_approval`, nem botões "Sim/Não", nem rota `/approve`.
- **Estados:** `working` (`label` = texto vivo do spinner, ex. "Elucidating…") · `idle` · `awaiting_input` · `dead`.
- **Spinner real:** glyph + gerúndio + `…` (ex. `✽ Elucidating…`), detectado por glyph — NÃO por "esc to interrupt".
- **Perguntas interativas** (`ExitPlanMode`, `AskUserQuestion`, selects numerados) ocorrem mesmo no bypass → estado `awaiting_input` com `options` parseadas; app mostra botões de opção; selecionar = `(n-1)×Down`+`Enter` (validado no spike); cancelar = `Esc`. Rota `POST /select {option}`.
- Pergunta de texto livre = bolha de assistant normal, respondida no composer.

---

## 1. Objetivo

App web (PWA) no iPhone que mostra as sessões do Claude Code rodando no PC como **chat de bolhas** (mensagens + cards de tool), com um **status ao vivo** (pensando / executando / aguardando aprovação / pronto) entregue por **SSE**, e que **manda prompts e aprovações direto para o terminal aberto** (sessão `tmux` viva, usando o login claude.ai do usuário).

Não usa o remote-control oficial da Anthropic, não roteia por nuvem de terceiro, não usa API key (usa a sessão interativa logada).

## 2. Escopo

**Dentro (v1):**
- Listar / criar / matar sessões `tmux` que rodam `claude`.
- Renderizar a conversa de uma sessão como chat (bolhas usuário/assistant + cards de tool).
- Stream SSE de **mensagens novas** + **estado vivo**.
- Enviar prompt para a sessão viva (`tmux send-keys`).
- **Aprovar tool calls** (Bash/Edit) pelo celular: botões Sim/Não detectados pela caixa de permissão.
- Interromper (Esc).
- Auth por token; bind no IP local da LAN; TLS.

**Fora (v2+):**
- Streaming token-a-token do texto do assistant (JSONL é por-mensagem, não por-token).
- Push notification iOS quando `awaiting_approval` / turno termina.
- Multiusuário / RBAC; file browser; UI de git.

## 3. Restrições e ambiente

- PC: CachyOS (Arch). Python 3.14 + `uv`. Node 24 (fnm). `tmux` **a instalar** (`paru -S tmux`).
- Claude Code grava transcript append-only em `~/.claude/projects/<cwd-sanitizado>/<session-uuid>.jsonl` (confirmado: ~1900 arquivos, escrita em tempo real, 1 evento por linha).
- Celular: iPhone (Safari). `EventSource` nativo; **não** suporta header custom → auth do SSE via cookie httpOnly ou `?token=` sobre TLS.
- Rede (v1): acesso pela **LAN, pelo IP local da máquina** (`enp1s0` → `192.168.77.23`). A VPN, quando usada, dá acesso à própria LAN, então o mesmo IP local atende tanto no Wi-Fi local quanto via VPN. Sem bind em `wg0`.

## 4. Arquitetura

```
iPhone (Safari · mesma LAN ou VPN→LAN)  ── PWA Svelte ──┐
   ├ EventSource  ◄──── SSE (message | state | tool) ────┐
   └ fetch POST  ───► input / approve / interrupt         │
                          ▼                               │
Python API (FastAPI · uvicorn · bind LAN IP · TLS · Bearer)│
   ├ SessionRegistry  → tmux list/new/kill, mapeia → jsonl │
   ├ TranscriptTailer → tail <uuid>.jsonl → eventos chat ──┤
   ├ StateMonitor     → tmux capture-pane → estado vivo ───┤ merge → SSE
   └ TerminalInput    → tmux send-keys (texto/y/n/Esc) ────┘
                          ▼
   tmux: cc · web · pm …  cada sessão roda `claude` (login claude.ai)
          └─ grava JSONL ──────────────────────────────────┘
```

Princípio: **conteúdo vem do JSONL** (robusto, estruturado); **estado vivo vem de uma leitura estreita de tela** (`capture-pane`, só spinner + caixa de aprovação); **input vai por `send-keys`** para a sessão viva. As três fontes se juntam num único stream SSE por sessão.

## 5. Componentes

### Backend (Python, FastAPI + uvicorn, gerenciado por `uv`)

**5.1 `SessionRegistry`**
- O quê: fonte de verdade das sessões. Lista `tmux list-sessions`, mapeia cada sessão → `cwd` → arquivo JSONL ativo no project dir correspondente. Cria sessão (`tmux new -d -s <name> -c <cwd> 'claude --session-id <uuid>'`) e mata (`tmux kill-session`).
- Interface: `list() -> [SessionInfo]`, `create(name, cwd) -> SessionInfo`, `kill(name)`, `resolve_jsonl(name) -> path`.
- Depende de: binário `tmux`, filesystem `~/.claude/projects`.
- Nota de mapeamento: para sessões criadas pelo app, forçamos `--session-id <uuid>` → sabemos o arquivo exato. Para sessões pré-existentes (terminal já aberto), mapeia por `cwd` → project dir → JSONL mais recente em escrita.

**5.2 `TranscriptTailer`** (1 por sessão observada)
- O quê: segue o JSONL (inotify via `watchfiles`, fallback poll), parseia cada linha em um `ChatEvent` normalizado e publica num pub/sub interno.
- Mapeamento JSONL → chat:
  - `type=user`, content texto → **bolha do usuário**.
  - `type=assistant`, `content[].text` → **bolha assistant**.
  - `type=assistant`, `content[].tool_use` → **card de tool** (name, input) chaveado por `tool_use_id`.
  - `type=user`, `content[].tool_result` → preenche o card correspondente (resultado/erro), **não** vira bolha.
  - `attachment` → ignora (v1).
  - Threading por `uuid`/`parentUuid`.
- Interface: `subscribe() -> async iterator[ChatEvent]`, `history() -> [ChatEvent]`.
- Depende de: arquivo JSONL; `watchfiles`.

**5.3 `StateMonitor`** (1 por sessão observada)
- O quê: deriva o **estado vivo**. Poll `tmux capture-pane -p -t <name>` (~750 ms) + dicas do Tailer.
- Classificação:
  - **awaiting_approval**: caixa de permissão na tela (`Do you want to proceed?`, `❯ 1. Yes`, `2. No`, variantes "don't ask again"). Parseia as opções.
  - **executing:`<tool>`**: spinner ativo (`esc to interrupt`/contador) **e** último evento do Tailer é `tool_use` sem `tool_result`.
  - **thinking**: spinner ativo sem tool pendente.
  - **idle/pronto**: sem spinner, input vazio visível.
- Interface: `subscribe() -> async iterator[StateEvent]`, `current() -> State`.
- Depende de: `tmux capture-pane`; padrões da TUI do Claude (**risco — validar no spike**).
- Degradação: padrão desconhecido → mantém último estado, loga raw, nunca derruba o chat.

**5.4 `TerminalInput`**
- O quê: injeta na sessão viva. Prompt: `tmux send-keys -t <name> -l -- "<texto>"` **depois** `tmux send-keys -t <name> Enter` (sempre dois comandos). Aprovação: envia a tecla da opção (`y`/`n` ou seta+Enter conforme a caixa). Interrupt: `send-keys Escape`.
- Interface: `send_prompt(name, text)`, `approve(name, choice)`, `interrupt(name)`.
- Segurança: sanitiza o corpo; só modo literal (`-l`) + Enter separado; nada de shell arbitrário.

**5.5 API HTTP + SSE** (camada FastAPI)
- `GET /api/sessions` → lista com estado atual.
- `POST /api/sessions` `{name, cwd}` → cria.
- `DELETE /api/sessions/{name}` → mata.
- `GET /api/sessions/{name}/history` → transcript parseado (load inicial do chat).
- `GET /api/sessions/{name}/events` (**SSE**) → `event: message|state|tool` + `data: {...}`. Suporta `Last-Event-ID` para reidratar no reconnect.
- `POST /api/sessions/{name}/input` `{text}`.
- `POST /api/sessions/{name}/approve` `{choice}`.
- `POST /api/sessions/{name}/interrupt`.
- Auth: `Authorization: Bearer <token>` no REST; SSE via cookie httpOnly setado no login (ou `?token=` sobre TLS).

**5.6 Auth + Rede**
- **Serving same-origin**: o Caddy serve o build estático do Svelte **e** faz proxy de `/api` + `/sse` no mesmo origin. Assim o cookie de auth flui pro SSE sem header custom (que o `EventSource` não suporta).
- Auth: token longo aleatório (config/env). Login simples → **cookie httpOnly + Secure + SameSite** (primário, usado pelo SSE). `?token=` só como fallback de debug (evitar — vaza em log).
- `uvicorn`/Caddy faz bind **no IP da LAN** (`LAN_BIND_IP`, ex. `192.168.77.23` / `enp1s0`), nunca em interface pública. Recomendado fixar o IP (reserva DHCP/estático) pra não quebrar o bind. Alternativa: bind `0.0.0.0` **com** firewall restrito à subnet local.
- TLS: cert para o IP/nome local (Caddy `tls internal` ou self-signed) — instalar root/perfil no iPhone uma vez.
- Firewall: `ufw default deny incoming` + liberar a porta só da subnet local (`ufw allow from 192.168.77.0/24 to any port <porta>`). **Nunca** port-forward no roteador.

### Frontend (Svelte + Vite, PWA)

- **Tela Sessões**: lista (nome, cwd, estado pill, última atividade), criar/matar.
- **Tela Chat**: bolhas (do `/history` + SSE `message`), cards de tool (status do `tool_result`), **status pill** no topo (do SSE `state`), **botões Sim/Não** quando `awaiting_approval`, **composer** de prompt (POST input), botão **interromper** (Esc).
- SSE via `EventSource`; POST via `fetch`. Reconnect automático do EventSource + refetch `/history` na volta.
- PWA: manifest + ícone, "Add to Home Screen" no iOS.

## 6. Fluxo de dados

- **Saída (Claude → celular)**: `claude` grava JSONL → `TranscriptTailer` parseia → SSE `message`/`tool`. Em paralelo `StateMonitor` lê `capture-pane` → SSE `state`. App renderiza.
- **Entrada (celular → Claude)**: composer → `POST /input` → `TerminalInput.send_prompt` → `send-keys` na sessão viva → Claude processa → (volta pela saída).
- **Aprovação**: Claude mostra caixa → `StateMonitor` detecta → SSE `state=awaiting_approval` → app mostra Sim/Não → `POST /approve` → `send-keys` da opção.

## 7. Máquina de estados (por sessão)

```
            POST /input
  idle ───────────────────► thinking ──(tool_use)──► executing:<tool>
   ▲                           │                          │
   │ (assistant final, prompt) │ (caixa permissão)        │ (caixa permissão)
   │                           ▼                          ▼
   └──────────────────── awaiting_approval ◄──────────────┘
                               │ POST /approve (yes→segue · no→volta idle)
   dead ◄── tmux kill / sessão encerrada (qualquer estado)
```

## 8. Tratamento de erros

- Sessão tmux morre → Registry detecta no próximo list → SSE `state=dead` → UI "sessão encerrada".
- JSONL rotaciona / novo session-id → Registry remapeia, Tailer reabre.
- `capture-pane` padrão desconhecido → mantém último estado, loga raw; chat segue (conteúdo é do JSONL).
- SSE cai → EventSource reconecta; server reidrata via `Last-Event-ID` + app refaz `/history`.
- `send-keys` em sessão morta → 410 ao cliente.
- Input com control chars → rejeita; interrupt só pelo endpoint dedicado.

## 9. Estratégia de testes

- **Spikes a validar PRIMEIRO** (de-risca as assunções):
  1. `tmux send-keys -l -- "..."` + Enter submete prompt num `claude` vivo dentro do tmux.
  2. `capture-pane` mostra spinner e a caixa de aprovação → coletar amostras reais → virar fixtures do classifier.
- **Unit**: parser JSONL (fixtures de `~/.claude` reais), classifier de estado (fixtures de capture-pane), montador de `send-keys`.
- **Integração**: Registry contra tmux real (criar/listar/matar); Tailer contra JSONL real.
- **E2E**: do iPhone via WireGuard — criar sessão, mandar prompt, ver bolhas + estado, aprovar um Bash, interromper.

## 10. Assunções a validar (resumo)

- (A) `send-keys -l` submete no input do Claude. → Spike 1.
- (B) `capture-pane` expõe spinner + caixa de aprovação de forma estável. → Spike 2.
- (C) JSONL é escrito por-evento em tempo real suficiente para chat fluido. → **confirmado** na sondagem.
- (D) Bind no IP local (`192.168.77.23`/`enp1s0`) — **confirmado** disponível. Fixar IP (reserva DHCP) recomendado.

## 11. Estrutura de pastas (proposta)

```
claude-pocket/
  backend/        # FastAPI + uv (pyproject.toml)
    app/{registry,tailer,state,input,api,auth}.py
    tests/
  frontend/       # Svelte + Vite (PWA)
    src/{routes,lib}/...
  docs/superpowers/specs/2026-06-25-claude-pocket-design.md
  README.md
```
