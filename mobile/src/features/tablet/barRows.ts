import { groupGlobal, isOrq, type AggSession } from '@hangar/core';
import { buildSessionSections } from '../sessions/sessionSections';

export type BarItem = { kind: 'single'; session: AggSession } | { kind: 'group'; key: string; label: string | null; members: AggSession[] };

const keyOf = (s: AggSession) => `${s.serverId}::${s.name}`;
export const isWorking = (s: AggSession) => s.state === 'working' && !s.limited;
type TabOrder = { tabsWorkingFirst?: boolean; tabsRecentFirst?: boolean };

export function barRows(rows: AggSession[], identities: ReadonlyMap<string, string>, groupsFirst = true, order: TabOrder = {}): BarItem[] {
  const ordered = buildSessionSections(rows, 'none', '', new Set()).flatMap(s => s.data);
  const byKey = new Map(ordered.map(s => [keyOf(s), s]));
  type Group = { key: string | null; members: Set<AggSession> };
  const groupOf = new Map<AggSession, Group>();
  const join = (members: AggSession[], key: string | null = null) => {
    const group: Group = { key, members: new Set(members) };
    for (const member of members) {
      const existing = groupOf.get(member);
      if (!existing) continue;
      if (existing.key && (!group.key || existing.key < group.key)) group.key = existing.key;
      for (const s of existing.members) group.members.add(s);
    }
    for (const member of group.members) groupOf.set(member, group);
  };
  for (const group of groupGlobal(ordered, identities)) {
    const members = group.members.flatMap(m => m.kind === 'here' ? byKey.get(`${m.server}::${m.name}`) ?? [] : []);
    if (members.length > 1) join(members, group.key);
  }
  for (const s of ordered) {
    // O vínculo é local; nomes iguais noutra máquina não são a mãe da filha.
    const parent = s.linked_by ? byKey.get(`${s.serverId}::${s.linked_by}`) : undefined;
    const anchor = s.outside ? byKey.get(`${s.outside.localServerId}::${s.outside.localSession}`) : undefined;
    if (parent) join([parent, s]);
    if (anchor) join([anchor, s]);
  }
  const members = new Map<Group, AggSession[]>();
  for (const s of ordered) {
    const group = groupOf.get(s);
    if (group) {
      const list = members.get(group) ?? [];
      list.push(s);
      members.set(group, list);
    }
  }
  const placed = new Set<Group>();
  const items = ordered.flatMap((session): BarItem[] => {
    const group = groupOf.get(session);
    if (!group || group.members.size < 2) return [{ kind: 'single', session }];
    if (placed.has(group)) return [];
    placed.add(group);
    const list = members.get(group)!;
    const opens = (parent: AggSession, child: AggSession) => child.serverId === parent.serverId && child.linked_by === parent.name;
    const opener = (s: AggSession) => list.some(child => opens(s, child));
    const opened = (s: AggSession) => list.some(parent => opens(parent, s));
    // Como no nativo: o árbitro, depois quem formou o grupo; numa cadeia de filhas, quem abriu a primeira.
    const formed = list.find(s => list.some(o => o.serverId === s.serverId && o.pair_lead === s.name));
    const mother = list.find(isOrq) ?? formed ?? list.find(s => opener(s) && !opened(s)) ?? list.find(opener);
    const sorted = mother ? [mother, ...list.filter(s => s !== mother)] : list;
    const key = group.key ?? (mother ? `mother:${keyOf(mother)}` : `members:${JSON.stringify(list.map(keyOf).sort())}`);
    const label = sorted.map(s => s.pair_task?.trim()).find(Boolean) || null;
    return [{ kind: 'group', key, label, members: sorted }];
  });
  if (order.tabsWorkingFirst || order.tabsRecentFirst) {
    const section = (item: BarItem) => item.kind === 'group' ? 0 : isWorking(item.session) ? 1 : 2;
    const recency = (item: BarItem) => {
      const at = order.tabsRecentFirst && item.kind === 'single' && section(item) === 2 ? item.session.last_reply_at ?? 0 : 0;
      return Number.isFinite(at) ? Math.max(0, at) : 0;
    };
    return items.sort((a, b) => section(a) - section(b) || recency(b) - recency(a));
  }
  return groupsFirst ? items.sort((a, b) => Number(b.kind === 'group') - Number(a.kind === 'group')) : items;
}
