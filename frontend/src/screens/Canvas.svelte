<script lang="ts">
  import { onMount } from 'svelte';
import * as m from '../paraglide/messages';
  import { SvelteMap } from 'svelte/reactivity';
  import BoardCard from '../components/BoardCard.svelte';
  import GroupGlyph from '../components/icons/GroupGlyph.svelte';
  import RateStrip from '../components/RateStrip.svelte';
  import StateChip from '../components/StateChip.svelte';
  import type { BoardRow, PendingMsg } from './Board.svelte';
  import { sessionsStore } from '../lib/sessionsStore.svelte';
  import { serverColor } from '../lib/auth';
  import { pairColor, canPair, canLeave, isFallenSession, type DropResult } from '@hangar/core';
  import { arrastarGrupo, mensagemRecusa } from '../lib/arrastarGrupo.svelte';
  import {
    canvasBounds, connectBoxes, fitCanvasScale, placeNew, resizeBox, planePoint, findDropTarget,
    MIN_SCALE, MAX_SCALE, PAD, GAP, CARD_W, CARD_H,
    type CanvasLayout, type CardBox, type CanvasDropTarget,
  } from '../lib/canvasLayout';

  interface Props { onOpenSession: (name: string, serverId: string) => void }
  let { onOpenSession }: Props = $props();

  onMount(() => {
    sessionsStore.retain();
    return () => sessionsStore.release();
  });

  // Sessão caída não tem conversa para o card; Reabrir e Dispensar ficam na lista.
  const rows = $derived<BoardRow[]>(sessionsStore.rows.filter((r) => !isFallenSession(r)));
  const offline = $derived(sessionsStore.byServer.filter((b) => b.error && !b.server.inviteEnded).map((b) => b.server.label));
  const endedInvites = $derived(sessionsStore.byServer.filter((b) => b.server.inviteEnded).map((b) => b.server.label));
  const rowKey = (r: BoardRow) => `${r.serverId}::${r.name}`;

  // Migração one-shot da VIEW: os formatos de cp_canvas_hidden/cp_canvas_collapsed mudaram
  // (chave de grupo ganhou escopo de servidor) e marca órfã do formato antigo escondia cards sem
  // nenhuma UI explicando (o "sumiu tudo" de 2026-07-17 — só resolvia limpando o storage na mão).
  // Versão < atual -> descarta SÓ as marcas de exibição; layout de posições fica.
  const VIEW_VERSION = 2;
  try {
    if (Number(localStorage.getItem('cp_canvas_v') ?? '1') < VIEW_VERSION) {
      localStorage.removeItem('cp_canvas_hidden');
      localStorage.removeItem('cp_canvas_collapsed');
      localStorage.setItem('cp_canvas_v', String(VIEW_VERSION));
    }
  } catch { /* priv mode: sem storage, sem marca pra migrar */ }

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
    catch (e) { console.warn('cp_canvas_collapsed: ' + m.board_falha_persistir(), e); }
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

  // Chave de grupo com ESCOPO DE SERVIDOR: o gid é 8 hex gerado por máquina (pair.py) e o canvas
  // opera sobre o agregado multi-servidor — dois grupos independentes em backends diferentes
  // podiam colidir no mesmo gid e virar uma moldura só (com "reunir" arrastando sessão alheia).
  const gkeyOf = (r: BoardRow) => (r.pair_gid ? `${r.serverId}::${r.pair_gid}` : null);

  const visibleRows = $derived(rows.filter((r) => {
    const k = rowKey(r);
    const gk = gkeyOf(r);
    if (hidden.includes(k)) return false;
    if (focusGid && gk !== focusGid) return false;
    if (gk && collapsedGids.includes(gk)) return false;
    return true;
  }));
  const hiddenRows = $derived(rows.filter((r) => hidden.includes(rowKey(r))));
  let showHidden = $state(false);

  // Moldura por grupo: bounding box dos MEMBROS RENDERIZADOS (2+), com folga pro header. Segue os
  // cards onde estiverem — arrastar um membro estica a moldura, o vínculo continua visível.
  // Etiqueta do grupo (label + ações), ancorada acima do membro mais alto. Marcação de
  // pertencimento é POR MEMBRO (aro no card, ver .cv-card.paired) — a versão anterior desenhava
  // uma caixa envolvente das posições, e card ESTRANHO parado dentro do retângulo parecia membro.
  const groupFrames = $derived.by(() => {
    const byGid = new Map<string, BoardRow[]>();
    for (const r of visibleRows) {
      const gk = gkeyOf(r);
      if (!gk) continue;
      const arr = byGid.get(gk);
      if (arr) arr.push(r); else byGid.set(gk, [r]);
    }
    const out: { gid: string; x: number; y: number; color: string; label: string; n: number }[] = [];
    for (const [gk, members] of byGid) {
      const boxes = members.map((m) => layout[rowKey(m)]).filter(Boolean);
      if (boxes.length < 2) continue;
      const topmost = boxes.reduce((a, b) => (b.y < a.y || (b.y === a.y && b.x < a.x) ? b : a));
      // Clamp: membro na 1ª linha (y = PAD) jogava a etiqueta pra fora do scroll (offset negativo).
      out.push({
        gid: gk,
        x: Math.max(2, topmost.x),
        y: Math.max(2, topmost.y - 20),
        color: pairColor(gk),
        label: members[0].pair_task ?? members.map((m) => m.name).join(' · '),
        n: members.length,
      });
    }
    return out;
  });

  // Linhas de vínculo no idioma do n8n: curvas entre as bordas dos cards, sem seta porque o grupo
  // não tem ordem. Uma corrente por posição evita a teia completa de N×N em grupos maiores.
  const groupLinks = $derived.by(() => {
    const byGid = new Map<string, { key: string; box: CardBox }[]>();
    for (const row of visibleRows) {
      const gid = gkeyOf(row);
      const box = layout[rowKey(row)];
      if (!gid || !box) continue;
      const member = { key: rowKey(row), box };
      const group = byGid.get(gid);
      if (group) group.push(member); else byGid.set(gid, [member]);
    }
    return [...byGid].flatMap(([gid, members]) => {
      const ordered = [...members].sort((a, b) =>
        (a.box.x + a.box.w / 2) - (b.box.x + b.box.w / 2)
        || (a.box.y + a.box.h / 2) - (b.box.y + b.box.h / 2));
      return ordered.slice(1).map((member, index) => ({
        gid,
        key: `${ordered[index].key}->${member.key}`,
        color: pairColor(gid),
        ...connectBoxes(ordered[index].box, member.box),
      }));
    });
  });

  // Grupo colapsado = UM card compacto no lugar dos membros (posição = canto do bounding box
  // salvo; expande de volta no clique). Membros mantêm posição no layout — expandir restaura.
  const collapsedCards = $derived.by(() =>
    collapsedGids.flatMap((gk) => {
      const members = rows.filter((r) => gkeyOf(r) === gk && !hidden.includes(rowKey(r)));
      if (members.length === 0) return [];
      if (focusGid && focusGid !== gk) return [];
      const boxes = members.map((m) => layout[rowKey(m)]).filter(Boolean);
      const x = boxes.length ? Math.min(...boxes.map((b) => b.x)) : PAD;
      const y = boxes.length ? Math.min(...boxes.map((b) => b.y)) : PAD;
      const w = boxes.length ? Math.max(...boxes.map((b) => b.w)) : CARD_W;
      return [{ gid: gk, x, y, w, h: 40 + members.length * 30, color: pairColor(gk), label: members[0].pair_task ?? null, members }];
    }));

  // ── Organizar: recoloca TODOS os visíveis numa grade — pareados (gid) contíguos, quem espera
  // por você primeiro, depois working, depois idle; 3 colunas dividindo a LARGURA da tela por
  // igual (pedido: cards redimensionados pra preencher, não 320px fixos encostados à esquerda). ──
  let planeWidth = $state(0);
  let planeHeight = $state(0);
  let canvasEl = $state<HTMLDivElement>();
  let zoom = $state(1);
  const setZoom = (value: number) => (zoom = Math.min(MAX_SCALE, Math.max(MIN_SCALE, Math.round(value * 20) / 20)));
  function fitView() {
    zoom = fitCanvasScale(planeWidth, Math.max(200, planeHeight - 48), bounds.w + PAD * 2, bounds.h + PAD * 2);
    requestAnimationFrame(() => canvasEl?.scrollTo({
      left: Math.max(0, (bounds.x - PAD) * zoom),
      top: Math.max(0, (bounds.y - PAD) * zoom),
      behavior: 'smooth',
    }));
  }
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
      ? Math.max(280, Math.floor((planeWidth / zoom - PAD * 2 - GAP * (cols - 1)) / cols))
      : CARD_W;
    const next: CanvasLayout = { ...layout };
    let i = 0;
    for (const group of orderedGroups) {
      // Par não quebra linha: se o grupo cabe numa linha mas não no resto desta, pula pro início
      // da próxima — senão "lado a lado" virava extremos de duas linhas na quebra da grade.
      // Grupo que não cabe no resto da linha começa em linha NOVA — vale também pra grupo maior
      // que a própria grade (4+ membros com 3 colunas): senão ele partia do meio de uma linha
      // dividida com estranhos.
      const rest = cols - (i % cols);
      if (i % cols !== 0 && group.length > rest) i += rest;
      for (const r of group) {
        next[rowKey(r)] = {
          x: PAD + (i % cols) * (w + GAP),
          y: PAD + Math.floor(i / cols) * (CARD_H + GAP),
          w, h: CARD_H,
        };
        i++;
      }
      // Linha de grupo é EXCLUSIVA: estranho não preenche a sobra (card solo caindo do lado dos
      // últimos membros parecia parte do grupo — o bug do hangar "dentro" do ABC-1234).
      if (group.length > 1 && i % cols !== 0) i += cols - (i % cols);
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

  const renderedBoxes = $derived([
    ...visibleRows.map((row) => layout[rowKey(row)]).filter(Boolean),
    ...collapsedCards.map((group) => ({ x: group.x, y: group.y, w: group.w, h: group.h })),
  ]);

  // Extensão do plano: o container interno cresce pra caber todo conteúdo renderizado, inclusive
  // o card compacto de um grupo colapsado quando não há membro individual na tela.
  const extent = $derived.by(() => {
    let w = 900, h = 600;
    for (const b of renderedBoxes) {
      w = Math.max(w, b.x + b.w + PAD);
      h = Math.max(h, b.y + b.h + PAD);
    }
    return { w, h };
  });
  const bounds = $derived(canvasBounds(renderedBoxes));

  // ── Drag pelo handle (o card em si é interativo — input/botões — então o drag tem faixa própria).
  // Aqui arrastar já MOVE o tile (tiles podem se sobrepor à vontade) — por isso não é o HTML5
  // drag-and-drop da Sidebar/Board: é o mesmo pointerdown/move/up que já existia, com um hit-test
  // extra contra a FAIXA DE CABEÇALHO (topo, ~34px) dos outros cards. Sobrepor o CORPO deles
  // continua sendo só mover; só o cabeçalho (ou um grupo recolhido) vira alvo de pareamento.
  // A geometria pura (planePoint/findDropTarget) mora em canvasLayout.ts, testada lá — este arquivo
  // só monta as listas na ORDEM DE RENDERIZAÇÃO (findDropTarget varre invertido: quem foi
  // desenhado por último vence, senão cabeçalhos sobrepostos pareavam com o card de baixo). ──
  // $state: lido no template (:559, :583 via drag!.key) — variável comum aqui disparava
  // non_reactive_update no compilador Svelte 5.
  let drag = $state<{ key: string; x0: number; y0: number; box: CardBox } | null>(null);
  // Alvo sob o ponteiro DURANTE o arrasto — 'group' é um card recolhido (Step 2: não está no
  // layout, então o alvo guarda a chave de um MEMBRO representante pra canPair/soltar).
  let dropHover = $state<CanvasDropTarget | null>(null);

  // Mesma checagem que a Sidebar/Board usam pro destaque (Step 3): só avaliada pra quem está sob o
  // ponteiro agora — evita rodar canPair pros ~dezenas de cards que não estão em jogo.
  function avaliarDropCanvas(origemKey: string, alvo: BoardRow): DropResult | null {
    const origem = rows.find((r) => rowKey(r) === origemKey);
    return origem ? canPair(origem, alvo) : null;
  }

  function dragStart(e: PointerEvent, key: string) {
    const b = layout[key];
    if (!b) return;
    // Sem isto o soltar() do arrastarGrupo nunca abria o pedido: soltar só monta o pedido quando
    // `origem` já está preenchida, e aqui nunca tinha entrado (achado da revisão final).
    const origem = rows.find((r) => rowKey(r) === key);
    if (origem) arrastarGrupo.comecar({ serverId: origem.serverId, name: origem.name });
    drag = { key, x0: e.clientX, y0: e.clientY, box: { ...b } };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
  }
  function dragMove(e: PointerEvent) {
    if (!drag) return;
    if (!layout[drag.key]) { drag = null; dropHover = null; return; }   // sessão morreu no meio do arrasto -> não grava entrada corrompida sem w/h
    layout = { ...layout, [drag.key]: {
      ...drag.box,
      x: Math.max(0, drag.box.x + (e.clientX - drag.x0) / zoom),
      y: Math.max(0, drag.box.y + (e.clientY - drag.y0) / zoom),
    } };
    const rect = canvasEl?.getBoundingClientRect();
    if (!canvasEl || !rect) { dropHover = null; return; }
    const p = planePoint(e.clientX, e.clientY, rect, canvasEl.scrollLeft, canvasEl.scrollTop, zoom);
    // Mesma ordem do template ({#each visibleRows}/{#each collapsedCards}): é essa ordem que diz
    // quem foi desenhado por último (findDropTarget varre invertido a partir dela).
    const cards = visibleRows.flatMap((row) => {
      const box = layout[rowKey(row)];
      return box ? [{ key: rowKey(row), box }] : [];
    });
    const groups = collapsedCards.flatMap((g) =>
      g.members[0] ? [{ gid: g.gid, key: rowKey(g.members[0]), box: { x: g.x, y: g.y, w: g.w, h: g.h } }] : [],
    );
    dropHover = findDropTarget(p.x, p.y, drag.key, cards, groups);
  }
  // Reúne o grupo recém-formado em volta de onde o usuário soltou — senão o tile fica empilhado
  // por cima do alvo (Step 4). ponytail: sem timer — o efeito só reavalia quando pedido/rows mudam
  // (SSE trazendo o pair_gid novo, por exemplo); o teto de 4s abaixo só é conferido nessas
  // reavaliações, então "desiste" quando a PRÓXIMA mudança cair depois do teto, não sozinho aos 4s.
  // O grupo já formou de qualquer forma nesse caso, só a reunião automática que não rodou, e o ⇱
  // do rótulo do grupo resolve na mão.
  let pendingGather: { origemKey: string; alvoKey: string; at: number } | null = null;
  $effect(() => {
    // Dependências lidas SEMPRE, antes de qualquer return: pendingGather é variável comum (só o
    // gatilho, não precisa ser $state), e um efeito que só lê pedido/rows dentro do `if` perde a
    // inscrição neles na 1ª rodada (pendingGather nulo) e nunca mais reexecuta sozinho.
    const pedidoAtual = arrastarGrupo.pedido;
    void rows;
    const pending = pendingGather;
    if (!pending || pedidoAtual) return;
    const origemNow = rows.find((r) => rowKey(r) === pending.origemKey);
    const alvoNow = rows.find((r) => rowKey(r) === pending.alvoKey);
    // Sessão de origem/alvo sumida (morreu, foi ocultada) -> nada pra reunir, desiste sem esperar
    // o teto de 4s.
    if (!origemNow || !alvoNow) { pendingGather = null; return; }
    // Exige o MESMO gid do alvo deste arrasto, não só "gid mudou": se a origem entrar em OUTRO
    // grupo por outro caminho (outra aba, celular, a Sidebar pareando a mesma sessão) enquanto o
    // diálogo estava aberto, "mudou" seria verdade sem ter nada a ver com este drop — e reuniria
    // membros de um grupo errado.
    const gOrigem = gkeyOf(origemNow);
    const gAlvo = gkeyOf(alvoNow);
    if (gOrigem && gOrigem === gAlvo) {
      pendingGather = null;
      if (layout[pending.alvoKey]) gatherPair(pending.alvoKey, gAlvo);
    } else if (Date.now() - pending.at > 4000) {
      pendingGather = null;
    }
  });
  function dragEnd() {
    if (!drag) return;
    const trigger = drag;
    const hit = dropHover;
    drag = null;
    dropHover = null;
    saveLayout();
    // Tile só reposicionado (sem alvo válido): desfaz o comecar() de dragStart, senão a origem
    // fica pendurada no store compartilhado até o próximo arrasto de QUALQUER tela.
    if (!hit) { arrastarGrupo.cancelar(); return; }
    const origem = rows.find((r) => rowKey(r) === trigger.key);
    const alvo = rows.find((r) => rowKey(r) === hit.key);
    if (!origem || !alvo || !canPair(origem, alvo).ok) { arrastarGrupo.cancelar(); return; }
    pendingGather = { origemKey: trigger.key, alvoKey: hit.key, at: Date.now() };
    arrastarGrupo.soltar(hit.key);
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
  function gatherPair(anchorKey: string, gk: string) {
    const a = layout[anchorKey];
    if (!a) { console.warn(m.board_reunir_ancora() + ':', anchorKey); return; }
    const members = rows.filter((r) => gkeyOf(r) === gk && rowKey(r) !== anchorKey);
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

  // ── Resize por QUALQUER borda/canto: 8 alças finas em volta do card. O `resize: both` do CSS que
  // havia aqui só oferece o canto inferior-direito — puxar pela esquerda/topo é o pedido. A conta
  // (clamps, borda oposta parada) mora em canvasLayout.resizeBox, testada. ──
  const RESIZE_DIRS = ['n', 's', 'e', 'w', 'nw', 'ne', 'sw', 'se'] as const;
  let rs: { key: string; dir: string; x0: number; y0: number; box: CardBox } | null = null;
  function resizeStart(e: PointerEvent, key: string, dir: string) {
    const b = layout[key];
    if (!b) return;
    rs = { key, dir, x0: e.clientX, y0: e.clientY, box: { ...b } };
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    e.preventDefault();
    e.stopPropagation();
  }
  function resizeMove(e: PointerEvent) {
    if (!rs) return;
    // Botão já solto = gesto que terminou sem passar pelo resizeEnd. Acontece quando o card é
    // DESMONTADO no meio do arrasto (a sessão morreu): o nó sai do DOM, a captura vira
    // `lostpointercapture` e o `pointerup` não chega mais nesta alça. Sem isto o `rs` ficava preso e
    // o próximo HOVER sobre qualquer alça redimensionava o card antigo sozinho — e o tamanho
    // arrastado nunca chegava ao localStorage.
    if (e.buttons === 0) { resizeEnd(); return; }
    if (!layout[rs.key]) { rs = null; return; }   // entrada sumiu do layout: não grava caixa corrompida
    layout = { ...layout, [rs.key]: resizeBox(rs.box, rs.dir, (e.clientX - rs.x0) / zoom, (e.clientY - rs.y0) / zoom) };
  }
  function resizeEnd() {
    if (!rs) return;
    // Empurra vizinhos só no FIM: durante o arrasto a cascata jogava card pra baixo a cada pixel.
    layout = resolveCollisions(rs.key, layout);
    rs = null;
    saveLayout();
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

<div class="canvas-shell">
<div class="canvas" bind:this={canvasEl} bind:clientWidth={planeWidth} bind:clientHeight={planeHeight}>
  <!-- Topo fixo: ⚡5h/📅7d por servidor (compartilhado pela conta) + ações do canvas. -->
  <div class="cv-top">
    <RateStrip buckets={sessionsStore.byServer} />
    <div class="cv-actions">
      {#if focusGid}
        <button class="cv-btn active" onclick={() => (focusGid = null)}
                title={m.board_sair_foco()}>✕ {m.board_mostrando_1_grupo()}</button>
      {/if}
      <button class="cv-btn" onclick={autoArrange}
              title={m.board_reorganiza_grade()}>
        {m.board_organizar()}
      </button>
      {#if hiddenRows.length}
        <button class="cv-btn" class:active={showHidden} onclick={() => (showHidden = !showHidden)}>
          {m.canvas_ocultos_n({ n: hiddenRows.length })}
        </button>
      {/if}
    </div>
  </div>
  {#if showHidden && hiddenRows.length}
    <div class="cv-hiddenrow">
      {#each hiddenRows as r (rowKey(r))}
        <button class="cv-chip" onclick={() => unhide(rowKey(r))} title={m.board_mostrar_de_novo()}>
          <span class="cv-chip-dot" style="background: {serverColor(r.serverId)}" aria-hidden="true"></span>
          {r.name}
        </button>
      {/each}
    </div>
  {/if}
  {#if offline.length}
    <p class="cv-offline">{m.board_sem_conexao()}: {offline.join(', ')}</p>
  {/if}
  {#if endedInvites.length}
    <p class="cv-offline" role="status">{m.convite_encerrado()} ({endedInvites.join(', ')})</p>
  {/if}
  {#each orphanErrors as [key, msg] (key)}
    <button class="cv-senderr" onclick={() => sendErrors.delete(key)} title={m.board_dispensar()}>
      {key.split('::')[1]}: {msg} — {m.board_msg_nao_entregue()}
    </button>
  {/each}
  <div class="cv-world" style="width: {extent.w * zoom}px; height: {extent.h * zoom}px;">
  <div class="cv-plane" style="width: {extent.w}px; height: {extent.h}px; transform: scale({zoom});">
    {#if groupLinks.length}
      <svg class="cv-links" width={extent.w} height={extent.h} aria-hidden="true">
        {#each groupLinks as link (link.key)}
          <path d={link.path} style="stroke: {link.color};" vector-effect="non-scaling-stroke" />
          <circle cx={link.from.x} cy={link.from.y} r={3 / zoom} style="fill: var(--surface-card); stroke: {link.color};" vector-effect="non-scaling-stroke" />
          <circle cx={link.to.x} cy={link.to.y} r={3 / zoom} style="fill: var(--surface-card); stroke: {link.color};" vector-effect="non-scaling-stroke" />
        {/each}
      </svg>
    {/if}
    <!-- Etiqueta do grupo sobre o membro mais alto (label + ações). O pertencimento é o ARO nos
         cards membros (.cv-card.paired) — a caixa envolvente antiga enganava: card estranho parado
         dentro do retângulo parecia membro. -->
    {#each groupFrames as f (f.gid)}
      <div class="cv-group-tag" style="left: {f.x}px; top: {f.y}px; color: {f.color};">
        <span class="cv-group-label" title={f.label}><GroupGlyph size={13} /> {f.label} · {f.n}</span>
        <!-- Âncora = um membro que JÁ TEM box — o primeiro de visibleRows podia estar sem posição
             (placeNew roda pós-render) e o reunir virava no-op mudo; grupo desfeito entre render e
             clique também não pode estourar. -->
        <button onclick={() => {
                  const anchor = visibleRows.find((r) => gkeyOf(r) === f.gid && layout[rowKey(r)]);
                  if (anchor) gatherPair(rowKey(anchor), f.gid);
                  else console.warn('reunir: grupo sem membro posicionado', f.gid);
                }}
                title={m.board_reunir_membros()}>⇱</button>
        <button onclick={() => toggleCollapse(f.gid)} title={m.board_colapsar_grupo()}>▾</button>
        <button onclick={() => (focusGid = focusGid === f.gid ? null : f.gid)}
                title={m.board_ver_so_grupo()}>◎</button>
      </div>
    {/each}
    <!-- Grupo colapsado: um card compacto no lugar dos membros. Também é alvo de soltar (Step 2). -->
    {#each collapsedCards as g (g.gid)}
      {@const hoverAqui = dropHover?.kind === 'group' && dropHover.gid === g.gid}
      {@const dragKey = drag?.key}
      {@const dropResultado = hoverAqui && dragKey && g.members[0] ? avaliarDropCanvas(dragKey, g.members[0]) : null}
      {@const dropRecusa = dropResultado && !dropResultado.ok ? dropResultado.reason : null}
      <div class="cv-gcard" class:drop-alvo={dropResultado?.ok === true} class:drop-recusado={dropRecusa !== null}
           title={dropRecusa !== null ? mensagemRecusa(dropRecusa) : undefined}
           style="left: {g.x}px; top: {g.y}px; width: {g.w}px; color: {g.color};">
        <button class="cv-gcard-head" onclick={() => toggleCollapse(g.gid)}
                title={m.board_expandir_grupo()}>
          ▸ <GroupGlyph size={13} /> {g.label ?? g.members.map((m) => m.name).join(' · ')}
        </button>
        {#each g.members as membro (rowKey(membro))}
          <button class="cv-gcard-row" onclick={() => onOpenSession(membro.name, membro.serverId)}
                  title={m.board_abrir_chat_de({ n: membro.name })}>
            <span class="cv-chip-dot" style="background: {serverColor(membro.serverId)}" aria-hidden="true"></span>
            <span class="cv-gcard-name">{membro.name}</span>
            <StateChip state={membro.state} label={membro.state === 'awaiting_input' ? m.estado_voce() : membro.state === 'working' ? m.estado_exec() : m.estado_pronto()} />
          </button>
        {/each}
      </div>
    {/each}
    {#each visibleRows as row (rowKey(row))}
      {@const key = rowKey(row)}
      {@const box = layout[key]}
      {#if box}
        {@const hoverAqui = dropHover?.kind === 'card' && dropHover.key === key}
        {@const dragKey = drag?.key}
        {@const dropResultado = hoverAqui && dragKey ? avaliarDropCanvas(dragKey, row) : null}
        {@const dropRecusa = dropResultado && !dropResultado.ok ? dropResultado.reason : null}
        <div class="cv-card" class:paired={!!row.pair_gid}
             class:drop-alvo={dropResultado?.ok === true} class:drop-recusado={dropRecusa !== null}
             title={dropRecusa !== null ? mensagemRecusa(dropRecusa) : undefined}
             style="left: {box.x}px; top: {box.y}px; width: {box.w}px; height: {box.h}px;{row.pair_gid ? ` --pair-c: ${pairColor(gkeyOf(row)!)};` : ''}">
          <!-- Barra tingida com a COR DO GRUPO (pairColor): membros do mesmo pareamento se
               reconhecem de longe no canvas; sem par, barra neutra de sempre. -->
          <div class="cv-handle" onpointerdown={(e) => dragStart(e, key)} onpointermove={dragMove}
               onpointerup={dragEnd} onpointercancel={dragEnd}
               role="button" tabindex="-1" aria-label={m.canvas_mover_aria({ name: row.name })}
               style={row.pair_gid ? `background: color-mix(in srgb, ${pairColor(gkeyOf(row)!)} 16%, var(--bg-surface)); color: ${pairColor(gkeyOf(row)!)};` : ''}
               title={m.board_arrastar_mover()}>⋮⋮</div>
          <!-- IRMÃO do handle (não filho): botão real dentro de role="button" é aninhamento
               interativo inválido (ARIA). Absoluto por cima da faixa; intercepta o ponteiro antes
               do handle, então não dispara drag. -->
          <button class="cv-hide" onclick={() => hide(key)}
                  title={m.board_ocultar_card()} aria-label={`${m.board_ocultar()} ${row.name}`}>−</button>
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
              onOpenArbiter={(n) => onOpenSession(n, row.serverId)}
              onGatherPair={row.pair_gid ? () => gatherPair(key, gkeyOf(row)!) : null}
              onLeavePair={canLeave(row) ? () => arrastarGrupo.pedirSaida({ serverId: row.serverId, name: row.name }) : null}
            />
          </div>
          <!-- Alças de resize: faixas de 6px nas 4 bordas + 12px nos cantos. Decorativas (o teclado
               não redimensiona; posição/tamanho não são conteúdo) -> aria-hidden. -->
          {#each RESIZE_DIRS as dir (dir)}
            <div class="cv-rs cv-rs-{dir}" aria-hidden="true"
                 onpointerdown={(e) => resizeStart(e, key, dir)} onpointermove={resizeMove}
                 onpointerup={resizeEnd} onpointercancel={resizeEnd}></div>
          {/each}
        </div>
      {/if}
    {/each}
    {#if rows.length === 0}
      <p class="cv-empty">{m.board_nenhuma_viva()}</p>
    {:else if visibleRows.length === 0 && collapsedCards.length > 0}
      <!-- Vazio de cards individuais mas COM grupos colapsados na tela: os compactos são o
           conteúdo — nenhuma mensagem (o texto de "ocultos" aqui mentiria sobre a causa). -->
    {:else if visibleRows.length === 0 && focusGid}
      <!-- Vazio POR CAUSA do foco (grupo focado esvaziou/desfez): a instrução certa é sair do
           foco, não o botão «Ocultos». -->
      <button class="cv-empty cv-empty-btn" onclick={() => (focusGid = null)}>
        {m.board_grupo_foco_sem_visiveis()}
      </button>
    {:else if visibleRows.length === 0}
      <p class="cv-empty">{m.board_cards_ocultos()}</p>
    {/if}
  </div>
  </div>
</div>
<div class="cv-zoom" role="group" aria-label={m.canvas_zoom_aria()}>
    <button onclick={fitView} title={m.canvas_ajustar()}>{m.canvas_ajustar()}</button>
    <span class="cv-zoom-sep" aria-hidden="true"></span>
    <button onclick={() => setZoom(zoom - 0.1)} disabled={zoom <= MIN_SCALE} aria-label={m.canvas_reduzir_zoom()}>−</button>
    <span class="cv-zoom-value">{Math.round(zoom * 100)}%</span>
    <button onclick={() => setZoom(zoom + 0.1)} disabled={zoom >= MAX_SCALE} aria-label={m.canvas_aumentar_zoom()}>+</button>
  </div>
</div>

<style>
  /* Canvas livre: scroll nativo nos 2 eixos; o plano inteiro escala, inclusive conexões e cards. Mesmas regras
     visuais do board: cor não tinge fundo, elevação por borda hairline, sem animação nova. */
  .canvas-shell { height: 100%; min-height: 0; position: relative; }
  .canvas { height: 100%; overflow: auto; padding: 0; position: relative; }
  .cv-offline { color: var(--warning); font-size: var(--text-xs); margin: var(--space-2) 24px 0; position: sticky; top: 8px; left: 24px; z-index: 3; }
  .cv-senderr {
    display: block; text-align: left; padding: 0; margin: var(--space-2) 24px 0;
    background: none; border: 0; cursor: pointer; min-height: 0; min-width: 0;
    color: var(--error); font-family: inherit; font-size: var(--text-xs);
    position: sticky; left: 24px; z-index: 3;
  }
  .cv-world { position: relative; }
  .cv-plane { position: relative; transform-origin: top left; transition: transform 180ms var(--ease-out); }
  .cv-links { position: absolute; inset: 0; z-index: 0; overflow: visible; pointer-events: none; }
  .cv-links path { fill: none; stroke-width: 1.5; opacity: 0.72; }
  .cv-links circle { stroke-width: 1.5; }
  .cv-card {
    position: absolute; z-index: 1; display: flex; flex-direction: column;
    overflow: hidden;
    min-width: 240px; min-height: 160px;                 /* espelha MIN_W/MIN_H de canvasLayout */
    border-radius: var(--radius-lg);
  }
  /* Alças de resize: por DENTRO da borda (o card tem overflow: hidden — alça pra fora seria
     recortada). Sem pintura: só cursor e área de agarre.
     A borda de CIMA divide espaço com a barra de arrastar (18px), e como as alças têm z-index
     positivo e a barra não, elas ganham a disputa em qualquer empate — clicar no topo do ⋮⋮
     redimensionava em vez de mover. Por isso a faixa norte é 4px (não 6) e os cantos de cima são
     10px: sobra a barra inteira em volta do ⋮⋮, que é centralizado e fica longe dos cantos. */
  .cv-rs { position: absolute; z-index: 2; touch-action: none; }
  .cv-rs-n, .cv-rs-s { left: 12px; right: 12px; cursor: ns-resize; }
  .cv-rs-n { top: 0; height: 4px; } .cv-rs-s { bottom: 0; height: 6px; }
  .cv-rs-w, .cv-rs-e { top: 12px; bottom: 12px; width: 6px; cursor: ew-resize; }
  .cv-rs-w { left: 0; } .cv-rs-e { right: 0; }
  .cv-rs-nw, .cv-rs-ne { width: 10px; height: 10px; }
  .cv-rs-sw, .cv-rs-se { width: 12px; height: 12px; }
  .cv-rs-nw { top: 0; left: 0; cursor: nwse-resize; }
  .cv-rs-se { bottom: 0; right: 0; cursor: nwse-resize; }
  .cv-rs-ne { top: 0; right: 0; cursor: nesw-resize; }
  .cv-rs-sw { bottom: 0; left: 0; cursor: nesw-resize; }
  .cv-handle {
    position: relative;
    flex-shrink: 0; height: 18px; cursor: grab; touch-action: none;
    display: flex; align-items: center; justify-content: center;
    color: var(--text-muted); font-size: 9px; letter-spacing: 2px; line-height: 1;
    border: 1px solid var(--border-subtle); border-bottom: 0;
    border-radius: var(--radius-lg) var(--radius-lg) 0 0;
    background: var(--surface-card);
    user-select: none;
  }
  .cv-handle:active { cursor: grabbing; }
  /* Ocultar: irmão do handle, ancorado na faixa; aparece no hover do card (descoberta sem poluir
     15 cards com 15 botões). */
  .cv-hide {
    position: absolute; right: 4px; top: 2px; z-index: 3;   /* acima das alças de resize */
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
    background: var(--surface-card); color: var(--text-primary); cursor: pointer;
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
    background: var(--surface-card); color: var(--text-secondary); cursor: pointer;
  }
  .cv-chip:hover { background: var(--bg-hover); color: var(--text-primary); }
  .cv-chip-dot { width: 6px; height: 6px; border-radius: 50%; flex-shrink: 0; }

  /* Membro de grupo: ARO na cor do grupo (--pair-c inline) — pertencimento por CARD, não por
     área. outline não mexe em layout e convive com a borda/resize do card. */
  .cv-card.paired {
    outline: 1.5px solid color-mix(in srgb, var(--pair-c) 55%, transparent);
    outline-offset: 2px;
  }
  /* Alvo do arrasto (Step 3, mesma receita da Sidebar/Board): válido acende a borda de accent;
     recusado avisa sem travar (não é HTML5 dnd, então nada impede soltar — dragEnd reconfere
     canPair antes de abrir o diálogo) — só o motivo, no title do card/card recolhido. */
  .cv-card.drop-alvo, .cv-gcard.drop-alvo { outline: 2px solid var(--accent); outline-offset: -2px; }
  .cv-card.drop-recusado, .cv-gcard.drop-recusado {
    cursor: not-allowed; outline: 2px dashed var(--text-muted); outline-offset: -2px;
  }
  /* Etiqueta do grupo: pill flutuante acima do membro mais alto, com as ações. A etiqueta invade
     ~4px do card de CIMA (GAP de 16 < altura do pill): container com pointer-events none — só os
     BOTÕES capturam clique, a borda do card de cima continua clicável por baixo. */
  .cv-group-tag {
    position: absolute; z-index: 3;
    pointer-events: none;
    display: inline-flex; align-items: center; gap: 2px;
    max-width: 340px;
    background: var(--surface-raised);
    border: 1px solid color-mix(in srgb, currentColor 45%, transparent);
    border-radius: var(--radius-full);
    padding: 1px 4px 1px 10px;
    font-size: var(--text-xs); font-weight: 600;
  }
  .cv-group-tag button { pointer-events: auto; }
  .cv-group-label {
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; margin-right: 4px;
  }
  .cv-group-tag button {
    background: none; border: 0; color: inherit; cursor: pointer;
    font-size: 11px; line-height: 1; padding: 2px 5px; border-radius: var(--radius-full);
    min-height: 0; min-width: 0; opacity: 0.75;
  }
  .cv-group-tag button:hover { opacity: 1; background: color-mix(in srgb, currentColor 14%, transparent); }

  /* Grupo colapsado: card compacto — header expande, linhas abrem o chat do membro. */
  .cv-gcard {
    position: absolute; z-index: 1; display: flex; flex-direction: column;
    background: var(--surface-card);
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
  .cv-body { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  /* O BoardCard interno preenche o corpo (prop fill). flex-shrink não se aplica (absolute),
     mas o min-height: 0 acima é o equivalente aqui: sem ele o body estoura em vez de rolar. */
  .cv-body > :global(.bcard) { flex: 1; min-height: 0; }
  .cv-empty { color: var(--text-muted); font-size: var(--text-xs); padding: var(--space-6); }
  .cv-empty-btn {
    display: block; background: none; border: 0; cursor: pointer; text-align: left;
    font-family: inherit; min-height: 0; min-width: 0;
  }
  .cv-empty-btn:hover { color: var(--text-primary); }
  .cv-zoom {
    position: absolute; right: var(--space-3); bottom: var(--space-3); z-index: 5;
    display: flex; align-items: center; gap: 2px; width: max-content;
    padding: 4px; border: 1px solid var(--border-default); border-radius: var(--radius-md);
    background: var(--bg-elevated); box-shadow: var(--elev-2, 0 8px 28px rgba(0,0,0,.24));
  }
  .cv-zoom button {
    min-width: 32px; min-height: 30px; padding: 0 8px; border: 0; border-radius: var(--radius-sm);
    background: transparent; color: var(--text-secondary); font: inherit; font-size: var(--text-xs); cursor: pointer;
  }
  .cv-zoom button:hover:not(:disabled) { background: var(--bg-hover); color: var(--text-primary); }
  .cv-zoom button:disabled { opacity: .35; cursor: default; }
  .cv-zoom-sep { width: 1px; height: 18px; margin: 0 2px; background: var(--border-subtle); }
  .cv-zoom-value { width: 44px; text-align: center; color: var(--text-primary); font-family: var(--font-mono); font-size: 11px; }
</style>
