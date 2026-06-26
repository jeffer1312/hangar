# Composer: teclado iOS, fila, badge migrando, sticky-scroll — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Corrigir 4 incômodos do chat mobile — teclado iOS empurrando o composer, falta de fila ao digitar durante o pensamento, badge de atividade preso no composer, e auto-scroll que arrasta pro fim durante o working.

**Architecture:** Frontend Svelte 5 (runes). O app é chat renderizado do transcript (bubbles), não mirror de terminal. A fila e a dedup centralizam em `Chat.svelte` (dono do SSE + eventos); o composer fica "burro" (sempre habilitado, só emite `onSend`). O badge de atividade vira componente próprio renderizado no fim da `MessageList`. O backend não muda.

**Tech Stack:** Svelte 5, TypeScript, Vite. `visualViewport` API para o teclado. Sem harness de teste no frontend — verificação por `npm run check` (svelte-check), `npm run build`, e checagem manual no aparelho (iOS PWA/Safari/Chrome + Android Chrome). Backend tem testes próprios (não tocados aqui).

**Convenção de verificação:** como não há vitest no frontend, cada task usa `npm run check` + `npm run build` como gate automático e uma checagem manual quando aplicável. Rodar comandos com caminho absoluto a partir de `frontend/` (ex: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check`) — não mudar o cwd.

**Git:** o usuário autoriza commits nesta branch (`main`, projeto pessoal) por task. NÃO usar trailer `Co-Authored-By`. Mensagens em inglês, formato conventional.

---

### Task 1: Viewport meta + lock do documento (anti page-scroll iOS)

**Files:**
- Modify: `frontend/index.html:5`
- Modify: `frontend/src/app.css` (regras `html`, `body`, `#app`)

- [ ] **Step 1: Adicionar `interactive-widget=resizes-content` ao meta viewport**

Em `frontend/index.html`, trocar a linha 5:

```html
<meta name="viewport" content="width=device-width, initial-scale=1, maximum-scale=1, viewport-fit=cover, interactive-widget=resizes-content" />
```

- [ ] **Step 2: Endurecer o lock do root pra matar o page-scroll do iOS**

Em `frontend/src/app.css`, substituir os blocos `html`, `body` e `#app` existentes por:

```css
html {
  height: 100%;
  width: 100%;
}

body {
  height: 100dvh;          /* dvh: acompanha a viewport dinâmica */
  width: 100%;
  overflow: hidden;
  overscroll-behavior: none; /* sem rubber-band que rola a página no iOS */
  font-family: var(--font-ui);
  font-size: var(--text-base);
  line-height: 1.55;
  color: var(--text-primary);
  background: var(--bg-base);
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
}

#app {
  height: 100dvh;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
```

- [ ] **Step 3: Verificar build/type-check**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: ambos PASS, sem erro.

- [ ] **Step 4: Commit**

```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/index.html frontend/src/app.css
git -C /home/jefferson/pessoal/claude-pocket commit -m "fix(mobile): lock document + interactive-widget to tame iOS keyboard"
```

---

### Task 2: Corrigir o handler do visualViewport (Chat.svelte)

**Files:**
- Modify: `frontend/src/screens/Chat.svelte:122-134` (o `$effect` do teclado)
- Modify: `frontend/src/components/Composer.svelte` (expor o nó da textarea? — NÃO; ver Step 1, usamos `focusin` no dock)

- [ ] **Step 1: Substituir o `$effect` de lift do dock**

Em `frontend/src/screens/Chat.svelte`, trocar o bloco atual (linhas 122-134):

```svelte
  // Lift the bottom dock (statusline + composer) above the iOS on-screen keyboard.
  $effect(() => {
    if (!dockEl) return;
    const vv = window.visualViewport;
    if (!vv) return;
    function onResize() {
      if (!dockEl || !vv) return;
      const kb = window.innerHeight - vv.height;
      dockEl.style.transform = `translateY(-${Math.max(0, kb)}px)`;
    }
    vv.addEventListener('resize', onResize);
    return () => vv.removeEventListener('resize', onResize);
  });
```

por:

```svelte
  // Lift the bottom dock (composer) above the on-screen keyboard, cross-platform.
  // iOS: window.innerHeight nao encolhe; o teclado vive na diferenca p/ visualViewport,
  // e o Safari ainda ROLA a viewport (offsetTop > 0) ao focar perto do fundo — por isso
  // o inset desconta offsetTop e escutamos 'scroll' alem de 'resize'. Android/Chrome com
  // interactive-widget=resizes-content encolhe o layout -> inset ~0 -> no-op (sem duplo).
  $effect(() => {
    if (!dockEl) return;
    const vv = window.visualViewport;
    if (!vv) return;
    function update() {
      if (!dockEl || !vv) return;
      const inset = window.innerHeight - vv.height - vv.offsetTop;
      dockEl.style.transform = `translateY(-${Math.max(0, inset)}px)`;
    }
    vv.addEventListener('resize', update);
    vv.addEventListener('scroll', update);
    // Foco na textarea pode preceder o resize do teclado: forca um update no proximo frame.
    function onFocusIn() {
      requestAnimationFrame(update);
      // segundo tick: iOS as vezes so estabiliza apos a animacao do teclado
      setTimeout(update, 300);
    }
    dockEl.addEventListener('focusin', onFocusIn);
    update();
    return () => {
      vv.removeEventListener('resize', update);
      vv.removeEventListener('scroll', update);
      dockEl?.removeEventListener('focusin', onFocusIn);
    };
  });
```

- [ ] **Step 2: Verificar build/type-check**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/screens/Chat.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "fix(mobile): anchor composer dock to visualViewport (offsetTop + scroll + focus)"
```

- [ ] **Step 4: Verificação manual (aparelho) — pode rodar no fim, junto das outras**

Abrir no celular (token novo já pareado). Tocar a textarea: o composer deve parar logo acima do teclado, sem voar. Testar iOS PWA, iOS Safari, iOS Chrome, Android Chrome.

---

### Task 3: Auto-scroll sticky-bottom (MessageList.svelte)

**Files:**
- Modify: `frontend/src/components/MessageList.svelte:18` (estado), `36-48` (efeito + scroll), `51-55` (listener no `<section>`)

- [ ] **Step 1: Trocar o estado + efeito de auto-scroll**

Em `frontend/src/components/MessageList.svelte`, trocar a declaração de `listEl` (linha 18) e o bloco do efeito (linhas 36-48) por:

```svelte
  let listEl: HTMLElement | undefined = $state();
  // O usuario "gruda" no fim por padrao; ao rolar pra cima, paramos de arrastar.
  let atBottom = $state(true);

  function onScroll() {
    if (!listEl) return;
    const gap = listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight;
    atBottom = gap < 64; // threshold ~64px do fim
  }

  // Auto-scroll APENAS quando ja estamos no fim. NAO depende de stateEvent (o tick do
  // cronometro/status atualiza stateEvent toda hora e arrastaria o scroll-up do usuario).
  $effect(() => {
    void events.length;
    void pending.length;
    if (!atBottom) return;
    tick().then(scrollToBottom);
  });

  function scrollToBottom() {
    if (listEl) {
      listEl.scrollTop = listEl.scrollHeight;
    }
  }
```

- [ ] **Step 2: Declarar a prop `pending` (placeholder até a Task 6)**

Ainda em `MessageList.svelte`, na interface `Props` (linhas 9-14) e no `$props()` (linha 16), adicionar `pending`:

```svelte
  interface Props {
    events: ChatEvent[];
    stateEvent: StateEvent | null;
    pending: { id: string; text: string }[];
    onSelectOption: (i: number) => void;
    onCancel: () => void;
  }

  let { events, stateEvent, pending, onSelectOption, onCancel }: Props = $props();
```

- [ ] **Step 3: Ligar o listener de scroll no `<section>`**

Trocar a tag de abertura (linhas 51-55) por:

```svelte
<section
  class="message-list"
  bind:this={listEl}
  onscroll={onScroll}
  aria-label="Mensagens"
>
```

- [ ] **Step 4: Passar `pending` a partir do Chat (provisório vazio)**

Em `frontend/src/screens/Chat.svelte`, no uso de `<MessageList>` (linhas 184-189), adicionar `pending`. Antes da Task 6 ainda não existe o estado, então declarar já um array vazio reativo logo após `let dockEl` (linha 32):

```svelte
  let pending = $state<{ id: string; text: string }[]>([]);
```

e no markup:

```svelte
    <MessageList
      {events}
      {stateEvent}
      {pending}
      onSelectOption={handleSelect}
      onCancel={handleInterrupt}
    />
```

- [ ] **Step 5: Verificar build/type-check**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/components/MessageList.svelte frontend/src/screens/Chat.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "fix(chat): sticky-bottom auto-scroll, stop yanking on scroll-up"
```

---

### Task 4: Componente ActivityBadge + render no fim da lista

**Files:**
- Create: `frontend/src/components/ActivityBadge.svelte`
- Modify: `frontend/src/components/MessageList.svelte` (import + render + prop `costUsd`)
- Modify: `frontend/src/screens/Chat.svelte` (passar `costUsd`)

- [ ] **Step 1: Criar `ActivityBadge.svelte`**

Conteúdo idêntico ao status-row atual (dot pulsante + label de atividade + cronômetro + custo) + botão Stop. O cronômetro (movido do Composer) começa ao montar (o badge só monta em `working`).

Create `frontend/src/components/ActivityBadge.svelte`:

```svelte
<script lang="ts">
  import { onDestroy } from 'svelte';
  import LiveMetrics from './LiveMetrics.svelte';
  import IconInterrupt from './icons/IconInterrupt.svelte';

  interface Props {
    label?: string | null;     // atividade do Claude (ex "Pollinating…")
    costUsd?: number | null;
    onCancel: () => void;
  }
  let { label = null, costUsd = null, onCancel }: Props = $props();

  const stateLabel = $derived(label ?? 'Trabalhando…');

  // Cronometro local: conta a partir do mount (o badge so existe em working). Aproximacao
  // client-side (reseta no reconnect do SSE) — ancora de liveness, nao tempo autoritativo.
  let elapsedLabel = $state('00:00');
  let startedAt = Date.now();
  function fmtElapsed(ms: number): string {
    const total = Math.max(0, Math.floor(ms / 1000));
    const mm = Math.floor(total / 60);
    const ss = total % 60;
    return String(mm).padStart(2, '0') + ':' + String(ss).padStart(2, '0');
  }
  const timer = setInterval(() => {
    elapsedLabel = fmtElapsed(Date.now() - startedAt);
  }, 1000);
  onDestroy(() => clearInterval(timer));
</script>

<div class="activity-badge" role="status" aria-live="polite">
  <div class="badge-left">
    <span class="dot" aria-hidden="true"></span>
    <span class="state-label">{stateLabel}</span>
    <LiveMetrics timeLabel={elapsedLabel} {costUsd} />
  </div>
  <button class="stop-btn" onclick={onCancel} aria-label="Interromper Claude">
    <IconInterrupt size={16} />
  </button>
</div>

<style>
  .activity-badge {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
    margin: var(--space-2) 0;
    padding: var(--space-2) var(--space-3);
    background: var(--bg-elevated);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-md);
    animation: bubble-in 200ms var(--ease-out);
  }

  .badge-left {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
  }

  .dot {
    width: 7px;
    height: 7px;
    border-radius: var(--radius-full);
    flex-shrink: 0;
    background: var(--pill-working-fg);
    animation: pulse-scale 1.4s ease-in-out infinite;
  }

  .state-label {
    font-size: var(--text-sm);
    font-weight: 500;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .stop-btn {
    width: 36px;
    height: 36px;
    min-width: 36px;
    min-height: 36px;
    flex-shrink: 0;
    background: transparent;
    border: 1px solid var(--error);
    border-radius: var(--radius-md);
    color: var(--error);
    transition: background 180ms var(--ease-out);
  }

  .stop-btn:active {
    background: rgba(255, 69, 58, 0.08);
  }
</style>
```

- [ ] **Step 2: Renderizar o badge no fim da MessageList (durante working)**

Em `frontend/src/components/MessageList.svelte`, adicionar o import (junto aos outros, ~linha 7):

```svelte
  import ActivityBadge from './ActivityBadge.svelte';
```

Adicionar `costUsd` à interface `Props` e ao `$props()`:

```svelte
  interface Props {
    events: ChatEvent[];
    stateEvent: StateEvent | null;
    pending: { id: string; text: string }[];
    costUsd?: number | null;
    onSelectOption: (i: number) => void;
    onCancel: () => void;
  }

  let { events, stateEvent, pending, costUsd = null, onSelectOption, onCancel }: Props = $props();
```

No markup, logo após o `{/each}` dos `visibleEvents` (linha 65) e antes do bloco `awaiting_input`:

```svelte
    {#if stateEvent?.state === 'working'}
      <ActivityBadge label={stateEvent.label} {costUsd} onCancel={onCancel} />
    {/if}
```

- [ ] **Step 3: Passar `costUsd` do Chat**

Em `frontend/src/screens/Chat.svelte`, no `<MessageList>`, adicionar `costUsd={status?.costUsd}`:

```svelte
    <MessageList
      {events}
      {stateEvent}
      {pending}
      costUsd={status?.costUsd}
      onSelectOption={handleSelect}
      onCancel={handleInterrupt}
    />
```

- [ ] **Step 4: Verificar build/type-check**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: PASS. (Neste commit o badge aparece na lista E o composer ainda mostra o status-row — duplicação transitória, removida na Task 5. App segue funcional.)

- [ ] **Step 5: Commit**

```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/components/ActivityBadge.svelte frontend/src/components/MessageList.svelte frontend/src/screens/Chat.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(chat): ActivityBadge at end of message list (working state)"
```

---

### Task 5: Composer vira input puro (sem status-row, sem disable, sempre enviar)

**Files:**
- Modify: `frontend/src/components/Composer.svelte` (imports, props, estado, markup, styles)
- Modify: `frontend/src/screens/Chat.svelte` (remover props `label`/`onInterrupt` do `<Composer>`)

- [ ] **Step 1: Remover imports e props não usados**

Em `frontend/src/components/Composer.svelte`:

- Remover o import de `LiveMetrics` (linha 13) e de `IconInterrupt` (linha 11).
- Na interface `Props` (linhas 21-29) e no `$props()` (linha 30), remover `label`, `onInterrupt` (o stop migrou pro badge):

```svelte
  interface Props {
    sessionName: string;
    sessionState: State;
    status: StatusFields | null;
    onSend: (text: string) => void;
    onCommand: (cmd: string) => void;
  }
  let { sessionName, sessionState, status, onSend, onCommand }: Props = $props();
```

- [ ] **Step 2: Remover o cronômetro e os derivados de status-row**

Remover os blocos (já migrados pro ActivityBadge):
- `stateLabels` + `stateLabel` (linhas 60-69)
- o cronômetro: `elapsedLabel`, `timer`, `startedAt`, `fmtElapsed`, `stopTimer`, o `$effect` do timer, `onDestroy(() => stopTimer())`, `timeLabel`, `hasMetrics`, `showStatusRow` (linhas 71-117)

Trocar `isWorking`/`canSend` (linhas 57-58) por (a textarea deixa de bloquear em working):

```svelte
  const canSend = $derived(inputText.trim().length > 0);
```

(Manter o import de `onDestroy`/`tick` da linha 9? `tick` ainda é usado em `fillCommand`. `onDestroy` deixa de ser usado — remover de `import { onDestroy, tick } from 'svelte';`, deixando `import { tick } from 'svelte';`.)

- [ ] **Step 3: Remover o markup do status-row**

Remover o bloco (linhas 227-237):

```svelte
    {#if showStatusRow}
      <div class="status-row">
        <div class="status-left">
          <span class="dot dot--{sessionState}" aria-hidden="true"></span>
          {#if stateLabel}
            <span class="state-label" role="status" aria-live="polite">{stateLabel}</span>
          {/if}
        </div>
        <LiveMetrics {timeLabel} costUsd={status?.costUsd} />
      </div>
    {/if}
```

- [ ] **Step 4: Textarea sempre habilitada**

Na `<textarea>` (linhas 241-251), remover `disabled={isWorking}`.

- [ ] **Step 5: Control-right sempre mostra Enviar (sem morph pra Stop)**

Trocar o bloco `control-right` (linhas 272-288) por:

```svelte
      <div class="control-right">
        <button
          class="send-btn"
          class:send-btn--disabled={!canSend}
          onclick={submit}
          disabled={!canSend}
          aria-label="Enviar mensagem"
        >
          <IconSend size={18} />
        </button>
      </div>
```

- [ ] **Step 6: Limpar CSS órfão**

Remover do `<style>` as regras que ficaram sem uso: `.status-row`, `.status-left`, `.state-label`, `.dot`, `.dot--working`, `.dot--idle`, `.dot--awaiting_input`, `.dot--dead` (linhas ~329-377) e `.stop-btn` + `.stop-btn:active` (linhas ~487-501). Manter `.send-btn`, `.send-btn--disabled`, `.model-pill`, `.slash-btn`, `.composer-textarea`, etc.

- [ ] **Step 7: Atualizar o uso do Composer no Chat**

Em `frontend/src/screens/Chat.svelte`, no `<Composer>` (linhas 199-207), remover `label` e `onInterrupt`:

```svelte
      <Composer
        {sessionName}
        sessionState={currentState}
        status={status}
        onSend={handleSend}
        onCommand={handleCommand}
      />
```

- [ ] **Step 8: Verificar build/type-check**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: PASS, sem warning de variável/CSS não usado.

- [ ] **Step 9: Commit**

```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/components/Composer.svelte frontend/src/screens/Chat.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(composer): pure input — always enabled, status/stop moved to badge"
```

---

### Task 6: Fila — pending bubbles + dedup contra o transcript

**Files:**
- Modify: `frontend/src/screens/Chat.svelte` (estado `pending` já existe; lógica de push + dedup em `handleSend`/efeito)
- Modify: `frontend/src/components/MessageList.svelte` (render dos pending bubbles)

- [ ] **Step 1: Push do pending ao enviar durante working + dedup**

Em `frontend/src/screens/Chat.svelte`, trocar `handleSend` (linhas 136-142) por (mantém o `pending` declarado na Task 3):

```svelte
  let pendingSeq = 0;

  async function handleSend(text: string) {
    // Enviou enquanto o Claude trabalha -> entra na fila (Claude Code enfileira no tmux).
    // Eco imediato como bubble pendente; solidifica quando o transcript trouxer a msg real.
    let pendingId: string | null = null;
    if (currentState === 'working') {
      pendingId = `pending-${pendingSeq++}`;
      pending = [...pending, { id: pendingId, text }];
    }
    try {
      await sendInput(sessionName, text);
    } catch (err) {
      console.error('sendInput error:', err);
      // Falhou o envio -> remove o pending que adicionamos (nao ficou enfileirado).
      if (pendingId) pending = pending.filter((p) => p.id !== pendingId);
    }
  }

  // Dedup: quando o transcript (SSE) traz o user_msg real, solta o pending de mesmo texto.
  // Idempotente -> nao entra em loop (apos filtrar, o length estabiliza e nao reatribui).
  $effect(() => {
    if (pending.length === 0) return;
    const committed = new Set(
      events.filter((e) => e.kind === 'user_msg' && e.text).map((e) => e.text)
    );
    const next = pending.filter((p) => !committed.has(p.text));
    if (next.length !== pending.length) pending = next;
  });
```

- [ ] **Step 2: Renderizar os pending bubbles na MessageList**

Em `frontend/src/components/MessageList.svelte`, logo após o bloco do `ActivityBadge` (Task 4 Step 2) e antes do `awaiting_input`:

```svelte
    {#each pending as p (p.id)}
      <div class="pending-bubble">
        <UserBubble text={p.text} ts={undefined} />
      </div>
    {/each}
```

E adicionar ao `<style>` da MessageList:

```svelte
  /* Bubble enfileirado: ainda nao processado pelo Claude — atenuado ate solidificar. */
  .pending-bubble {
    opacity: 0.5;
  }
```

- [ ] **Step 3: Confirmar que UserBubble aceita `ts` opcional**

Verificar a assinatura de `frontend/src/components/UserBubble.svelte`. Se `ts` for obrigatório, torná-lo opcional:

Run: `grep -n "ts" /home/jefferson/pessoal/claude-pocket/frontend/src/components/UserBubble.svelte`
Se a prop `ts` não tiver default, ajustar para `ts = undefined` / tipo `number | undefined` na interface de Props do UserBubble (mudança mínima, sem alterar o render quando `ts` falta).

- [ ] **Step 4: Verificar build/type-check**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/screens/Chat.svelte frontend/src/components/MessageList.svelte frontend/src/components/UserBubble.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(chat): queue input while working — pending bubbles with transcript dedup"
```

---

### Task 7: Verificação manual no aparelho (gate final)

**Files:** nenhum (teste manual).

- [ ] **Step 1: Subir/garantir o stack**

Backend (:8765), frontend (:5173), tailscale serve já rodando (ver handoff). Se o backend caiu, relançar com token e re-parear (QR). Rebuild do frontend não é necessário em dev (vite HMR), mas conferir que o vite pegou as mudanças.

- [ ] **Step 2: Teclado (iOS PWA, iOS Safari, iOS Chrome, Android Chrome)**

Tocar a textarea → composer fixo logo acima do teclado, sem voar pra cima. Fechar teclado → volta ao fundo. Repetir nos 4 contextos.

- [ ] **Step 3: Fila**

Com o Claude trabalhando, digitar e enviar → bubble pendente (atenuado) aparece na hora no fim da lista. Quando o Claude processa, o pending solidifica (vira bubble normal) sem duplicar.

- [ ] **Step 4: Badge**

Durante working, a atividade (dot + "Pollinating…/Trabalhando…" + cronômetro + custo) aparece como último item da lista (não no composer). Botão Stop interrompe o Claude.

- [ ] **Step 5: Sticky-scroll**

Durante working, rolar pra cima → NÃO é mais arrastado pro fim. No fim da lista, novas mensagens continuam colando no fundo.

- [ ] **Step 6: Se tudo ok, atualizar o handoff**

`/handoff save` registrando o que foi entregue e o que falta (Slice 1B, Phase 4, backlog).

---

## Notas de cobertura (self-review)

- **Item 1 (teclado iOS)** → Tasks 1 + 2 (lock + handler) + verificação Task 7 Step 2.
- **Item 2 (fila)** → Tasks 5 (composer sempre habilitado/enviar) + 6 (pending + dedup).
- **Item 3 (badge migrando)** → Tasks 4 (cria + renderiza) + 5 (remove do composer).
- **Item 4 (sticky-scroll)** → Task 3.
- **Fora de escopo** (tmux theme, Slice 1B, Phase 4, backlog) — não incluídos, conforme spec.

Consistência de tipos: `pending: { id: string; text: string }[]` é a mesma forma em Chat, MessageList e nas funções de push/dedup. `ActivityBadge` recebe `label`, `costUsd`, `onCancel` — todos providos pela MessageList a partir de `stateEvent.label`/`costUsd`/`onCancel`.
