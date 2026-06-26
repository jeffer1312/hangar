# Design — Composer: teclado iOS, fila de input, badge migrando, sticky-scroll

**Data:** 2026-06-25
**Escopo:** 4 ajustes coesos no chat mobile (frontend Svelte). App continua sendo
chat renderizado do transcript (bubbles), **não** mirror do terminal.

## Problema

No celular, o chat tem 4 incômodos vivos:

1. **Teclado iOS** empurra o composer pra cima demais (PWA, Safari, Chrome) —
   deveria ficar fixo logo acima do teclado.
2. **Não dá pra digitar enquanto o Claude pensa** (textarea `disabled`), sem fila.
3. **O badge de atividade** ("Trabalhando… · tempo · atividade") vive dentro do
   composer; o usuário quer ele no **fim da lista**, como última "msg" até a
   resposta real chegar.
4. **Scroll-up durante o pensamento é arrastado de volta pro fim** toda hora.

## Diagnósticos (confirmados no código)

- **iOS keyboard** — `Chat.svelte:123` faz `translateY(-(innerHeight - vv.height))`,
  ignora `vv.offsetTop` e não escuta `scroll`. No iOS, focar a textarea perto do
  fundo faz o Safari rolar a página inteira pra revelar o input; somado ao
  translateY = deslocamento duplo (voa pra cima).
- **Fila** — `Composer.svelte:247` `disabled={isWorking}` e `canSend` exige
  `!isWorking`. Mandar input no tmux enquanto o Claude trabalha → Claude Code
  **enfileira nativamente**; logo o passthrough já funciona no backend.
- **Badge** — o `status-row` (dot + label + `LiveMetrics` tempo/custo) está em
  `Composer.svelte:227-237`. Cronômetro (`elapsedLabel`) e timer vivem no script
  do Composer.
- **Auto-scroll** — `MessageList.svelte:37-42` tem `$effect` que referencia
  `void stateEvent` e `void events.length` e chama `scrollToBottom()` sempre. O
  backend emite state events de fora (StateMonitor) + status_line atualiza →
  scroll forçado contra o usuário. **É SSE, não polling** (`connectSSE`/
  `openEventStream`); não há migração a fazer.

## Solução

### 1. Teclado iOS (PWA + Safari + Chrome)

- **Travar o documento**: root do app `100dvh` + `overflow:hidden`; único scroller
  = a `MessageList` (já é). Mata o auto-scroll-into-view do iOS que sobe a página.
- **Corrigir o handler do visualViewport** (`Chat.svelte`):
  - `inset = window.innerHeight - vv.height - vv.offsetTop` (com `Math.max(0, …)`).
  - escutar `vv.resize` **e** `vv.scroll`.
  - chamar `update()` também no `focus` da textarea (o resize às vezes atrasa).
- **`interactive-widget=resizes-content`** no meta viewport (`index.html`): no
  Android/Chrome o layout encolhe e o `fixed bottom:0` ancora nativo (formula dá
  ≈0, sem duplo); inerte no iOS.
- **Verificar no aparelho** (iOS é teimoso) — stack + celular disponíveis.

### 2. Fila (bubble pendente)

- Textarea **sempre habilitada** (remover `disabled={isWorking}`; `canSend` deixa
  de exigir `!isWorking`). Exceções: estado `dead` (composer já é trocado pelo
  dead-footer) e `awaiting_input` (composer some, OptionButtons assume).
- Ao enviar durante `working`:
  - adiciona um **pending** local (id temporário, texto, ts) a uma lista de fila.
  - manda pro backend na hora (`sendInput`) — Claude enfileira no tmux.
  - **renderiza bubble pendente** (estilo cinza/opaco) no fim da lista.
  - **solidificação/dedup**: quando o transcript (SSE) trouxer o `user_msg` real
    com texto igual e ainda não casado, remove o pending correspondente (match por
    texto, 1-a-1, em ordem). Garante que não duplique nem suma cedo demais.
- **Stop migra pro badge** (ver item 3): o composer passa a mostrar **sempre o
  botão enviar**; interromper o Claude fica no badge de atividade (onde faz
  sentido). `onInterrupt` continua o mesmo handler, só muda de lugar.

### 3. Badge de atividade no fim da lista

- Novo componente `ActivityBadge.svelte`, renderizado como **último item da
  `MessageList`** quando `state === 'working'` (e podendo cobrir `awaiting_input`
  no lugar do OptionButtons? — não: awaiting continua com OptionButtons).
- Conteúdo **idêntico ao atual**: dot pulsante + label de atividade
  (`stateEvent.label` ex "Pollinating…") + cronômetro vivo (mm:ss) + custo. Move o
  cronômetro/timer pra cá (ou pra Chat, passando via prop).
- Botão **Stop** no badge (substitui o stop-btn do composer).
- **Composer mantém**: `ContextRing` (ctx %), pill modelo·effort, botão `/`. Só o
  `status-row` (dot/label/tempo/custo) migra.

### 4. Auto-scroll sticky-bottom (`MessageList.svelte`)

- Rastrear `atBottom` no evento `scroll` da lista (threshold ~64px do fim).
- O `$effect` só chama `scrollToBottom()` **se `atBottom`**. Scrollou pra cima →
  para de arrastar.
- Remover a dependência de `stateEvent` do efeito (tick do cronômetro/state não
  deve rolar). Manter dependência de `events.length` (mensagem nova, gated por
  atBottom).
- Opcional (nice-to-have, pode ficar pra depois): pílula "↓ recentes" quando há
  conteúdo novo e o usuário está scrollado pra cima.

## Arquivos

- `frontend/index.html` — meta viewport (`interactive-widget=resizes-content`).
- `frontend/src/app.css` — lock do root (100dvh + overflow hidden).
- `frontend/src/screens/Chat.svelte` — handler visualViewport corrigido; remove o
  consumo do status-row do dock; passa `label`/state pro badge na lista.
- `frontend/src/components/Composer.svelte` — tira `disabled`/`canSend !isWorking`;
  remove `status-row`; remove stop-btn (sempre enviar); fila de pending.
- `frontend/src/components/ActivityBadge.svelte` — **novo**; badge no fim da lista
  com dot+label+timer+custo+stop.
- `frontend/src/components/MessageList.svelte` — sticky-bottom; renderiza
  `ActivityBadge` (working) + bubbles pendentes.
- (timer/cronômetro: mover de Composer pra ActivityBadge ou Chat.)

## Testes / critério de sucesso

- **iOS PWA + Safari + Chrome**: abrir teclado → composer fixo logo acima dele,
  sem voar; fechar → volta ao fundo. (verificação manual no aparelho).
- **Android Chrome**: idem.
- **Fila**: digitar e enviar durante working → bubble pendente aparece na hora;
  Claude processa; pending solidifica sem duplicar.
- **Badge**: durante working, atividade+tempo+custo aparecem como último item da
  lista (não no composer); Stop interrompe.
- **Sticky-scroll**: scrollar pra cima durante working **não** é mais arrastado;
  no fim, novas msgs continuam colando no fundo.
- Build limpo (`npm run build`), testes backend seguem passando (sem mudança de
  backend prevista).

## Fora de escopo

- Persistir tema do tmux (`window-style bg=default`) — separado, já aplicado live.
- Slice 1B (métricas via `message.usage`), Phase 4 (chips 5h/7d), backlog
  (agents view, anexos) — depois.
