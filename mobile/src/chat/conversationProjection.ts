import {
  agruparConversa, entraNoPensamento, foldTasks, hiddenByQueuePanel, htmlPageFromResult,
  isHtmlRenderTool, type ChatEvent, type PensamentoTools, type QueuePanel,
} from '@hangar/core';
import { accumulateFoldSummary, appendFoldSummary, foldConversation, updateFoldSummary, type ConversationRow, type FoldSummary } from './tools/fold';

export type MessageRow = ConversationRow & {
  dayTs?: number;
  summary?: FoldSummary;
  resultOf?: (ev: ChatEvent) => ChatEvent | null;
};
export interface ProjectionInput {
  events: ChatEvent[];
  eventsRevision?: number;
  pref: PensamentoTools;
  merge: boolean;
  terminal?: boolean;
  /** Claude Desktop: toda sequência vira grupo, como no Terminal. */
  desktop?: boolean;
  tasksEnabled: boolean;
  pagesEnabled: boolean;
  running: ReadonlySet<string>;
  queuePanel?: QueuePanel;
}
interface GroupState {
  mode: 'run' | 'thought' | null;
  count: number;
  first: string;
}
export interface ConversationProjection {
  events: ChatEvent[];
  revision?: number;
  config: string;
  rows: MessageRow[];
  data: MessageRow[];
  results: Map<string, ChatEvent>;
  tools: Map<string, ChatEvent[]>;
  toolRows: Map<ChatEvent, number>;
  eventRows: Map<string, number>;
  starts: Map<string, number | null>;
  group: GroupState;
  lastDay: string;
  lastSent: number | null;
  taskEvents: ChatEvent[];
  tasks: ReturnType<typeof foldTasks>;
  taskAnchor: string | null;
  lastAnchor: string | null;
  seenIds: Set<string>;
  duplicateIds: boolean;
}

const isTask = (ev: ChatEvent) => ev.kind === 'tool_use' && (ev.tool_name === 'TaskCreate' || ev.tool_name === 'TaskUpdate');
const visible = (ev: ChatEvent) => ev.kind === 'user_msg' ? !!ev.text || !!ev.image_count : ev.kind !== 'assistant_msg' || !!ev.text;
const resultOf = (results: Map<string, ChatEvent>) => (ev: ChatEvent) => results.get(ev.tool_use_id ?? '') ?? null;
const emptyGroup = (): GroupState => ({ mode: null, count: 0, first: '' });

function configKey(input: ProjectionInput): string {
  return JSON.stringify([input.pref, input.merge, !!input.terminal, !!input.desktop, input.tasksEnabled, input.pagesEnabled,
    [...input.running].sort(), input.queuePanel?.entries.map((e) => e.id).sort() ?? []]);
}

function sameRow(a: MessageRow, b: MessageRow, results: Map<string, ChatEvent>, before: Map<string, ChatEvent>): boolean {
  if (a.type !== b.type || a.dayTs !== b.dayTs) return false;
  if (a.type === 'event') return b.type === 'event' && a.ev === b.ev;
  if (a.type === 'tasks') return b.type === 'tasks' && a.tasks === b.tasks;
  if (a.type === 'page') return b.type === 'page' && a.ev === b.ev
    && results.get(a.ev.tool_use_id ?? '') === before.get(a.ev.tool_use_id ?? '');
  return b.type === 'fold' && a.source === b.source && a.parts.length === b.parts.length
    && a.parts.every((ev, i) => ev === b.parts[i]
      && results.get(ev.tool_use_id ?? '') === before.get(ev.tool_use_id ?? ''));
}

/** A revisão igual autoriza apenas a cauda; sem ela, um array novo é uma substituição. */
export function projectConversation(before: ConversationProjection | null, input: ProjectionInput): ConversationProjection {
  const everyRun = !!input.terminal || !!input.desktop;
  const groupMin = everyRun ? 1 : 3;
  const config = configKey(input);
  if (before && before.events === input.events && before.revision === input.eventsRevision && before.config === config) return before;
  const append = !!before && before.config === config && input.eventsRevision !== undefined
    && before.revision === input.eventsRevision && input.events.length >= before.events.length;
  if (append && before.events.length === input.events.length) return { ...before, events: input.events };
  const start = append ? before.events.length : 0;
  // Só os recipientes são copiados: eventos e linhas já publicados nunca são alterados.
  const next: ConversationProjection = append ? {
    ...before, events: input.events, rows: before.rows.slice(), group: { ...before.group },
    results: new Map(before.results), tools: new Map(before.tools), toolRows: new Map(before.toolRows),
    eventRows: new Map(before.eventRows), starts: new Map(before.starts), taskEvents: before.taskEvents.slice(), seenIds: new Set(before.seenIds),
  } : {
    events: input.events, revision: input.eventsRevision, config, rows: [], data: [], results: new Map(),
    tools: new Map(), toolRows: new Map(), eventRows: new Map(), starts: new Map(), group: emptyGroup(),
    lastDay: '', lastSent: null, taskEvents: [], tasks: [], taskAnchor: null, lastAnchor: null, seenIds: new Set(), duplicateIds: false,
  };
  const { rows } = next;
  const changedResults = new Set<string>();
  let tasksChanged = !append;
  let normalizeKeys = append && before.duplicateIds && before.group.mode !== null;
  // Resultados são lidos antes da montagem: um lote pode trazer o resultado antes da chamada.
  for (let i = start; i < input.events.length; i++) {
    const ev = input.events[i];
    if (ev.kind === 'tool_result' && ev.tool_use_id) {
      if (next.results.get(ev.tool_use_id) !== ev) changedResults.add(ev.tool_use_id);
      next.results.set(ev.tool_use_id, ev);
    }
  }
  const pageOf = (ev: ChatEvent) => {
    const result = next.results.get(ev.tool_use_id ?? '');
    return input.pagesEnabled && result && !result.is_error ? htmlPageFromResult(ev.tool_name, result.result) : null;
  };
  const remember = (ev: ChatEvent, index: number) => {
    next.eventRows.set(ev.id, index);
    if (ev.kind === 'tool_use' && ev.tool_use_id) next.toolRows.set(ev, index);
  };
  // Só o trecho da cauda exclusivo desta chamada pode crescer sem copiar seus recipientes.
  let editableFold: MessageRow | undefined;
  const addFold = (ev: ChatEvent, id: string, source: 'tool' | 'group' | 'pensamento', extend: boolean) => {
    const index = rows.length - 1;
    let last = rows[index];
    if (last?.type === 'fold' && (input.merge || extend)) {
      if (last !== editableFold) {
        last = { ...last, parts: last.parts.slice(), summary: appendFoldSummary(last.summary, [], resultOf(next.results)), resultOf: resultOf(next.results) };
        rows[index] = last;
      }
      last.parts.push(ev);
      accumulateFoldSummary(last.summary!, [ev], resultOf(next.results));
      editableFold = last;
      remember(ev, index);
      return index;
    }
    editableFold = { type: 'fold', id: `f-${id}`, parts: [ev], ...(!input.merge && { source }), summary: appendFoldSummary(undefined, [ev], resultOf(next.results)), resultOf: resultOf(next.results) };
    rows.push(editableFold);
    remember(ev, rows.length - 1);
    return rows.length - 1;
  };
  for (let i = start; i < input.events.length; i++) {
    const ev = input.events[i];
    const kind = ev.kind;
    let dayTs: number | undefined;
    if (ev.ts != null) {
      const day = new Date(ev.ts * 1000).toDateString();
      if (day !== next.lastDay) { dayTs = ev.ts; next.lastDay = day; }
    }
    if (kind === 'user_msg' && !ev.id.startsWith('queued-') && ev.ts) next.lastSent = ev.ts * 1000;
    if (kind === 'tool_result') continue;
    if (next.seenIds.has(ev.id) || (next.duplicateIds && /#\d+$/.test(ev.id))) {
      normalizeKeys = true;
      next.duplicateIds = true;
    }
    next.seenIds.add(ev.id);
    if (kind === 'tool_use' && ev.tool_use_id) {
      const id = ev.tool_use_id;
      next.tools.set(id, [...(next.tools.get(id) ?? []), ev]);
      if (!next.starts.has(id)) next.starts.set(id, ev.ts ? ev.ts * 1000 : null);
    }
    if (isTask(ev)) {
      next.taskEvents.push(ev);
      next.taskAnchor = next.lastAnchor;
      tasksChanged = true;
    } else next.lastAnchor = ev.id;
    if (!visible(ev) || (input.tasksEnabled && isTask(ev))
      || (kind === 'tool_use' && input.running.has(ev.tool_use_id ?? ''))
      || (input.queuePanel && hiddenByQueuePanel(ev, input.queuePanel))) continue;
    const g = next.group;
    if (kind === 'thinking' || (g.mode === 'thought' && kind === 'tool_use'
      && !isHtmlRenderTool(ev.tool_name) && entraNoPensamento(input.pref, ev.tool_name))) {
      const extend = g.mode === 'thought';
      addFold(ev, extend ? `p-${g.first}` : `p-${ev.id}`, 'pensamento', extend);
      if (!extend) g.first = ev.id;
      g.mode = 'thought';
      g.count = 0;
    } else if (kind === 'tool_use' && ev.tool_name !== 'Agent' && !isHtmlRenderTool(ev.tool_name)) {
      const extend = g.mode === 'run';
      if (!extend) { g.first = ev.id; g.count = 0; }
      g.count++;
      if (!input.merge && extend && g.count === 3 && !everyRun) {
        const first = rows[rows.length - 2], second = rows[rows.length - 1];
        if (first.type === 'fold' && second.type === 'fold') {
          const left = first.summary!, right = second.summary!;
          const summary: FoldSummary = {
            lines: [...left.lines, ...right.lines], counts: left.counts.map((count, i) => count + right.counts[i]),
            calls: left.calls + right.calls, thoughts: left.thoughts + right.thoughts,
            failures: left.failures + right.failures, live: new Set([...left.live, ...right.live]),
          };
          const row: MessageRow = { type: 'fold', id: `f-g-${g.first}`, source: 'group',
            parts: [...first.parts, ...second.parts, ev],
            summary: accumulateFoldSummary(summary, [ev], resultOf(next.results)), resultOf: resultOf(next.results) };
          rows.splice(rows.length - 2, 2, row);
          editableFold = row;
          for (const part of row.parts) remember(part, rows.length - 1);
        }
      } else {
        const index = addFold(ev, g.count >= groupMin ? `g-${g.first}` : ev.id, g.count >= groupMin ? 'group' : 'tool', extend && g.count > groupMin);
        if (input.merge && !everyRun && g.count === 3 && rows[index].id === `f-${g.first}`) {
          rows[index].id = `f-g-${g.first}`;
        }
      }
      g.mode = 'run';
    } else if (kind === 'tool_use') {
      next.group = emptyGroup();
      const page = pageOf(ev);
      if (page) {
        rows.push({ type: 'page', id: `p-${ev.id}`, ev, page });
        remember(ev, rows.length - 1);
      } else addFold(ev, ev.id, 'tool', false);
    } else {
      next.group = emptyGroup();
      rows.push({ type: 'event', id: ev.id, ev, ...(ev.orq && { dayTs }) });
      remember(ev, rows.length - 1);
    }
  }
  for (const id of changedResults) {
    const tools = next.tools.get(id) ?? [];
    if (tools.some(isTask)) tasksChanged = true;
    const usesByRow = new Map<number, ChatEvent[]>();
    for (const ev of append ? before.tools.get(id) ?? [] : []) {
      const index = next.toolRows.get(ev);
      if (index !== undefined) usesByRow.set(index, [...(usesByRow.get(index) ?? []), ev]);
    }
    for (const [index, uses] of usesByRow) {
      const row = rows[index];
      if (row.type === 'fold') rows[index] = { ...row, parts: row.parts.slice(),
        summary: row.summary && updateFoldSummary(row.summary, uses, before?.results.get(id) ?? null, next.results.get(id) ?? null),
        resultOf: resultOf(next.results) };
    }
  }
  // Uma página que acabou de chegar corta apenas o trecho que a continha.
  const pageChanges = [...changedResults].filter((id) => (next.tools.get(id) ?? []).some((ev) => isHtmlRenderTool(ev.tool_name)));
  if (pageChanges.length && append) {
    const affected = new Set(pageChanges.flatMap((id) => (next.tools.get(id) ?? []).map((ev) => next.toolRows.get(ev))).filter((i): i is number => i !== undefined));
    for (const index of [...affected].sort((a, b) => b - a)) {
      const row = rows[index];
      if (row.type !== 'fold' && row.type !== 'page') continue;
      let from = index, to = index + 1;
      if (row.type === 'page' && !pageOf(row.ev) && input.merge) {
        if (rows[from - 1]?.type === 'fold') from--;
        if (rows[to]?.type === 'fold') to++;
      }
      const parts = rows.slice(from, to).flatMap((r) => r.type === 'fold' ? r.parts : r.type === 'page' ? [r.ev] : []);
      const replacement = foldConversation(agruparConversa(parts, { entraNoPensamento: (name) => entraNoPensamento(input.pref, name), groupMin }), input.merge, pageOf)
        .map((r) => ({ ...r, ...(r.type === 'fold' && { summary: appendFoldSummary(undefined, r.parts, resultOf(next.results)) }), resultOf: resultOf(next.results) }));
      rows.splice(from, to - from, ...replacement);
    }
    next.toolRows.clear(); next.eventRows.clear();
    rows.forEach((row, i) => {
      if (row.type === 'fold') row.parts.forEach((ev) => remember(ev, i));
      else if (row.type === 'event' || row.type === 'page') remember(row.ev, i);
    });
  }
  if (normalizeKeys) {
    const days = new Map(rows.flatMap((r) => r.type === 'event' ? [[r.ev, r.dayTs] as const] : []));
    const visibleEvents = input.events.filter((ev) => visible(ev) && !(input.tasksEnabled && isTask(ev))
      && !(ev.kind === 'tool_use' && input.running.has(ev.tool_use_id ?? ''))
      && !(input.queuePanel && hiddenByQueuePanel(ev, input.queuePanel)));
    const normalized = foldConversation(agruparConversa(visibleEvents, { entraNoPensamento: (name) => entraNoPensamento(input.pref, name), groupMin }), input.merge, pageOf);
    rows.splice(0, rows.length, ...normalized.map((r) => ({ ...r,
      ...(r.type === 'event' && r.ev.orq && { dayTs: days.get(r.ev) }),
      ...(r.type === 'fold' && { summary: appendFoldSummary(undefined, r.parts, resultOf(next.results)), resultOf: resultOf(next.results) }),
    })));
    next.toolRows.clear(); next.eventRows.clear();
    rows.forEach((row, i) => {
      if (row.type === 'fold') row.parts.forEach((ev) => remember(ev, i));
      else if (row.type === 'event' || row.type === 'page') remember(row.ev, i);
    });
  }
  if (tasksChanged && input.tasksEnabled) {
    const sameTasks = before && before.config === config && before.taskEvents.length === next.taskEvents.length
      && next.taskEvents.every((ev, i) => ev === before.taskEvents[i] && next.results.get(ev.tool_use_id ?? '') === before.results.get(ev.tool_use_id ?? ''));
    next.tasks = sameTasks ? before.tasks : foldTasks(next.taskEvents, (id) => next.results.get(id));
  }
  if (!append && before) {
    const old = new Map(before.rows.map((row) => [row.id, row]));
    for (let i = 0; i < rows.length; i++) {
      const previous = old.get(rows[i].id);
      if (previous && sameRow(rows[i], previous, next.results, before.results)) rows[i] = previous;
    }
  }
  next.data = rows;
  if (input.tasksEnabled && next.tasks.length) {
    next.data = rows.slice();
    const index = next.taskAnchor === null ? -1 : next.eventRows.get(next.taskAnchor) ?? -1;
    const previous = before?.data.find((row) => row.type === 'tasks');
    const taskRow: MessageRow = previous?.type === 'tasks' && previous.tasks === next.tasks ? previous : { type: 'tasks', id: 'tasks', tasks: next.tasks };
    next.data.splice(index + 1, 0, taskRow);
  }
  return next;
}
