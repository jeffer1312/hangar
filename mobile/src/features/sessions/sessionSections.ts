import { agruparSessoes, sortSessions, type AggSession, type GlobalGroup, type GroupBy, type GrupoSessoes } from '@hangar/core';
import { placeOutside } from './outsideRows';
import * as m from '../../paraglide/messages';

export interface SessionSection {
  id: string;
  label: string;
  color: string | null;
  total: number;
  collapsed: boolean;
  data: AggSession[];
  // cabeçalho do bloco de grupo, na chave `<servidor>::<nome>` do primeiro membro
  heads: ReadonlyMap<string, GroupHead>;
}

export type GroupHead = { id: string; label: string; machines: number; size: number; awaiting: number; collapsed: boolean };

// Seção do topo e bloco de grupo valem em qualquer modo de agrupar.
export const sessionGroupKey = (mode: GroupBy, id: string) => id.startsWith('state:') || id.startsWith('pair:') ? id : `${mode}:${id}`;

const rowKey = (r: AggSession) => `${r.serverId}::${r.name}`;

/** O que fazer com as linhas `hidden_from_list`: cada tela escolhe, nenhuma herda sem querer.
 *  `collapsed`/`expanded`: seção "Ocultas (N)" no fim (lista); `exclude`: fora (barra do tablet);
 *  `include`: no lugar de sempre (seletor, que mostra a marca na linha). */
export type HiddenRows = 'collapsed' | 'expanded' | 'exclude' | 'include';
export const HIDDEN_SECTION_ID = 'hidden';

// O grupo é um bloco só, com os membros de todas as máquinas, onde o primeiro deles cairia na lista.
// Recolhido, o bloco guarda só o primeiro membro, que carrega o cabeçalho.
function gatherGroups(sections: GrupoSessoes[], groupOf: ReadonlyMap<string, GlobalGroup>, collapsedGroups: ReadonlySet<string>) {
  const heads = new Map<string, GroupHead>();
  if (!groupOf.size) return { sections, heads };
  const members = new Map<GlobalGroup, AggSession[]>();
  for (const s of sections) for (const r of s.sessions) {
    const g = groupOf.get(rowKey(r));
    if (g) members.set(g, [...members.get(g) ?? [], r]);
  }
  const placed = new Set<GlobalGroup>();
  const out = sections.map((s) => ({ ...s, sessions: s.sessions.flatMap((r) => {
    const g = groupOf.get(rowKey(r));
    if (!g) return [r];
    if (placed.has(g)) return [];
    placed.add(g);
    const list = members.get(g)!;
    // máquinas das linhas mostradas, mais as sem conexão aqui
    const machines = new Set([...list.map((x) => `here:${x.serverId}`), ...g.members.flatMap((mb) => mb.kind === 'missing' ? [`id:${mb.server_id}`] : [])]).size;
    const id = `pair:${g.key}`;
    const collapsed = collapsedGroups.has(id);
    const awaiting = list.filter((x) => x.state === 'awaiting_input').length;
    heads.set(rowKey(r), { id, label: g.label, machines, size: list.length, awaiting, collapsed });
    return collapsed ? [r] : list;
  }) }));
  return { sections: out, heads };
}

export function buildSessionSections(rows: AggSession[], hiddenRows: HiddenRows, mode: GroupBy, search: string, collapsedGroups: ReadonlySet<string>, preferredServerId?: string, pairGroups: GlobalGroup[] = []): SessionSection[] {
  const allLive = rows.filter((r) => r.state !== 'dead');
  // A sessão de fora acompanha a do par: some e volta junto com ela.
  const hiddenKeys = new Set(hiddenRows === 'include' ? [] : allLive.filter((r) => r.hidden_from_list).map(rowKey));
  const isHidden = (r: AggSession) => hiddenKeys.has(rowKey(r)) || (!!r.outside && hiddenKeys.has(`${r.outside.localServerId}::${r.outside.localSession}`));
  const liveRows = hiddenKeys.size ? allLive.filter((r) => !isHidden(r)) : allLive;
  const live = new Set(liveRows.map(rowKey));
  const groupOf = new Map<string, GlobalGroup>();
  for (const g of pairGroups) {
    // membro morto não aparece na lista, então não conta para formar o bloco
    const here = g.members.filter((mb): mb is Extract<typeof mb, { kind: 'here' }> => mb.kind === 'here' && live.has(`${mb.server}::${mb.name}`));
    if (here.length > 1) for (const mb of here) groupOf.set(`${mb.server}::${mb.name}`, g);
  }
  const matches = (r: AggSession) => !search || r.name.toLowerCase().includes(search) || (r.cwd ?? '').toLowerCase().includes(search);
  // achar um membro traz o grupo inteiro
  const found = new Set(liveRows.filter(matches).flatMap((r) => groupOf.get(rowKey(r)) ?? []));
  const visible = liveRows.filter((r) => {
    const g = groupOf.get(rowKey(r));
    return matches(r) || (!!g && found.has(g));
  });
  const own = visible.filter((r) => !r.outside);
  const top = [
    { id: 'state:awaiting_input', label: m.home_sessions_awaiting(), color: null, sessions: sortSessions(own.filter((r) => r.state === 'awaiting_input')) },
    { id: 'state:working', label: m.home_sessions_working(), color: null, sessions: sortSessions(own.filter((r) => r.state === 'working')) },
  ];
  const rest = agruparSessoes(own.filter((r) => r.state !== 'awaiting_input' && r.state !== 'working'), mode);
  if (mode === 'server') rest.sort((a, b) => a.label.localeCompare(b.label));
  if (mode === 'none' && top.some((g) => g.sessions.length)) rest[0].label = m.home_sessions_ready();
  // Buscando, o bloco abre: o membro achado pode não ser o primeiro.
  const gathered = gatherGroups([...top, ...rest], groupOf, search ? new Set() : collapsedGroups);
  const groups = placeOutside(gathered.sections, visible.filter((r) => r.outside), mode, liveRows);
  if (mode === 'server' && preferredServerId) {
    const rank = (id: string) => id === 'state:awaiting_input' ? 0 : id === 'state:working' ? 1 : 2;
    groups.sort((a, b) => rank(a.id) - rank(b.id)
      || Number(b.id === preferredServerId) - Number(a.id === preferredServerId) || a.label.localeCompare(b.label));
  }
  const out: SessionSection[] = groups.filter((g) => g.sessions.length).map((g) => {
    const collapsed = !!g.label && collapsedGroups.has(sessionGroupKey(mode, g.id));
    // o grupo recolhido ainda conta todos os membros
    const total = g.sessions.reduce((n, r) => { const h = gathered.heads.get(rowKey(r)); return n + (h?.collapsed ? h.size : 1); }, 0);
    return { id: g.id, label: g.label, color: g.color, total, collapsed, data: collapsed ? [] : g.sessions, heads: gathered.heads };
  });
  if (hiddenRows !== 'collapsed' && hiddenRows !== 'expanded') return out;
  const hidden = allLive.filter((r) => isHidden(r) && matches(r));
  if (!hidden.length) return out;
  // Lista corrida de todas as máquinas, sem blocos de grupo; buscando, a seção abre como os grupos.
  const data = placeOutside([{ id: HIDDEN_SECTION_ID, label: '', color: null, sessions: sortSessions(hidden.filter((r) => !r.outside)) }],
    hidden.filter((r) => r.outside), mode, hidden)[0].sessions;
  const collapsed = hiddenRows === 'collapsed' && !search;
  return [...out, { id: HIDDEN_SECTION_ID, label: m.session_hidden_group({ n: data.length }), color: null, total: data.length,
    collapsed, data: collapsed ? [] : data, heads: new Map() }];
}
