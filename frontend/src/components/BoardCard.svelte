<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import AssistantBubble from './AssistantBubble.svelte';
  import { getHistoryTailForServer, sendInputForServer, selectOptionForServer } from '../lib/api';
  import { relativeTime } from '../lib/format';
  import type { Server } from '../lib/auth';
  import type { ChatEvent, State } from '../lib/types';
  import type { BoardRow } from '../screens/Board.svelte';

  interface Props {
    session: BoardRow;
    server: Server;
    color: string;                    // cor do servidor (dot)
    draft: string;                    // rascunho içado no Board (sobrevive à troca de coluna)
    onDraftChange: (t: string) => void;
    onOpen: () => void;               // abre o chat completo
  }
  let { session, server, color, draft, onDraftChange, onOpen }: Props = $props();

  const TAIL = 15;
  let events = $state<ChatEvent[]>([]);
  // `solid` = eco já consumido pelo Claude, mas que pode nunca virar entrada no transcript (fila).
  let pending = $state<{ id: string; text: string; solid?: boolean }[]>([]);
  let sendError = $state('');
  let loading = $state(true);
  let bodyEl = $state<HTMLElement>();
  // Semeia UMA vez com o rascunho içado; daqui pra frente o dono do texto é o card (untrack: reagir a
  // `draft` faria o Board reescrever por cima do que está sendo digitado).
  let text = $state(untrack(() => draft));
  // Rascunho içado: cada tecla persiste no Map do Board. Corpo em BLOCO de propósito — o
  // onDraftChange do Board é `(t) => drafts.set(...)`, que DEVOLVE o Map; num arrow de corpo-expressão
  // o Svelte leria esse retorno como teardown do efeito e quebraria (effect_returns_value).
  $effect(() => {
    onDraftChange(text);
  });

  async function loadTail() {
    try {
      events = await getHistoryTailForServer(server, session.name, TAIL);
      solidifyPending();
      loading = false;
      requestAnimationFrame(() => bodyEl?.scrollTo({ top: bodyEl.scrollHeight }));
    } catch { loading = false; /* offline/sem transcript: card fica só com header */ }
  }
  onMount(loadTail);

  // Fila do Claude Code: msg enviada com ele `working` MUITAS VEZES não vira entrada no transcript
  // (só as enviadas no idle viram prompt gravado) — mesma razão do solidify do Chat.svelte:634-646.
  // Sem isto o dedup abaixo nunca casaria e o eco ficaria meio-apagado pra sempre. Solidificar só
  // tira a marca de "não confirmada": o dedup ainda remove as que casarem com a cauda.
  function solidifyPending() {
    if (pending.some((p) => !p.solid)) pending = pending.map((p) => ({ ...p, solid: true }));
  }

  // Re-busca SÓ em transição de estado (working -> idle/awaiting: o texto estabilizou).
  // NUNCA por last_activity (com N sessões working viraria fetch contínuo + parse de jsonl no backend).
  let prevState: State = untrack(() => session.state);
  $effect(() => {
    const s = session.state;
    if (prevState === 'working' && (s === 'idle' || s === 'awaiting_input')) loadTail();
    prevState = s;
  });

  // Dedup do eco pendente contra a cauda (versão reduzida do contrato do Chat.svelte:612-632: texto
  // normalizado + por LINHA, já que o Claude Code funde msgs enfileiradas numa só, separadas por \n).
  $effect(() => {
    if (pending.length === 0) return;
    const committed = new Set<string>();
    for (const e of events) {
      if (e.kind !== 'user_msg' || !e.text) continue;
      const t = e.text.trim();
      committed.add(t);
      for (const line of t.split('\n')) committed.add(line.trim());
    }
    const next = pending.filter((p) => !committed.has(p.text.trim()));
    if (next.length !== pending.length) pending = next;
  });

  let seq = 0;
  async function send() {
    const t = text.trim();
    if (!t) return;
    const id = `bp-${seq++}`;
    pending = [...pending, { id, text: t }];
    text = '';
    sendError = '';
    try {
      await sendInputForServer(server, session.name, t);
    } catch (err) {
      // 404 = sessão morta etc: remove o eco e SINALIZA — msg nunca some calada.
      pending = pending.filter((p) => p.id !== id);
      text = t; // devolve pro input
      sendError = err instanceof Error ? err.message : 'falha no envio';
    }
  }
  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); send(); }
  }

  let optBusy = $state(false);
  async function answer(i: number) {
    if (optBusy) return;
    optBusy = true;
    sendError = '';
    // option é 1-BASED (api.ts:150 -> backend Field(ge=1) -> (option-1)×Down + Enter).
    try { await selectOptionForServer(server, session.name, i + 1); }
    catch (err) { sendError = err instanceof Error ? err.message : 'falha'; }
    finally { optBusy = false; }
  }

  const visible = $derived(events.filter((e) => (e.kind === 'user_msg' || e.kind === 'assistant_msg') && e.text));
</script>

<article class="bcard" class:attention={session.state === 'awaiting_input'}>
  {#if session.state === 'working'}
    <!-- Progresso indeterminado: o sinal de "trabalhando" do card. NÃO pulsa (só o dot da coluna
         "Precisa de você" pulsa no board inteiro). -->
    <div class="bc-progress" aria-hidden="true"></div>
  {/if}
  <header class="bc-head" onclick={onOpen} onkeydown={(e) => e.key === 'Enter' && onOpen()} role="button" tabindex="0">
    <span class="bc-dot" style="background: {color}" aria-hidden="true"></span>
    <span class="bc-name">{session.name}</span>
    {#if session.branch}<span class="bc-branch">⎇ {session.branch}</span>{/if}
    {#if session.pair_peers?.length}<span class="bc-chip">🤝 {session.pair_peers.length}</span>{/if}
    <span class="bc-time">{relativeTime(session.last_activity)}</span>
    <span class="bc-open" title="Abrir chat completo">⤢</span>
  </header>

  <div class="bc-body" bind:this={bodyEl}>
    {#if loading}
      <p class="bc-empty">carregando…</p>
    {:else}
      {#each visible as e (e.id)}
        {#if e.kind === 'assistant_msg'}
          <!-- Sem sessionName: o FileAttachment resolveria o path contra o servidor ATIVO, que pode
               não ser o dono desta sessão (mesma razão do Compare.svelte:137-141). -->
          <AssistantBubble text={e.text ?? ''} animate={false} />
        {:else}
          <p class="bc-user">{e.text}</p>
        {/if}
      {/each}
      {#each pending as p (p.id)}
        <p class="bc-user" class:bc-pending={!p.solid}>{p.text}</p>
      {/each}
      {#if session.state === 'working' && session.label}
        <p class="bc-typing">✳ {session.label}</p>
      {/if}
      {#if session.state === 'awaiting_input' && session.question}
        <div class="bc-question">
          <p>{session.question}</p>
          {#if session.options?.length}
            <div class="bc-options">
              {#each session.options as opt, i (i)}
                <button class="bc-opt" disabled={optBusy} onclick={() => answer(i)}>{opt}</button>
              {/each}
            </div>
          {/if}
        </div>
      {/if}
    {/if}
  </div>

  {#if session.state !== 'dead' && session.tracked !== false}
    <footer class="bc-foot">
      <textarea rows="1" placeholder="Mensagem…" bind:value={text} onkeydown={onKey}></textarea>
      <button class="bc-send" onclick={send} disabled={!text.trim()} aria-label="Enviar">↑</button>
    </footer>
    {#if sendError}<p class="bc-error">{sendError}</p>{/if}
  {/if}
</article>

<style>
  /* Card = a superfície do board. Elevação por BORDA hairline, nunca sombra (não lê em dark).
     Sem fundo tingido por estado: a luminância é o eixo de hierarquia aqui. */
  .bcard {
    position: relative;
    display: flex; flex-direction: column;
    background: var(--bg-surface);
    border: 1px solid rgba(255, 255, 255, 0.06);
    border-radius: var(--radius-md);
    box-shadow: none;
    overflow: hidden;
  }
  /* Faixa colorida é EXCEÇÃO: só quem espera por você. Em todo card viraria listra decorativa. */
  .bcard.attention { border-left: 2px solid var(--warning); }
  .bc-progress {
    position: absolute; top: 0; left: 0; right: 0; height: 2px; overflow: hidden;
    background: transparent;
  }
  .bc-progress::after {
    content: ''; position: absolute; inset: 0; width: 40%;
    background: var(--accent);
    animation: bc-slide 1.6s var(--ease-in-out) infinite;
  }
  @keyframes bc-slide { from { transform: translateX(-100%); } to { transform: translateX(350%); } }
  .bc-head {
    display: flex; align-items: center; gap: 6px; min-width: 0;
    padding: var(--space-2); cursor: pointer;
  }
  .bc-head:hover { background: var(--bg-hover); }
  .bc-dot { width: 8px; height: 8px; border-radius: 50%; flex-shrink: 0; }
  /* Peso 510, não 600: hierarquia por luminância. */
  .bc-name { font-size: 13px; font-weight: 510; color: var(--text-primary); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .bc-branch, .bc-chip, .bc-time { font-size: var(--text-xs); color: var(--text-muted); flex-shrink: 0; }
  .bc-time { margin-left: auto; }
  .bc-open { color: var(--text-muted); flex-shrink: 0; }
  /* Fade SÓ no topo (o rodapé é a msg mais recente — o que você não quer apagar). Dois masks:
     o segundo preserva a scrollbar, senão ela some junto com o conteúdo mascarado. */
  .bc-body {
    max-height: 240px; overflow-y: auto;
    padding: 0 var(--space-2) var(--space-2);
    display: flex; flex-direction: column; gap: 6px;
    --mask-h: 28px; --sb-w: 8px;
    mask-image:
      linear-gradient(to bottom, transparent, black var(--mask-h)),
      linear-gradient(black, black);
    mask-size: calc(100% - var(--sb-w)) 100%, var(--sb-w) 100%;
    mask-position: 0 0, 100% 0;
    mask-repeat: no-repeat, no-repeat;
  }
  .bc-empty { color: var(--text-muted); font-size: var(--text-xs); }
  /* Papel por LUMINÂNCIA, não por bolha: bolha dentro de card é contenção dupla. */
  .bc-user {
    font-size: 13px; line-height: 1.45; color: var(--text-muted);
    white-space: pre-wrap; word-break: break-word; margin: 0;
  }
  .bc-pending { opacity: 0.55; }
  .bc-typing { color: var(--text-secondary); font-size: var(--text-xs); font-style: italic; }
  .bc-question { border-left: 2px solid var(--warning); padding-left: var(--space-2); font-size: var(--text-xs); }
  .bc-options { display: flex; flex-wrap: wrap; gap: 6px; margin-top: 6px; }
  .bc-opt { font-size: var(--text-xs); padding: 3px 10px; border-radius: var(--radius-full); border: 1px solid var(--border-default); background: var(--bg-elevated); color: var(--text-primary); cursor: pointer; }
  .bc-opt:hover:not(:disabled) { background: var(--bg-hover); }
  .bc-opt:disabled { opacity: 0.5; cursor: default; }
  /* Input FANTASMA: 15 cards × 15 inputs com borda = 15 alvos competindo. Borda real só no
     hover/focus. */
  .bc-foot { display: flex; gap: 6px; padding: var(--space-2); }
  .bc-foot textarea {
    flex: 1; resize: none; min-height: 28px; max-height: 72px; font: inherit; font-size: 12px;
    background: rgba(255, 255, 255, 0.03); color: var(--text-primary);
    border: 1px solid transparent; border-radius: var(--radius-sm); padding: 5px 8px;
    transition: border-color 120ms var(--ease-out);
  }
  .bc-foot textarea::placeholder { color: var(--text-muted); }
  .bc-foot:hover textarea { border-color: var(--border-subtle); }
  .bc-foot textarea:focus { border-color: var(--accent); outline: none; }
  .bc-send { width: 28px; border-radius: var(--radius-sm); border: 0; background: var(--accent); color: var(--text-inverse); cursor: pointer; }
  .bc-send:disabled { opacity: 0.4; cursor: default; }
  .bc-error { color: var(--error); font-size: var(--text-xs); padding: 0 var(--space-2) var(--space-2); margin: 0; }
</style>
