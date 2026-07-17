<script lang="ts">
  import { onMount } from 'svelte';
  import { SvelteMap } from 'svelte/reactivity';
  import BoardCard from '../components/BoardCard.svelte';
  import RateStrip from '../components/RateStrip.svelte';
  import type { BoardRow, PendingMsg } from './Board.svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { serverColor } from '../lib/auth';
  import { placeNew, PAD, type CanvasLayout, type CardBox } from '../lib/canvasLayout';

  interface Props { onOpenSession: (name: string, serverId: string) => void }
  let { onOpenSession }: Props = $props();

  onMount(() => {
    sessionsStore.retain();
    return () => sessionsStore.release();
  });

  const rows = $derived<BoardRow[]>(sessionsStore.rows);
  const offline = $derived(sessionsStore.byServer.filter((b) => b.error).map((b) => b.server.label));
  const rowKey = (r: BoardRow) => `${r.serverId}::${r.name}`;

  // ── Layout persistido: posição+tamanho por serverId::name (mesmo padrão dos drafts do Board). ──
  const LAYOUT_KEY = 'cp_canvas_layout';
  function loadLayout(): CanvasLayout {
    let raw: unknown;
    try { raw = JSON.parse(localStorage.getItem(LAYOUT_KEY) ?? '{}'); } catch { return {}; }
    // O try/catch só pega JSON inválido, não shape errado: uma entrada sem x/y/w/h numéricos viraria
    // NaN no style e envenenaria o extent. Fica só com as entradas cujos 4 campos sejam todos finitos.
    const src = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {};
    const out: CanvasLayout = {};
    for (const [k, v] of Object.entries(src)) {
      const b = v as Record<string, unknown>;
      if (b && ['x', 'y', 'w', 'h'].every((f) => Number.isFinite(b[f]))) out[k] = b as unknown as CardBox;
    }
    return out;
  }
  let layout = $state<CanvasLayout>(loadLayout());
  function saveLayout() {
    try { localStorage.setItem(LAYOUT_KEY, JSON.stringify(layout)); } catch { /* quota/priv mode */ }
  }
  // Sessão morta mantém a entrada salva (volta no mesmo lugar se ressuscitar) — barato, sem poda.

  // Card novo (sem posição salva) nasce via placeNew: coluna por servidor, pareados juntos.
  $effect(() => {
    const fresh = placeNew(
      layout,
      rows.map((r) => ({ key: rowKey(r), serverId: r.serverId, pairGid: r.pair_gid ?? null })),
      sessionsStore.servers.map((s) => s.id),
    );
    if (Object.keys(fresh).length) { layout = { ...layout, ...fresh }; saveLayout(); }
  });

  // Extensão do plano: o container interno cresce pra caber o card mais fundo/largo.
  const extent = $derived.by(() => {
    let w = 900, h = 600;
    for (const r of rows) {
      const b = layout[rowKey(r)];
      if (!b) continue;
      w = Math.max(w, b.x + b.w + PAD);
      h = Math.max(h, b.y + b.h + PAD);
    }
    return { w, h };
  });

  // ── Drag pelo handle (o card em si é interativo — input/botões — então o drag tem faixa própria). ──
  let drag: { key: string; dx: number; dy: number } | null = null;
  function dragStart(e: PointerEvent, key: string) {
    const b = layout[key];
    if (!b) return;
    drag = { key, dx: e.clientX - b.x, dy: e.clientY - b.y };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function dragMove(e: PointerEvent) {
    if (!drag) return;
    const b = layout[drag.key];
    if (!b) { drag = null; return; }   // sessão morreu no meio do arrasto -> não grava entrada corrompida sem w/h
    layout = { ...layout, [drag.key]: { ...b, x: Math.max(0, e.clientX - drag.dx), y: Math.max(0, e.clientY - drag.dy) } };
  }
  function dragEnd() {
    if (!drag) return;
    drag = null;
    saveLayout();
  }

  // ── Resize: CSS resize:both nativo no wrapper; o observer captura e persiste. ──
  function observeSize(node: HTMLElement, key: string) {
    const ro = new ResizeObserver(() => {
      const b = layout[key];
      if (!b) return;
      const w = node.offsetWidth, h = node.offsetHeight;
      if (w === b.w && h === b.h) return;
      layout = { ...layout, [key]: { ...b, w, h } };
      saveLayout();   // dispara a cada passo do arrasto de resize; barato (um setItem)
    });
    ro.observe(node);
    return { destroy() { ro.disconnect(); } };
  }

  // ── Estado içado por card (mesmo padrão e motivo do Board: o recibo de erro precisa sobreviver ao
  // sumiço da linha; ver Board.svelte). ponytail: 2ª cópia consciente do padrão — um 3º consumidor
  // extrai um host comum. ──
  const drafts = new Map<string, string>();
  const pendings = new SvelteMap<string, PendingMsg[]>();
  const sendErrors = new SvelteMap<string, string>();
  function updatePending(key: string, fn: (prev: PendingMsg[]) => PendingMsg[]) {
    const next = fn(pendings.get(key) ?? []);
    if (next.length) pendings.set(key, next);
    else pendings.delete(key);
  }
  function setSendError(key: string, msg: string) {
    if (msg) sendErrors.set(key, msg);
    else sendErrors.delete(key);
  }
  const orphanErrors = $derived(
    [...sendErrors].filter(([k]) => !rows.some((r) => rowKey(r) === k)),
  );
</script>

<div class="canvas">
  <!-- ⚡5h/📅7d por servidor — compartilhado pela conta, então acima de todas as sessões. -->
  <RateStrip buckets={sessionsStore.byServer} />
  {#if offline.length}
    <p class="cv-offline">sem conexão: {offline.join(', ')}</p>
  {/if}
  {#each orphanErrors as [key, msg] (key)}
    <button class="cv-senderr" onclick={() => sendErrors.delete(key)} title="Dispensar">
      {key.split('::')[1]}: {msg} — msg não entregue
    </button>
  {/each}
  <div class="cv-plane" style="width: {extent.w}px; height: {extent.h}px;">
    {#each rows as row (rowKey(row))}
      {@const key = rowKey(row)}
      {@const box = layout[key]}
      {#if box}
        <div class="cv-card" use:observeSize={key}
             style="left: {box.x}px; top: {box.y}px; width: {box.w}px; height: {box.h}px;">
          <div class="cv-handle" onpointerdown={(e) => dragStart(e, key)} onpointermove={dragMove}
               onpointerup={dragEnd} onpointercancel={dragEnd}
               role="button" tabindex="-1" aria-label={`Mover ${row.name}`}
               title="Arrastar pra mover">⋮⋮</div>
          <div class="cv-body">
            <BoardCard
              session={row}
              server={sessionsStore.servers.find((s) => s.id === row.serverId)!}
              color={serverColor(row.serverId)}
              fill
              draft={drafts.get(key) ?? ''}
              onDraftChange={(t) => drafts.set(key, t)}
              pending={pendings.get(key) ?? []}
              updatePending={(fn) => updatePending(key, fn)}
              sendError={sendErrors.get(key) ?? ''}
              onSendError={(m) => setSendError(key, m)}
              onOpen={() => onOpenSession(row.name, row.serverId)}
            />
          </div>
        </div>
      {/if}
    {/each}
    {#if rows.length === 0}
      <p class="cv-empty">nenhuma sessão viva</p>
    {/if}
  </div>
</div>

<style>
  /* Canvas livre: scroll nativo nos 2 eixos (sem pan/zoom na v1); cards absolutos. Mesmas regras
     visuais do board: cor não tinge fundo, elevação por borda hairline, sem animação nova. */
  .canvas { height: 100%; overflow: auto; padding: 0; position: relative; }
  .cv-offline { color: var(--warning); font-size: var(--text-xs); margin: var(--space-2) 24px 0; position: sticky; top: 8px; left: 24px; z-index: 3; }
  .cv-senderr {
    display: block; text-align: left; padding: 0; margin: var(--space-2) 24px 0;
    background: none; border: 0; cursor: pointer; min-height: 0; min-width: 0;
    color: var(--error); font-family: inherit; font-size: var(--text-xs);
    position: sticky; left: 24px; z-index: 3;
  }
  .cv-plane { position: relative; }
  .cv-card {
    position: absolute; display: flex; flex-direction: column;
    resize: both; overflow: hidden;                      /* resize nativo; observer persiste */
    min-width: 240px; min-height: 160px;
    border-radius: var(--radius-lg);
  }
  .cv-handle {
    flex-shrink: 0; height: 18px; cursor: grab; touch-action: none;
    display: flex; align-items: center; justify-content: center;
    color: var(--text-muted); font-size: 9px; letter-spacing: 2px; line-height: 1;
    border: 1px solid var(--border-subtle); border-bottom: 0;
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
    background: var(--bg-surface);
    user-select: none;
  }
  .cv-handle:active { cursor: grabbing; }
  .cv-body { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  /* O BoardCard interno preenche o corpo (prop fill). flex-shrink não se aplica (absolute),
     mas o min-height: 0 acima é o equivalente aqui: sem ele o body estoura em vez de rolar. */
  .cv-body > :global(.bcard) { flex: 1; min-height: 0; }
  .cv-empty { color: var(--text-muted); font-size: var(--text-xs); padding: var(--space-6); }
</style>
