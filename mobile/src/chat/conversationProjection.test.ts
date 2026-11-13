import { describe, expect, it } from 'vitest';
import { agruparConversa, entraNoPensamento, htmlPageFromResult, type ChatEvent } from '@hangar/core';
import { foldConversation, summaryTitle, foldTitle } from './tools/fold';
import { projectConversation, type ConversationProjection, type ProjectionInput } from './conversationProjection';

const tool = (id: string, name = 'Bash', input = {}): ChatEvent => ({ id, kind: 'tool_use', tool_use_id: id, tool_name: name, tool_input: input });
const result = (id: string, text = 'ok', error = false): ChatEvent => ({ id: `r-${id}`, kind: 'tool_result', tool_use_id: id, result: text, is_error: error });
const message = (id: string): ChatEvent => ({ id, kind: 'assistant_msg', text: id });
const input = (events: ChatEvent[], patch: Partial<ProjectionInput> = {}): ProjectionInput => ({
  events, eventsRevision: 0, pref: 'busca', merge: true, tasksEnabled: false, pagesEnabled: true, running: new Set(), ...patch,
});

const pageResult = (id: string) => result(id, JSON.stringify({ hangar_page: { id: 'page', title: 'report' } }));
const plainRows = (rows: ConversationProjection['data']) => rows.map((row) => {
  const { resultOf: _resultOf, summary: _summary, ...plain } = row;
  return plain;
});

describe.each([[true, false], [false, false], [false, true]])('contrato do agrupamento, árvore=%s, toda sequência=%s', (merge, everyRun) => {
  it.each(['nada', 'busca', 'tudo'] as const)('preserva cada prefixo com preferência %s', (pref) => {
    const events: ChatEvent[] = [
      message('a'), { id: 'thought', kind: 'thinking', text: 'reason' }, tool('load', 'ToolSearch'),
      tool('search', 'WebSearch'), tool('t1'), tool('t2'), tool('t3'),
      tool('agent', 'Agent'), tool('html', 'mcp__hangar__html_render'), tool('read', 'Read'),
      pageResult('html'), message('b'), tool('after'), result('t2', 'failed', true),
    ];
    let state: ConversationProjection | null = null;
    for (let n = 1; n <= events.length; n++) {
      const prefix = events.slice(0, n);
      state = projectConversation(state, input(prefix, { merge, pref, everyRun }));
      const results = new Map(prefix.filter((ev) => ev.kind === 'tool_result').map((ev) => [ev.tool_use_id, ev]));
      const expected = foldConversation(agruparConversa(prefix, { entraNoPensamento: (name) => entraNoPensamento(pref, name), groupMin: everyRun ? 1 : 3 }), merge,
        (ev) => { const r = results.get(ev.tool_use_id); return r && !r.is_error ? htmlPageFromResult(ev.tool_name, r.result) : null; });
      expect(plainRows(state.data), `prefix=${n}`).toEqual(expected);
      for (const row of state.data) if (row.type === 'fold') expect(summaryTitle(row.summary!)).toBe(foldTitle(row.parts));
    }
  });
});

it('Claude Desktop agrupa sequência de uma chamada e o Agent corta o grupo', () => {
  const events = [message('a'), tool('one'), message('b'), tool('t1'), tool('agent', 'Agent'), tool('t2'), tool('t3')];
  const rows = projectConversation(null, input(events, { merge: false, everyRun: true })).data;
  expect(rows.map((row) => row.type === 'fold' ? [row.source, row.parts.map((p) => p.id)] : row.id)).toEqual([
    'a', ['group', ['one']], 'b', ['group', ['t1']], ['tool', ['agent']], ['group', ['t2', 't3']],
  ]);
  expect(projectConversation(null, input(events, { merge: false })).config)
    .not.toBe(projectConversation(null, input(events, { merge: false, everyRun: true })).config);
});

it('chaves duplicadas continuam únicas inclusive quando um sufixo existe no transcript', () => {
  const events = [message('same'), message('same'), message('same#2')];
  const state = projectConversation(null, input(events));
  expect(state.data.map((row) => row.id)).toEqual(['same', 'same#3', 'same#2']);
  const appended = projectConversation(state, input([...events, message('tail')]));
  expect(appended.data.map((row) => row.id)).toEqual(['same', 'same#3', 'same#2', 'tail']);
});

it('reativar tarefas recompõe a lista e oculta as chamadas sem novos eventos', () => {
  const events = [tool('create', 'TaskCreate', { subject: 'Fix chat' }), result('create', 'Task #1 created successfully: Fix chat')];
  const before = projectConversation(null, input(events, { tasksEnabled: false }));
  expect(before.tasks).toEqual([]);
  expect(before.data.map((row) => row.type)).toEqual(['fold']);

  const after = projectConversation(before, input(events, { tasksEnabled: true }));
  expect(after.data.map((row) => row.type)).toEqual(['tasks']);
  expect(after.data[0]).toMatchObject({ type: 'tasks', tasks: [{ id: '1', subject: 'Fix chat', status: 'pending' }] });
  expect(before.tasks).toEqual([]);
});

it('resultado tardio atualiza todas as ocorrências da mesma chamada sem tocar vizinhas', () => {
  const first = { ...tool('first'), tool_use_id: 'shared' };
  const second = { ...tool('second'), tool_use_id: 'shared' };
  const events = [first, message('separator'), second, message('end')];
  const before = projectConversation(null, input(events));
  const after = projectConversation(before, input([...events, result('shared', 'failed', true)]));
  expect(after.data.filter((r) => r.type === 'fold').map((r) => r.summary?.failures)).toEqual([1, 1]);
  expect(after.data[1]).toBe(before.data[1]);
  expect(after.data[3]).toBe(before.data[3]);
  expect(before.data.filter((r) => r.type === 'fold').map((r) => r.summary?.failures)).toEqual([0, 0]);
});

it.each([false, true])('promoção em lote conta o erro da segunda uma vez, resultado da terceira=%s', (thirdResolved) => {
  const events = [tool('first'), tool('second')];
  const before = projectConversation(null, input(events, { merge: false }));
  const third = tool('third');
  const after = projectConversation(before, input([
    ...events, third, result('second', 'failed', true), ...(thirdResolved ? [result('third')] : []),
  ], { merge: false }));

  expect(after.data).toHaveLength(1);
  expect(after.data[0]).toMatchObject({ type: 'fold', source: 'group', summary: {
    lines: [...events, third], counts: [0, 0, 0, 0, 3, 0], calls: 3, thoughts: 0, failures: 1,
  } });
  expect(after.data[0].summary?.live).toEqual(new Set(thirdResolved ? [events[0]] : [events[0], third]));
  expect(before.data.map((row) => row.summary?.failures)).toEqual([0, 0]);
  expect(before.data.map((row) => [...row.summary!.live])).toEqual([[events[0]], [events[1]]]);
});

it.each([true, false])('lote frio de 400 ferramentas mantém trabalho linear, árvore=%s', (merge) => {
  const events = Array.from({ length: 400 }, (_, i) => tool(`t${i}`));
  const original = structuredClone(events);
  for (const ev of events) { Object.freeze(ev.tool_input); Object.freeze(ev); }
  Object.freeze(events);
  const options = input(events, { merge });
  const arrayIterator = Array.prototype[Symbol.iterator];
  const slice = Array.prototype.slice;
  const setIterator = Set.prototype[Symbol.iterator];
  const work = { array: 0, slice: 0, set: 0 };
  let state: ConversationProjection;
  try {
    Array.prototype[Symbol.iterator] = function* () {
      for (const value of arrayIterator.call(this)) { work.array++; yield value; }
      return undefined;
    };
    Array.prototype.slice = function (start?: number, end?: number) {
      const copied = slice.call(this, start, end);
      work.slice += copied.length;
      return copied;
    };
    Set.prototype[Symbol.iterator] = function* () {
      for (const value of setIterator.call(this)) { work.set++; yield value; }
      return undefined;
    };
    state = projectConversation(null, options);
  } finally {
    Array.prototype[Symbol.iterator] = arrayIterator;
    Array.prototype.slice = slice;
    Set.prototype[Symbol.iterator] = setIterator;
  }
  const total = work.array + work.slice + work.set;
  process.stdout.write(`lote frio árvore=${merge}: ${JSON.stringify({ ...work, total })}\n`);
  expect(state.data.map((row) => row.id)).toEqual(['f-g-t0']);
  const row = state.data[0];
  if (row.type !== 'fold') throw new Error('expected fold');
  const ids = Array.from({ length: 400 }, (_, i) => `t${i}`);
  expect(row.source).toBe(merge ? undefined : 'group');
  expect(row.parts.map((ev) => ev.id)).toEqual(ids);
  expect(row.parts.every((ev, i) => ev === events[i])).toBe(true);
  expect(row.summary).toMatchObject({ calls: 400, thoughts: 0, failures: 0, counts: [0, 0, 0, 0, 400, 0] });
  expect(row.summary!.lines.map((ev) => ev.id)).toEqual(ids);
  expect([...row.summary!.live].map((ev) => ev.id)).toEqual(ids);
  expect(events).toEqual(original);
  expect(total, JSON.stringify(work)).toBeLessThanOrEqual(events.length * 12);
});

it.each([true, false])('lote appended preserva snapshots na promoção e no resultado tardio, árvore=%s', (merge) => {
  const calls = Array.from({ length: 400 }, (_, i) => tool(`t${i}`));
  for (const ev of calls) { Object.freeze(ev.tool_input); Object.freeze(ev); }
  const events = [message('start'), ...calls.slice(0, 2)];
  Object.freeze(events[0]);
  Object.freeze(events);
  const capture = (state: ConversationProjection) => state.rows.map((row) => {
    const { resultOf: _resultOf, ...plain } = row;
    return structuredClone(plain);
  });
  const before = projectConversation(null, input(events, { merge }));
  const originalRows = before.rows.slice();
  const original = capture(before);
  expect(before.data.map((row) => row.id)).toEqual(merge ? ['start', 'f-t0'] : ['start', 'f-t0', 'f-t1']);

  const appended = [...events, ...calls.slice(2)];
  const grown = projectConversation(before, input(appended, { merge }));
  const grownRows = grown.rows.slice();
  const grownSnapshot = capture(grown);
  expect(grown.data.map((row) => row.id)).toEqual(['start', 'f-g-t0']);
  const row = grown.data[1];
  if (row.type !== 'fold') throw new Error('expected fold');
  expect(row.parts).toEqual(calls);
  expect(row.source).toBe(merge ? undefined : 'group');
  expect(row.summary).toMatchObject({ lines: calls, calls: 400, thoughts: 0, failures: 0, counts: [0, 0, 0, 0, 400, 0] });
  expect([...row.summary!.live]).toEqual(calls);
  expect(grown.data[0]).toBe(before.data[0]);

  const tail = tool('tail');
  const failed = result('t1', 'failed', true);
  const after = projectConversation(grown, input([...appended, tail, failed], { merge }));
  expect(after.data.map((row) => row.id)).toEqual(['start', 'f-g-t0']);
  const finalRow = after.data[1];
  if (finalRow.type !== 'fold') throw new Error('expected fold');
  expect(finalRow.parts).toEqual([...calls, tail]);
  expect(finalRow.summary).toMatchObject({ lines: [...calls, tail], calls: 401, thoughts: 0, failures: 1, counts: [0, 0, 0, 0, 401, 0] });
  expect([...finalRow.summary!.live].map((ev) => ev.id)).toEqual(['t0', ...Array.from({ length: 398 }, (_, i) => `t${i + 2}`), 'tail']);
  expect(finalRow.resultOf!(calls[1])).toBe(failed);
  expect(row.resultOf!(calls[1])).toBeNull();
  expect(after.data[0]).toBe(before.data[0]);
  expect(capture(before)).toEqual(original);
  expect(capture(grown)).toEqual(grownSnapshot);
  expect(before.rows.every((r, i) => r === originalRows[i])).toBe(true);
  expect(grown.rows.every((r, i) => r === grownRows[i])).toBe(true);
  expect(before.results.size).toBe(0);
  expect(grown.results.size).toBe(0);
  expect(events).toEqual([message('start'), calls[0], calls[1]]);
  expect(appended).toEqual([message('start'), ...calls]);
});

it('resumo de 5000 ferramentas e 200 resultados tardios não relê o trecho fechado', () => {
  let reads = 0;
  const counted = (ev: ChatEvent) => new Proxy(ev, { get(target, key, receiver) {
    if (typeof key === 'string') reads++;
    return Reflect.get(target, key, receiver);
  } });
  let events = Array.from({ length: 5000 }, (_, i) => counted(tool(`t${i}`)));
  let state = projectConversation(null, input(events));
  const before = state;
  reads = 0;
  for (let i = 0; i < 200; i++) {
    events = [...events, counted(result(`t${i}`, 'failed', true))];
    state = projectConversation(state, input(events));
  }
  const deltaReads = reads;
  const summary = (state.data[0] as { summary?: { calls: number; failures: number; live: Set<ChatEvent>; lines: ChatEvent[] } }).summary;
  expect(summary?.calls).toBe(5000);
  expect(summary?.failures).toBe(200);
  expect(summary?.live.size).toBe(4800);
  expect(summary?.lines).toHaveLength(5000);
  expect((before.data[0] as { summary?: { failures: number } }).summary?.failures).toBe(0);
  expect(before.results.size).toBe(0);
  expect(deltaReads).toBeLessThan(200 * 100);
});
