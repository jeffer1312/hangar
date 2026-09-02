// frontend/src/lib/sessionListModel.svelte.ts
// Lógica da lista de sessões compartilhada pelas duas views (Sidebar desktop, SessionList
// celular). Só lógica: template e CSS continuam em cada view. Formato do sessionsStore — fábrica
// com getters, sem destructuring (perderia a reatividade).
import { broadcast } from './api';
import { getActiveId, selectServer, serverColor } from './auth';
import { sessionsStore } from './sessionsStore.svelte';
import {
  countAwaiting, effectiveGroupBy, groupSelectedByServer, projectKey, projectLabel, providerName,
  sortSessions, type GroupBy,
} from './format';
import type { AggSession } from './types';
import * as m from '../paraglide/messages';

export type ListVariant = 'desktop' | 'mobile';

export interface Group {
  id: string;
  label: string;
  color: string | null;
  error: string | null;
  sessions: AggSession[];
}

export interface SessionListModelOptions {
  variant: ListVariant;
  onOpen: (name: string) => void;
  onCompare: (ids: { serverId: string; name: string }[]) => void;
  // Só o desktop tem sessão aberta ao lado da lista: renomeá-la troca a rota.
  currentSession?: () => string | null;
}

interface Rules {
  readGroupBy: (stored: GroupBy) => GroupBy;
  groupMode: (pref: GroupBy, serverCount: number) => GroupBy;
  serverGroups: 'store-order' | 'alpha-non-empty';
  filterLabel: (s: AggSession, g: Group) => string;
  compareOrder: 'groups' | 'rows';
  restoreServerAfterAction: boolean;
}

// Onde as duas views divergem, uma regra por linha. Convergir é trocar um valor aqui, de
// propósito — nunca por acidente numa refatoração.
const RULES: Record<ListVariant, Rules> = {
  desktop: {
    readGroupBy: (v) => v,
    groupMode: effectiveGroupBy,
    serverGroups: 'store-order',
    filterLabel: (_s, g) => g.label,
    compareOrder: 'groups',
    restoreServerAfterAction: true,
  },
  mobile: {
    // O toggle do celular só conhece Servidor|Projeto: 'none' gravado pelo desktop lê como
    // 'server' (é o que marca o botão certo).
    readGroupBy: (v) => (v === 'project' ? 'project' : 'server'),
    groupMode: (pref, n) => (pref === 'project' ? 'project' : n > 1 ? 'server' : 'none'),
    serverGroups: 'alpha-non-empty',
    filterLabel: (s) => s.serverLabel,
    compareOrder: 'rows',
    restoreServerAfterAction: false,
  },
};

const GROUP_BY_KEY = 'cp_group_by';
const COLLAPSE_KEY = 'cp_collapsed_servers';
const FILTER_FROM = 6;

function loadGroupBy(): GroupBy {
  const v = localStorage.getItem(GROUP_BY_KEY);
  return v === 'project' || v === 'none' || v === 'server' ? v : 'server';
}
function loadCollapsed(): Set<string> {
  try { return new Set(JSON.parse(localStorage.getItem(COLLAPSE_KEY) ?? '[]')); } catch { return new Set(); }
}
// Chave de seleção "<serverId>:<name>" — a mesma string que os templates montam à mão; fica
// interna pra não colidir com o `{@const selKey}` da linha do desktop.
const selectionKey = (s: { serverId: string; name: string }) => `${s.serverId}:${s.name}`;

export function createSessionListModel(opts: SessionListModelOptions) {
  const rules = RULES[opts.variant];

  let groupBy = $state<GroupBy>(rules.readGroupBy(loadGroupBy()));
  let collapsed = $state<Set<string>>(loadCollapsed());
  let filterText = $state('');

  const rows = $derived(sessionsStore.rows);
  const servers = $derived(sessionsStore.servers);
  const groupMode = $derived(rules.groupMode(groupBy, servers.length));

  const allGroups = $derived.by<Group[]>(() => {
    if (servers.length === 0) return [];
    if (groupMode === 'none') {
      return [{ id: '*', label: '', color: null, error: null, sessions: sortSessions([...rows]) }];
    }
    if (groupMode === 'project') {
      const byKey = new Map<string, AggSession[]>();
      for (const s of rows) {
        const k = projectKey(s.cwd);
        const arr = byKey.get(k);
        if (arr) arr.push(s); else byKey.set(k, [s]);
      }
      return [...byKey.entries()]
        .map(([id, list]) => ({ id, label: projectLabel(list[0]?.cwd), color: null, error: null, sessions: sortSessions(list) }))
        .sort((a, b) => a.label.localeCompare(b.label));
    }
    // "offline" só quando NÃO há lista: com lista stale o grupo mostra as sessões, não o aviso.
    const byServer: Group[] = sessionsStore.byServer.map((b) => ({
      id: b.server.id,
      label: b.server.label,
      color: serverColor(b.server.id),
      error: b.loaded ? null : b.error,
      sessions: sortSessions(b.sessions),
    }));
    if (rules.serverGroups === 'store-order') return byServer;
    return byServer.filter((g) => g.sessions.length > 0).sort((a, b) => a.label.localeCompare(b.label));
  });

  const groups = $derived.by<Group[]>(() => {
    const q = filterText.trim().toLowerCase();
    if (!q) return allGroups;
    return allGroups
      .map((g) => ({
        ...g,
        sessions: g.sessions.filter(
          (s) =>
            s.name.toLowerCase().includes(q) ||
            (s.cwd ?? '').toLowerCase().includes(q) ||
            rules.filterLabel(s, g).toLowerCase().includes(q),
        ),
      }))
      .filter((g) => g.sessions.length > 0);
  });
  const flatRows = $derived(groups.flatMap((g) => g.sessions));
  // Contagens e broadcast saem dos GRUPOS (pré-filtro), como o desktop já fazia: no store real
  // é o mesmo conjunto das rows, mas o Sidebar.test só preenche byServer.
  const allSessions = $derived(allGroups.flatMap((g) => g.sessions));
  const showFilter = $derived(allSessions.length > FILTER_FROM);
  const filterEmpty = $derived(filterText.trim() !== '' && groups.length === 0);
  const showProviderTags = $derived(new Set(rows.map((s) => providerName(s.provider))).size > 1);
  const awaitingTotal = $derived(countAwaiting(allSessions));

  function setGroupBy(mode: GroupBy) {
    groupBy = mode;
    try { localStorage.setItem(GROUP_BY_KEY, mode); } catch { /* storage cheio/off */ }
  }
  function toggleGroup(id: string) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id); else next.add(id);
    collapsed = next;
    // O colapso de cluster de pareamento ('pair:<gid>') é efêmero: o gid renasce a cada
    // pareamento, e gravá-lo acumularia lixo pra sempre.
    try { localStorage.setItem(COLLAPSE_KEY, JSON.stringify([...next].filter((k) => !k.startsWith('pair:')))); } catch { /* idem */ }
  }

  // ── Seleção múltipla: broadcast (1 prompt pra N sessões) e comparar (grade lado a lado) ──
  let selectMode = $state(false);
  let selected = $state<Set<string>>(new Set());
  let broadcastText = $state('');
  let broadcastBusy = $state(false);
  let broadcastMsg = $state('');
  // Slash-command é roteado por sessão: replicar "/clear" pra N sessões de uma vez seria perigoso.
  const broadcastIsSlash = $derived(broadcastText.trim().startsWith('/'));
  const broadcastDisabled = $derived(broadcastBusy || selected.size === 0 || !broadcastText.trim() || broadcastIsSlash);
  const compareDisabled = $derived(selected.size < 2);

  function toggleSelectMode() {
    selectMode = !selectMode;
    selected = new Set();
    broadcastText = '';
    broadcastMsg = '';
  }
  function openSelectMode() { if (!selectMode) toggleSelectMode(); }
  function toggleSelected(key: string) {
    const next = new Set(selected);
    if (next.has(key)) next.delete(key); else next.add(key);
    selected = next;
  }
  function selectGroupForBroadcast(g: Group) {
    selectMode = true;
    selected = new Set(g.sessions.filter((s) => s.tracked !== false).map(selectionKey));
  }
  function openCompare() {
    const order = rules.compareOrder === 'groups' ? allSessions : rows;
    opts.onCompare(order.filter((s) => selected.has(selectionKey(s))).map((s) => ({ serverId: s.serverId, name: s.name })));
  }
  async function sendBroadcast() {
    const text = broadcastText.trim();
    if (broadcastDisabled) return;
    broadcastBusy = true;
    broadcastMsg = '';
    const byServer = groupSelectedByServer(allSessions, selected);
    const prev = getActiveId();
    const failed: string[] = [];
    for (const [serverId, names] of byServer) {
      selectServer(serverId);
      try {
        const results = await broadcast(names, text);
        for (const [n, r] of Object.entries(results)) if (!r.ok) failed.push(n);
      } catch {
        failed.push(...names);   // servidor offline/erro de rede: o lote inteiro dele falhou
      }
    }
    if (prev) selectServer(prev);
    broadcastBusy = false;
    if (failed.length) {
      broadcastMsg = m.lista_broadcast_falha({ nomes: failed.join(', ') });
    } else {
      broadcastText = '';
      selected = new Set();
      selectMode = false;
    }
  }

  return {
    mount() { sessionsStore.retain(); return () => sessionsStore.release(); },
    get rows() { return rows; },
    get servers() { return servers; },
    get groupBy() { return groupBy; },
    get groupMode() { return groupMode; },
    get collapsed() { return collapsed; },
    get filterText() { return filterText; },
    set filterText(v: string) { filterText = v; },
    get showFilter() { return showFilter; },
    get filterEmpty() { return filterEmpty; },
    get allGroups() { return allGroups; },
    get allSessions() { return allSessions; },
    get groups() { return groups; },
    get flatRows() { return flatRows; },
    get showProviderTags() { return showProviderTags; },
    get awaitingTotal() { return awaitingTotal; },
    setGroupBy,
    toggleGroup,
    get selectMode() { return selectMode; },
    get selected() { return selected; },
    get broadcastText() { return broadcastText; },
    set broadcastText(v: string) { broadcastText = v; },
    get broadcastBusy() { return broadcastBusy; },
    get broadcastMsg() { return broadcastMsg; },
    get broadcastIsSlash() { return broadcastIsSlash; },
    get broadcastDisabled() { return broadcastDisabled; },
    get compareDisabled() { return compareDisabled; },
    toggleSelectMode, openSelectMode, toggleSelected, selectGroupForBroadcast, openCompare, sendBroadcast,
  };
}
