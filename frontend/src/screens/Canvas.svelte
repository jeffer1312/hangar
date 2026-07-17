<script lang="ts">
  import { onMount } from 'svelte';
  import { SvelteMap } from 'svelte/reactivity';
  import BoardCard from '../components/BoardCard.svelte';
  import RateStrip from '../components/RateStrip.svelte';
  import type { BoardRow, PendingMsg } from './Board.svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { serverColor } from '../lib/auth';
  import { pairColor } from '../lib/format';
  import { placeNew, PAD, GAP, CARD_W, CARD_H, type CanvasLayout, type CardBox } from '../lib/canvasLayout';

  interface Props { onOpenSession: (name: string, serverId: string) => void }
  let { onOpenSession }: Props = $props();

  onMount(() => {
    sessionsStore.retain();
    return () => sessionsStore.release();
  });

  const rows = $derived<BoardRow[]>(sessionsStore.rows);
  const offline = $derived(sessionsStore.byServer.filter((b) => b.error).map((b) => b.server.label));
  const rowKey = (r: BoardRow) => `${r.serverId}::${r.name}`;

  // ── Ocultar/desocultar cards (persistido): chave serverId::name, mesmo esquema do layout.
  // Sessão morta mantém a marca (se ressuscitar, volta oculta como estava) — barato, sem poda. ──
  const HIDDEN_KEY = 'cp_canvas_hidden';
  function loadHidden(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem(HIDDEN_KEY) ?? '[]');
      return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : [];
    } catch { return []; }
  }
  let hidden = $state<string[]>(loadHidden());
  function saveHidden() {
    try { localStorage.setItem(HIDDEN_KEY, JSON.stringify(hidden)); }
    catch (e) { console.warn('cp_canvas_hidden: falha ao persistir (estado só em memória)', e); }
  }
  function hide(key: string) {
    if (!hidden.includes(key)) { hidden = [...hidden, key]; saveHidden(); }
  }
  function unhide(key: string) {
    hidden = hidden.filter((k) => k !== key);
    saveHidden();
  }
  // ── Grupos de pareamento como CIDADÃOS do canvas: moldura visual em volta dos membros,
  // colapsar o grupo num card compacto único (persistido) e focar ("só este grupo"). ──
  const COLLAPSED_KEY = 'cp_canvas_collapsed';
  function loadCollapsed(): string[] {
    try {
      const v = JSON.parse(localStorage.getItem(COLLAPSED_KEY) ?? '[]');
      return Array.isArray(v) ? v.filter((x) => typeof x === 'string') : [];
    } catch { return []; }
  }
  let collapsedGids = $state<string[]>(loadCollapsed());
  function saveCollapsed() {
    try { localStorage.setItem(COLLAPSED_KEY, JSON.stringify(collapsedGids)); }
    catch (e) { console.warn('cp_canvas_collapsed: falha ao persistir', e); }
  }
  function toggleCollapse(gid: string) {
    collapsedGids = collapsedGids.includes(gid)
      ? collapsedGids.filter((g) => g !== gid)
      : [...collapsedGids, gid];
    saveCollapsed();
  }
  // Foco é efêmero de propósito (não persiste): "ver só eles" é um modo momentâneo, voltar do
  // reload com o canvas filtrado sem aviso seria o bug do card sumido de novo.
  let focusGid = $state<string | null>(null);

  const visibleRows = $derived(rows.filter((r) => {
    const k = rowKey(r);
    if (hidden.includes(k)) return false;
    if (focusGid && r.pair_gid !== focusGid) return false;
    if (r.pair_gid && collapsedGids.includes(r.pair_gid)) return false;
    return true;
  }));
  const hiddenRows = $derived(rows.filter((r) => hidden.includes(rowKey(r))));
  let showHidden = $state(false);

  // Moldura por grupo: bounding box dos MEMBROS RENDERIZADOS (2+), com folga pro header. Segue os
  // cards onde estiverem — arrastar um membro estica a moldura, o vínculo continua visível.
  const groupFrames = $derived.by(() => {
    const byGid = new Map<string, BoardRow[]>();
    for (const r of visibleRows) {
      if (!r.pair_gid) continue;
      const arr = byGid.get(r.pair_gid);
      if (arr) arr.push(r); else byGid.set(r.pair_gid, [r]);
    }
    const out: { gid: string; x: number; y: number; w: number; h: number; color: string; label: string; n: number }[] = [];
    for (const [gid, members] of byGid) {
      const boxes = members.map((m) => layout[rowKey(m)]).filter(Boolean);
      if (boxes.length < 2) continue;
      const x = Math.min(...boxes.map((b) => b.x)) - 10;
      const y = Math.min(...boxes.map((b) => b.y)) - 28;
      const x2 = Math.max(...boxes.map((b) => b.x + b.w)) + 10;
      const y2 = Math.max(...boxes.map((b) => b.y + b.h)) + 10;
      out.push({
        gid, x, y, w: x2 - x, h: y2 - y,
        color: pairColor(gid),
        label: members[0].pair_task ?? members.map((m) => m.name).join(' · '),
        n: members.length,
      });
    }
    return out;
  });

  // Grupo colapsado = UM card compacto no lugar dos membros (posição = canto do bounding box
  // salvo; expande de volta no clique). Membros mantêm posição no layout — expandir restaura.
  const collapsedCards = $derived.by(() =>
    collapsedGids.flatMap((gid) => {
      const members = rows.filter((r) => r.pair_gid === gid && !hidden.includes(rowKey(r)));
      if (members.length === 0) return [];
      if (focusGid && focusGid !== gid) return [];
      const boxes = members.map((m) => layout[rowKey(m)]).filter(Boolean);
      const x = boxes.length ? Math.min(...boxes.map((b) => b.x)) : PAD;
      const y = boxes.length ? Math.min(...boxes.map((b) => b.y)) : PAD;
      const w = boxes.length ? Math.max(...boxes.map((b) => b.w)) : CARD_W;
      return [{ gid, x, y, w, color: pairColor(gid), label: members[0].pair_task ?? null, members }];
    }));

  // ── Organizar: recoloca TODOS os visíveis numa grade — pareados (gid) contíguos, quem espera
  // por você primeiro, depois working, depois idle; 3 colunas dividindo a LARGURA da tela por
  // igual (pedido: cards redimensionados pra preencher, não 320px fixos encostados à esquerda). ──
  let planeWidth = $state(0);
  function autoArrange() {
    const rank = (s: string) => (s === 'awaiting_input' ? 0 : s === 'working' ? 1 : 2);
    const groups = new Map<string, BoardRow[]>();
    for (const r of visibleRows) {
      const g = r.pair_gid ? `g:${r.pair_gid}` : `s:${rowKey(r)}`;
      const arr = groups.get(g);
      if (arr) arr.push(r); else groups.set(g, [r]);
    }
    const orderedGroups = [...groups.values()]
      .map((members) => [...members].sort((a, b) =>
        rank(a.state) - rank(b.state) || (b.last_activity ?? 0) - (a.last_activity ?? 0)))
      .sort((a, b) =>
        Math.min(...a.map((m) => rank(m.state))) - Math.min(...b.map((m) => rank(m.state))) ||
        Math.max(...b.map((m) => m.last_activity ?? 0)) - Math.max(...a.map((m) => m.last_activity ?? 0)));
    // 3 colunas dividindo a largura visível por igual (fallback CARD_W se a medida ainda não veio).
    const cols = 3;
    const w = planeWidth > 0
      ? Math.max(280, Math.floor((planeWidth - PAD * 2 - GAP * (cols - 1)) / cols))
      : CARD_W;
    const next: CanvasLayout = { ...layout };
    let i = 0;
    for (const group of orderedGroups) {
      // Par não quebra linha: se o grupo cabe numa linha mas não no resto desta, pula pro início
      // da próxima — senão "lado a lado" virava extremos de duas linhas na quebra da grade.
      const rest = cols - (i % cols);
      if (group.length > rest && group.length <= cols) i += rest;
      for (const r of group) {
        next[rowKey(r)] = {
          x: PAD + (i % cols) * (w + GAP),
          y: PAD + Math.floor(i / cols) * (CARD_H + GAP),
          w, h: CARD_H,
        };
        i++;
      }
    }
    layout = next;
    saveLayout();
  }

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
  // Ocultos não precisam de posição — ganham uma ao serem desocultados.
  $effect(() => {
    const fresh = placeNew(
      layout,
      visibleRows.map((r) => ({ key: rowKey(r), serverId: r.serverId, pairGid: r.pair_gid ?? null })),
      sessionsStore.servers.map((s) => s.id),
    );
    if (Object.keys(fresh).length) { layout = { ...layout, ...fresh }; saveLayout(); }
  });

  // Extensão do plano: o container interno cresce pra caber o card mais fundo/largo.
  const extent = $derived.by(() => {
    let w = 900, h = 600;
    for (const r of visibleRows) {
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

  // Empurra pra BAIXO (cascata) quem intersecta o card `key` — crescer um card não deixa mais
  // vizinho coberto por baixo dele. Card pode ser re-empurrado (dois irmãos jogados pro mesmo y
  // precisam se resolver ENTRE SI — um set de "já empurrado" deixava os dois sobrepostos); cada
  // empurrão só AUMENTA y (estritamente, pela condição de overlap), então termina — o teto de
  // iterações é só cinto de segurança.
  function resolveCollisions(key: string, base: CanvasLayout): CanvasLayout {
    const next = { ...base };
    const queue = [key];
    let iter = 0;
    while (queue.length && ++iter < 500) {
      const ak = queue.shift()!;
      const a = next[ak];
      if (!a) continue;
      for (const r of visibleRows) {
        const bk = rowKey(r);
        if (bk === ak || bk === key) continue;   // nunca re-empurra o card que o usuário segura
        const b = next[bk];
        if (!b) continue;
        if (a.x < b.x + b.w && a.x + a.w > b.x && a.y < b.y + b.h && a.y + a.h > b.y) {
          next[bk] = { ...b, y: a.y + a.h + GAP };
          queue.push(bk);
        }
      }
    }
    return next;
  }

  // Reúne o GRUPO de pareamento em volta do card âncora: membros empilham logo abaixo, com o
  // mesmo tamanho; membro oculto é desocultado (reunir = "quero ver o grupo inteiro"); terceiros
  // atropelados são empurrados pela cascata. Disparado pelo clique no chip 🤝 do card.
  function gatherPair(anchorKey: string, gid: string) {
    const a = layout[anchorKey];
    if (!a) return;
    const members = rows.filter((r) => r.pair_gid === gid && rowKey(r) !== anchorKey);
    if (members.length === 0) return;
    const memberKeys = members.map(rowKey);
    if (hidden.some((k) => memberKeys.includes(k))) {
      hidden = hidden.filter((k) => !memberKeys.includes(k));
      saveHidden();
    }
    let next: CanvasLayout = { ...layout };
    let y = a.y + a.h + GAP;
    for (const k of memberKeys) {
      next[k] = { x: a.x, y, w: a.w, h: a.h };
      y += a.h + GAP;
    }
    for (const k of [anchorKey, ...memberKeys]) next = resolveCollisions(k, next);
    layout = next;
    saveLayout();
  }

  // ── Resize: CSS resize:both nativo no wrapper; o observer captura, empurra vizinhos e persiste. ──
  function observeSize(node: HTMLElement, key: string) {
    const ro = new ResizeObserver(() => {
      const b = layout[key];
      if (!b) return;
      const w = node.offsetWidth, h = node.offsetHeight;
      if (w === b.w && h === b.h) return;
      layout = resolveCollisions(key, { ...layout, [key]: { ...b, w, h } });
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
  // Órfão = sem CARD renderizado (sessão morta OU oculta): o recibo de erro precisa sobreviver ao
  // sumiço do card por qualquer via — erro de envio atrás de um card oculto ficava invisível.
  const orphanErrors = $derived(
    [...sendErrors].filter(([k]) => !visibleRows.some((r) => rowKey(r) === k)),
  );
</script>

<div class="canvas" bind:clientWidth={planeWidth}>
  <!-- Topo fixo: ⚡5h/📅7d por servidor (compartilhado pela conta) + ações do canvas. -->
  <div class="cv-top">
    <RateStrip buckets={sessionsStore.byServer} />
    <div class="cv-actions">
      {#if focusGid}
        <button class="cv-btn active" onclick={() => (focusGid = null)}
                title="Sair do modo foco e mostrar todas as sessões">✕ mostrando só 1 grupo</button>
      {/if}
      <button class="cv-btn" onclick={autoArrange}
              title="Reorganiza numa grade: quem espera por você primeiro, pareados lado a lado">
        Organizar
      </button>
      {#if hiddenRows.length}
        <button class="cv-btn" class:active={showHidden} onclick={() => (showHidden = !showHidden)}>
          Ocultos ({hiddenRows.length})
        </button>
      {/if}
    </div>
  </div>
  {#if showHidden && hiddenRows.length}
    <div class="cv-hiddenrow">
      {#each hiddenRows as r (rowKey(r))}
        <button class="cv-chip" onclick={() => unhide(rowKey(r))} title="Mostrar de novo">
          <span class="cv-chip-dot" style="background: {serverColor(r.serverId)}" aria-hidden="true"></span>
          {r.name}
        </button>
      {/each}
    </div>
  {/if}
  {#if offline.length}
    <p class="cv-offline">sem conexão: {offline.join(', ')}</p>
  {/if}
  {#each orphanErrors as [key, msg] (key)}
    <button class="cv-senderr" onclick={() => sendErrors.delete(key)} title="Dispensar">
      {key.split('::')[1]}: {msg} — msg não entregue
    </button>
  {/each}
  <div class="cv-plane" style="width: {extent.w}px; height: {extent.h}px;">
    <!-- Molduras de grupo ANTES dos cards (ordem no DOM = ficam por trás). Moldura segue o
         bounding box dos membros; header dela concentra as ações do grupo. -->
    {#each groupFrames as f (f.gid)}
      <div class="cv-group" style="left: {f.x}px; top: {f.y}px; width: {f.w}px; height: {f.h}px; color: {f.color};">
        <div class="cv-group-head">
          <span class="cv-group-label" title={f.label}>🤝 {f.label} · {f.n}</span>
          <button onclick={() => gatherPair(rowKey(visibleRows.find((r) => r.pair_gid === f.gid)!), f.gid)}
                  title="Reunir os membros lado a lado">⇱</button>
          <button onclick={() => toggleCollapse(f.gid)} title="Colapsar o grupo num card só">▾</button>
          <button onclick={() => (focusGid = focusGid === f.gid ? null : f.gid)}
                  title="Ver só este grupo (esconde o resto)">◎</button>
        </div>
      </div>
    {/each}
    <!-- Grupo colapsado: um card compacto no lugar dos membros. -->
    {#each collapsedCards as g (g.gid)}
      <div class="cv-gcard" style="left: {g.x}px; top: {g.y}px; width: {g.w}px; color: {g.color};">
        <button class="cv-gcard-head" onclick={() => toggleCollapse(g.gid)}
                title="Expandir o grupo de volta">
          ▸ 🤝 {g.label ?? g.members.map((m) => m.name).join(' · ')}
        </button>
        {#each g.members as m (rowKey(m))}
          <button class="cv-gcard-row" onclick={() => onOpenSession(m.name, m.serverId)}
                  title="Abrir chat de {m.name}">
            <span class="cv-chip-dot" style="background: {serverColor(m.serverId)}" aria-hidden="true"></span>
            <span class="cv-gcard-name">{m.name}</span>
            <span class="cv-gcard-state" data-state={m.state}>{m.state === 'awaiting_input' ? 'você' : m.state === 'working' ? 'exec' : 'pronto'}</span>
          </button>
        {/each}
      </div>
    {/each}
    {#each visibleRows as row (rowKey(row))}
      {@const key = rowKey(row)}
      {@const box = layout[key]}
      {#if box}
        <div class="cv-card" use:observeSize={key}
             style="left: {box.x}px; top: {box.y}px; width: {box.w}px; height: {box.h}px;">
          <!-- Barra tingida com a COR DO GRUPO (pairColor): membros do mesmo pareamento se
               reconhecem de longe no canvas; sem par, barra neutra de sempre. -->
          <div class="cv-handle" onpointerdown={(e) => dragStart(e, key)} onpointermove={dragMove}
               onpointerup={dragEnd} onpointercancel={dragEnd}
               role="button" tabindex="-1" aria-label={`Mover ${row.name}`}
               style={row.pair_gid ? `background: color-mix(in srgb, ${pairColor(row.pair_gid)} 16%, var(--bg-surface)); color: ${pairColor(row.pair_gid)};` : ''}
               title="Arrastar pra mover">⋮⋮</div>
          <!-- IRMÃO do handle (não filho): botão real dentro de role="button" é aninhamento
               interativo inválido (ARIA). Absoluto por cima da faixa; intercepta o ponteiro antes
               do handle, então não dispara drag. -->
          <button class="cv-hide" onclick={() => hide(key)}
                  title="Ocultar card (volta em «Ocultos»)" aria-label={`Ocultar ${row.name}`}>−</button>
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
              onGatherPair={row.pair_gid ? () => gatherPair(key, row.pair_gid!) : null}
            />
          </div>
        </div>
      {/if}
    {/each}
    {#if rows.length === 0}
      <p class="cv-empty">nenhuma sessão viva</p>
    {:else if visibleRows.length === 0}
      <p class="cv-empty">todos os cards estão ocultos — use «Ocultos» no topo pra trazer de volta</p>
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
    position: relative;
    flex-shrink: 0; height: 18px; cursor: grab; touch-action: none;
    display: flex; align-items: center; justify-content: center;
    color: var(--text-muted); font-size: 9px; letter-spacing: 2px; line-height: 1;
    border: 1px solid var(--border-subtle); border-bottom: 0;
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
    background: var(--bg-surface);
    user-select: none;
  }
  .cv-handle:active { cursor: grabbing; }
  /* Ocultar: irmão do handle, ancorado na faixa; aparece no hover do card (descoberta sem poluir
     15 cards com 15 botões). */
  .cv-hide {
    position: absolute; right: 4px; top: 2px; z-index: 2;
    width: 16px; height: 14px; padding: 0; line-height: 1;
    display: flex; align-items: center; justify-content: center;
    background: none; border: 0; border-radius: var(--radius-sm);
    color: var(--text-muted); font-size: 12px; cursor: pointer;
    opacity: 0; transition: opacity 120ms var(--ease-out), background 120ms var(--ease-out);
    min-width: 0; min-height: 0;
  }
  .cv-card:hover .cv-hide { opacity: 1; }
  .cv-hide:hover { background: var(--bg-hover); color: var(--text-primary); }

  /* Topo: RateStrip + ações, fixo nos DOIS eixos de scroll do canvas. */
  .cv-top {
    position: sticky; top: 0; left: 0; z-index: 4;
    display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-3);
    width: fit-content; min-width: 100%;
    padding-right: var(--space-3);
  }
  .cv-actions { display: flex; gap: 6px; padding-top: var(--space-2); }
  .cv-btn {
    font-size: var(--text-xs); padding: 3px 12px; min-height: 0; min-width: 0;
    border-radius: var(--radius-full); border: 1px solid var(--border-default);
    background: var(--bg-surface); color: var(--text-primary); cursor: pointer;
    transition: background 120ms var(--ease-out);
  }
  .cv-btn:hover { background: var(--bg-hover); }
  .cv-btn.active { background: var(--accent-dim); border-color: var(--accent); }
  .cv-hiddenrow {
    position: sticky; left: 0; z-index: 4;
    display: flex; flex-wrap: wrap; gap: 6px;
    padding: var(--space-2) var(--space-3) 0;
  }
  .cv-chip {
    display: inline-flex; align-items: center; gap: 6px;
    font-size: var(--text-xs); padding: 3px 10px; min-height: 0; min-width: 0;
    border-radius: var(--radius-full); border: 1px solid var(--border-subtle);
    background: var(--bg-surface); color: var(--text-secondary); cursor: pointer;
  }
  .cv-chip:hover { background: var(--bg-hover); color: var(--text-primary); }
  .cv-chip-dot { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }

  /* Moldura de grupo: por trás dos cards (ordem no DOM), borda inteira + véu na cor do grupo.
     pointer-events só no header — a moldura nunca rouba clique/drag dos cards. */
  .cv-group {
    position: absolute; pointer-events: none;
    border: 1px solid color-mix(in srgb, currentColor 45%, transparent);
    background: color-mix(in srgb, currentColor 5%, transparent);
    border-radius: var(--radius-lg);
  }
  .cv-group-head {
    position: absolute; top: 3px; left: 10px; right: 10px;
    display: flex; align-items: center; gap: 2px;
    pointer-events: auto;
    font-size: var(--text-xs); font-weight: 600;
  }
  .cv-group-label {
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; margin-right: 4px;
  }
  .cv-group-head button {
    background: none; border: 0; color: inherit; cursor: pointer;
    font-size: 11px; line-height: 1; padding: 2px 5px; border-radius: var(--radius-sm);
    min-height: 0; min-width: 0; opacity: 0.75;
  }
  .cv-group-head button:hover { opacity: 1; background: color-mix(in srgb, currentColor 14%, transparent); }

  /* Grupo colapsado: card compacto — header expande, linhas abrem o chat do membro. */
  .cv-gcard {
    position: absolute; display: flex; flex-direction: column;
    background: var(--bg-surface);
    border: 1px solid color-mix(in srgb, currentColor 45%, transparent);
    border-radius: var(--radius-lg); overflow: hidden;
    padding-bottom: 4px;
  }
  .cv-gcard-head {
    text-align: left; background: color-mix(in srgb, currentColor 10%, transparent);
    border: 0; color: inherit; font: inherit; font-size: var(--text-sm); font-weight: 600;
    padding: 8px 12px; cursor: pointer; min-height: 0;
    white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  }
  .cv-gcard-row {
    display: flex; align-items: center; gap: 8px; text-align: left;
    background: none; border: 0; cursor: pointer; min-height: 0; min-width: 0;
    padding: 5px 12px; font: inherit; font-size: var(--text-xs); color: var(--text-primary);
  }
  .cv-gcard-row:hover { background: var(--bg-hover); }
  .cv-gcard-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .cv-gcard-state { color: var(--text-muted); }
  .cv-gcard-state[data-state='awaiting_input'] { color: var(--pill-input-fg); }
  .cv-gcard-state[data-state='working'] { color: var(--pill-working-fg); }
  .cv-body { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  /* O BoardCard interno preenche o corpo (prop fill). flex-shrink não se aplica (absolute),
     mas o min-height: 0 acima é o equivalente aqui: sem ele o body estoura em vez de rolar. */
  .cv-body > :global(.bcard) { flex: 1; min-height: 0; }
  .cv-empty { color: var(--text-muted); font-size: var(--text-xs); padding: var(--space-6); }
</style>
