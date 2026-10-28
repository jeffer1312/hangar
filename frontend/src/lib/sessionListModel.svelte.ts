// frontend/src/lib/sessionListModel.svelte.ts
// Lógica da lista de sessões compartilhada pelas duas views (Sidebar desktop, SessionList
// celular). Só lógica: template e CSS continuam em cada view. Formato do sessionsStore — fábrica
// com getters, sem destructuring (perderia a reatividade).
import { broadcast, deleteSession, renameSession, resumeSession } from '@hangar/core';
import { getActiveId, selectServer, serverColor } from './auth';
import { formataErro } from '@hangar/core';
import { sessionsStore } from './sessionsStore.svelte';
import {
  countAwaiting, effectiveGroupBy, groupBadges, groupSelectedByServer, projectKey, projectLabel, providerName,
  isOrq, sortSessions, type GroupBy, placeExternalRows, pairExternals, pairHeaderLabel, groupGlobal, clusterByPair,
} from '@hangar/core';
import type { AggSession, ResumeCandidate, State } from '@hangar/core';
import * as m from '../paraglide/messages';

const errMsg = (e: unknown) => (e instanceof Error ? e.message : String(e));

export type ListVariant = 'desktop' | 'mobile';

// O rótulo do grupo pareado é o resumo do ticket inteiro ("ABC-1234 Assunto comprido do
// chamado…"). A chave sozinha identifica; o resto é assunto e vai em cinza, sem competir com a
// lista. Mora aqui porque as duas views desenham o mesmo cabeçalho.
const PAIR_COD = /^([A-Za-z][\w.]*-\d+)\b\s*(.*)$/;

export function pairCodigo(label: string): string {
  return PAIR_COD.exec(label)?.[1] ?? label;
}

export function pairResto(label: string): string {
  return PAIR_COD.exec(label)?.[2] ?? '';
}

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
  // Sessão de fora (par externo) entra no grupo local; a vista desktop web não ganha recurso novo.
  outsideRows: 'place' | 'hide';
  // Grupo de trabalho vira um bloco só, com os membros de todas as máquinas, onde o 1º apareceria.
  gatherGroups: boolean;
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
    outsideRows: 'hide',
    gatherGroups: false,
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
    outsideRows: 'place',
    gatherGroups: true,
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

  // Entradas temporárias de par externo nunca viram seção nem contam como servidor.
  const placed = $derived.by(() => {
    const agg = placeExternalRows({ rows: sessionsStore.rows, byServer: sessionsStore.byServer });
    if (rules.outsideRows === 'place') return agg;
    const local = (r: AggSession) => !r.outside;
    return { rows: agg.rows.filter(local), byServer: agg.byServer.map((b) => ({ ...b, sessions: b.sessions.filter(local) })) };
  });
  const rows = $derived(placed.rows);
  const servers = $derived(sessionsStore.servers.filter((s) => !s.ephemeral));
  const groupMode = $derived(rules.groupMode(groupBy, servers.length));

  // gid de cada grupo global: o par antigo entre máquinas tem um gid de cada lado, e os dois passam
  // a responder pelo gid do primeiro membro.
  const canonGid = $derived.by(() => {
    const map = new Map<string, string>();
    if (!rules.gatherGroups) return map;
    const byKey = new Map(rows.map((r) => [`${r.serverId}::${r.name}`, r]));
    for (const g of groupGlobal(rows, sessionsStore.identities ?? new Map())) {
      const gids = g.members.flatMap((mb) => mb.kind === 'here' ? [byKey.get(`${mb.server}::${mb.name}`)?.pair_gid] : [])
        .filter((x): x is string => !!x);
      for (const gid of gids) if (!map.has(gid)) map.set(gid, gids[0]);
    }
    return map;
  });
  const blockOf = (s: AggSession): string | null => (s.pair_gid ? canonGid.get(s.pair_gid) ?? s.pair_gid : null);

  // Os membros de cada grupo saem das outras seções e entram juntos onde o primeiro estava.
  function gather(sections: Group[]): Group[] {
    const members = new Map<string, AggSession[]>();
    for (const g of sections) for (const s of g.sessions) {
      const k = blockOf(s);
      if (k) members.set(k, [...(members.get(k) ?? []), s]);
    }
    const placedGroups = new Set<string>();
    return sections
      .map((g) => ({ ...g, sessions: g.sessions.flatMap((s) => {
        const k = blockOf(s);
        if (!k) return [s];
        if (placedGroups.has(k)) return [];
        placedGroups.add(k);
        return members.get(k)!;
      }) }))
      // Seção esvaziada pelo bloco sai, menos a que carrega erro do servidor: o erro tem que aparecer.
      .filter((g, i) => g.sessions.length > 0 || sections[i].sessions.length === 0 || !!g.error);
  }

  const allGroups = $derived.by<Group[]>(() => {
    const sections = groupSections();
    return rules.gatherGroups ? gather(sections) : sections;
  });

  function groupSections(): Group[] {
    if (servers.length === 0) return [];
    if (groupMode === 'none') {
      return [{ id: '*', label: '', color: null, error: null, sessions: sortSessions(rows) }];
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
    const byServer: Group[] = placed.byServer.map((b) => ({
      id: b.server.id,
      label: b.label ?? b.server.label,
      color: b.color ?? serverColor(b.server.id),
      error: b.error,
      sessions: sortSessions(b.sessions),
    }));
    if (rules.serverGroups === 'store-order') return byServer;
    return [...byServer.filter((g) => g.sessions.length > 0).sort((a, b) => a.label.localeCompare(b.label))];
  }

  const groups = $derived.by<Group[]>(() => {
    const q = filterText.trim().toLowerCase();
    if (!q) return allGroups;
    return allGroups
      .map((g) => {
        const matches = (s: AggSession) => s.name.toLowerCase().includes(q) ||
          (s.cwd ?? '').toLowerCase().includes(q) || rules.filterLabel(s, g).toLowerCase().includes(q);
        // No celular o bloco do grupo não se parte: um membro que casa traz os outros.
        const hit = new Set(rules.gatherGroups ? g.sessions.filter(matches).map(blockOf).filter(Boolean) : []);
        return { ...g, sessions: g.sessions.filter((s) => matches(s) || hit.has(blockOf(s))) };
      })
      .filter((g) => g.sessions.length > 0);
  });
  const flatRows = $derived(groups.flatMap((g) => g.sessions));
  // Contagens e broadcast saem dos GRUPOS (pré-filtro), como o desktop já fazia: no store real
  // é o mesmo conjunto das rows, mas o Sidebar.test só preenche byServer.
  // No celular isso também significa visitar os servidores na ordem dos grupos (alfabética), não
  // na ordem crua das rows — mesmo conjunto, mensagem de falha pode listar os nomes fora de ordem.
  const allSessions = $derived(allGroups.flatMap((g) => g.sessions));
  const showFilter = $derived(allSessions.length > FILTER_FROM);
  const filterEmpty = $derived(filterText.trim() !== '' && groups.length === 0);
  // A linha do orquestrador não é um agente: sozinha, ela faria toda linha Claude ganhar o glifo.
  const showProviderTags = $derived(new Set(rows.filter((s) => !isOrq(s)).map((s) => providerName(s.provider))).size > 1);
  const awaitingTotal = $derived(countAwaiting(allSessions));

  function pairMembers(gid: string): AggSession[] {
    const k = canonGid.get(gid) ?? gid;
    return allSessions.filter(s => blockOf(s) === k);
  }
  const cluster = (sessions: AggSession[]) => clusterByPair(sessions, blockOf);

  // Selo do grupo que atravessa máquinas (I-9A): o código do ticket ou quantas máquinas o grupo
  // tem. Grupo de uma máquina só já tem o cabeçalho.
  const badges = $derived(groupBadges(rows, sessionsStore.identities ?? new Map()));
  function groupBadge(s: AggSession): string | null {
    const badge = badges.get(`${s.serverId}::${s.name}`);
    return badge ? badge.text ?? m.grupo_selo_maquinas({ n: badge.machines }) : null;
  }
  // Cabeçalho do bloco: "N máquinas" quando o grupo atravessa máquinas.
  function pairSeal(gid: string): string | null {
    const n = pairMembers(gid).map((s) => badges.get(`${s.serverId}::${s.name}`)?.machines ?? 0).find((x) => x > 1);
    return n ? m.grupo_selo_maquinas({ n }) : null;
  }
  // Dentro do bloco a seção já não diz a máquina: a linha de grupo entre máquinas diz.
  function memberMachine(s: AggSession): string | null {
    return badges.has(`${s.serverId}::${s.name}`) ? s.serverLabel : null;
  }

  // Cabeçalho do grupo com sessão de fora: "hangar ↔ casa::api (Ana)", só com os membros locais.
  function pairLabel(gid: string, label: string): string {
    const local = pairMembers(gid).filter((s) => !s.outside);
    if (!local.some((s) => pairExternals(s).length > 0)) return label;
    const task = local.map((s) => s.pair_task?.trim()).find(Boolean);
    return pairHeaderLabel(task || local.map((s) => s.name).join(', '), local);
  }

  function setGroupBy(mode: GroupBy) {
    groupBy = mode;
    try { localStorage.setItem(GROUP_BY_KEY, mode); } catch { /* storage cheio/off */ }
  }
  function toggleGroup(id: string) {
    const next = new Set(collapsed);
    if (next.has(id)) next.delete(id); else next.add(id);
    collapsed = next;
    // O colapso de cluster de pareamento ('pair:<gid>') é gravado enquanto o grupo existir na
    // lista: no celular a lista desmonta ao abrir um chat, e sem gravar o grupo voltava aberto a
    // cada volta. O gid renasce a cada pareamento, então o que não está mais na lista é podado
    // aqui mesmo, e o localStorage não acumula gid morto.
    const vivos = new Set(rows.map((s) => `pair:${blockOf(s) ?? ''}`));
    try {
      localStorage.setItem(COLLAPSE_KEY, JSON.stringify([...next].filter((k) => !k.startsWith('pair:') || vivos.has(k))));
    } catch { /* idem */ }
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
    // Orquestrador não recebe mensagem: fora do envio em massa em qualquer lista.
    if (allSessions.some((s) => isOrq(s) && selectionKey(s) === key)) return;
    const next = new Set(selected);
    if (next.has(key)) next.delete(key); else next.add(key);
    selected = next;
  }
  function selectGroupForBroadcast(g: Group) {
    selectMode = true;
    selected = new Set(g.sessions.filter((s) => s.tracked !== false && !isOrq(s)).map(selectionKey));
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

  // ── Ações sobre uma sessão ──
  // Restaurar o servidor ativo depois da ação é regra por view (C1 no desktop: o chat aberto tem
  // que continuar no servidor dele; o celular nunca restaurou).
  function restore(prev: string | null, serverId: string) {
    if (rules.restoreServerAfterAction && prev && prev !== serverId) selectServer(prev);
  }

  // Sem id confiável não abre. DUAS exceções, pelo mesmo motivo: "sem id" ali é o estado normal de
  // antes de a conversa existir, não um defeito.
  // - Kimi: o id nasce no 1º prompt, e o /input não depende de jsonl — bloquear impedia a sessão
  //   de nascer.
  // - Codex: o id vem da thread, e a TUI pode estar parada num prompt DELA (a aprovação dos hooks,
  //   por exemplo). Bloquear trancava a única saída: com o card fechado, o terminal — que só
  //   precisa do nome no tmux — ficava inalcançável pelo celular, e a sessão não tinha como sair
  //   do lugar sem um `tmux attach` na máquina.
  function open(s: { name: string; serverId: string; tracked?: boolean; provider?: AggSession['provider'] }): boolean {
    if (s.tracked === false && s.provider !== 'kimi' && s.provider !== 'codex') return false;
    selectServer(s.serverId);   // o Chat usa o servidor ativo
    opts.onOpen(s.name);
    return true;
  }

  let confirmDel = $state<{ name: string; serverId: string; state: State | null } | null>(null);
  function requestDelete(name: string, serverId: string, state: State | null = null) {
    confirmDel = { name, serverId, state };
  }
  async function doDelete(): Promise<{ ok: boolean; erro: string }> {
    if (!confirmDel) return { ok: false, erro: '' };
    const { name, serverId } = confirmDel;
    confirmDel = null;
    const prev = getActiveId();
    selectServer(serverId);
    // Otimista: some na hora; falhou, desmarca (reaparece) e a view mostra o erro.
    sessionsStore.markDeleting(serverId, name);
    let result = { ok: true, erro: '' };
    try {
      const res = await deleteSession(name);
      // Sessão morreu, mas um companheiro do grupo não foi avisado: a view mostra o motivo em vez
      // de fechar mudo (mesmo tratamento do PairSheet com res.warning).
      if (res.warning) result = { ok: true, erro: formataErro(res.warning) ?? String(res.warning) };
    }
    catch (e) { sessionsStore.unmarkDeleting(serverId, name); result = { ok: false, erro: errMsg(e) }; }
    restore(prev, serverId);
    return result;
  }

  async function rename(nv: string, old: string, serverId: string): Promise<{ ok: boolean; name: string; erro: string }> {
    const prev = getActiveId();
    selectServer(serverId);
    try {
      const r = await renameSession(old, nv);
      if (old === opts.currentSession?.()) opts.onOpen(r.name);
      return { ok: true, name: r.name, erro: '' };
    } catch (e) {
      return { ok: false, name: old, erro: e instanceof Error ? e.message : m.sessao_falha_renomear() };
    } finally {
      restore(prev, serverId);   // o SSE re-emite a sessão renomeada
    }
  }

  let resumeCandidates = $state<{ name: string; serverId: string; candidates: ResumeCandidate[] } | null>(null);
  let resumeBusy = $state('');
  let resumeError = $state('');
  async function resume(name: string, serverId: string, sessionId?: string): Promise<{ ok: boolean; ambiguous: boolean; erro: string }> {
    resumeError = '';
    resumeBusy = name;
    const prev = getActiveId();
    selectServer(serverId);
    try {
      const r = await resumeSession(name, sessionId);
      if (r && 'ambiguous' in r && r.ambiguous) {
        resumeCandidates = { name, serverId, candidates: r.candidates };
        return { ok: true, ambiguous: true, erro: '' };
      }
      resumeCandidates = null;   // religada; o SSE atualiza a linha
      return { ok: true, ambiguous: false, erro: '' };
    } catch (err) {
      resumeError = err instanceof Error ? err.message : m.sessao_falha_retomar();
      return { ok: false, ambiguous: false, erro: resumeError };
    } finally {
      resumeBusy = '';
      restore(prev, serverId);
    }
  }

  // Git/Loop miram o servidor ATIVO (api.ts): aponta pro dono enquanto a folha está aberta e
  // restaura no fechar — nas duas views, sem regra.
  let gitSheet = $state<{ name: string } | null>(null);
  let gitPrev: string | null = null;
  function openGit(name: string, serverId: string) { gitPrev = getActiveId(); selectServer(serverId); gitSheet = { name }; }
  function closeGit() { gitSheet = null; if (gitPrev) { selectServer(gitPrev); gitPrev = null; } }

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
    pairMembers,
    pairLabel,
    groupBadge,
    pairSeal,
    memberMachine,
    cluster,
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
    open, requestDelete, doDelete, rename, resume, openGit, closeGit,
    get confirmDel() { return confirmDel; },
    set confirmDel(v: { name: string; serverId: string; state: State | null } | null) { confirmDel = v; },
    get resumeCandidates() { return resumeCandidates; },
    set resumeCandidates(v: { name: string; serverId: string; candidates: ResumeCandidate[] } | null) { resumeCandidates = v; },
    get resumeBusy() { return resumeBusy; },
    get resumeError() { return resumeError; },
    get gitSheet() { return gitSheet; },
  };
}
