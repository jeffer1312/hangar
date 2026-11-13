// @vitest-environment happy-dom
import { act, createElement, type ComponentProps } from 'react';
import { createRoot } from 'react-dom/client';
import { expect, it, vi } from 'vitest';
import { Pressable } from 'react-native';
import type { ChatEvent } from '@hangar/core';
import { projectConversation, type ProjectionInput } from '../conversationProjection';
import { ToolGroup } from './ToolGroup';
import { ToolCard } from './ToolCard';
import * as m from '../../paraglide/messages';

vi.mock('react-native', async (original) => {
  const native = await original<typeof import('react-native')>();
  return { ...native, Pressable: vi.fn(native.Pressable) };
});

vi.mock('react-native-enriched-markdown', () => ({ EnrichedMarkdownText: () => null }));
vi.mock('expo-file-system', () => ({}));
vi.mock('expo-clipboard', () => ({}));
vi.mock('expo-sharing', () => ({}));
vi.mock('expo-intent-launcher', () => ({}));
vi.mock('expo-router', () => ({ useRouter: () => ({}) }));
vi.mock('expo-audio', () => ({}));
vi.mock('expo-secure-store', () => ({}));
vi.mock('expo-glass-effect', () => ({ isLiquidGlassAvailable: () => false }));
vi.mock('sonner-native', () => ({ toast: { error: () => {}, success: () => {} } }));
vi.mock('react-native-gifted-charts', () => ({}));
vi.mock('expo-linear-gradient', () => ({}));
vi.mock('expo-blur', () => ({}));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: () => null }));
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, getNumber: () => undefined, getBoolean: () => undefined, set: () => {}, remove: () => {} }) }));
(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

it('Terminal mantém Update e o diff numerado à vista sem abrir detalhe', () => {
  const use: ChatEvent = { id: 'edit', kind: 'tool_use', tool_use_id: 'edit', tool_name: 'Edit',
    tool_input: { file_path: '/src/app.ts', old_string: 'const old = 1;', new_string: 'const next = 2;' } };
  const result: ChatEvent = { id: 'done', kind: 'tool_result', tool_use_id: 'edit', result: 'ok',
    patch: [{ old_start: 42, new_start: 42, lines: ['-const old = 1;', '+const next = 2;'] }] };
  const container = document.createElement('div');
  const root = createRoot(container);
  const onAbrir = vi.fn();
  try {
    act(() => root.render(createElement(ToolGroup, { parts: [use], resultOf: () => result, onAbrir, look: 'terminal' })));
    expect(container.textContent).toContain('Update');
    expect(container.textContent).toContain('(app.ts)');
    expect(container.textContent).toContain('⎿');
    expect(container.textContent).toContain('42 - const old = 1;');
    expect(container.textContent).toContain('42 + const next = 2;');
    expect(container.querySelector('button')).toBeNull();
    expect(onAbrir).not.toHaveBeenCalled();
  } finally {
    act(() => root.unmount());
  }
});

it('Terminal dobra leituras mas mantém edição e falha visíveis, abrindo a saída na conversa', () => {
  const read = (id: string): ChatEvent => ({ id, kind: 'tool_use', tool_use_id: id, tool_name: 'Read', tool_input: { file_path: '/src/same.ts' } });
  const parts: ChatEvent[] = [read('read-1'), read('read-2'),
    { id: 'edit', kind: 'tool_use', tool_use_id: 'edit', tool_name: 'Edit', tool_input: { file_path: '/src/app.ts', old_string: 'a', new_string: 'b' } },
  ];
  const resultOf = (tool: ChatEvent): ChatEvent => ({ id: `r-${tool.id}`, kind: 'tool_result', tool_use_id: tool.id,
    result: tool.id === 'read-1' ? 'permission-denied\nmore-error-details' : 'ok', is_error: tool.id === 'read-1' });
  const container = document.createElement('div');
  const root = createRoot(container);
  const onAbrir = vi.fn(), onExpand = vi.fn();
  try {
    act(() => root.render(createElement(ToolGroup, { parts, resultOf, onAbrir, onExpand, look: 'terminal' })));
    expect(container.textContent?.toLowerCase()).toContain(m.term_dobra_leu_1().toLowerCase());
    expect(container.textContent).toContain('permission-denied');
    expect(container.textContent).toContain('Update(app.ts)');
    expect(container.textContent).not.toContain('Read(same.ts)');
    act(() => container.querySelector('button')!.click());
    expect(container.textContent).toContain('Read(same.ts)');
    const head = [...container.querySelectorAll('button')].find((b) => b.getAttribute('aria-label')?.startsWith('Read:'))!;
    act(() => head.click());
    expect(container.textContent).toContain('more-error-details');
    expect(onExpand).toHaveBeenCalledTimes(2);
    expect(onAbrir).not.toHaveBeenCalled();
    act(() => head.click());
    expect(container.textContent).not.toContain('more-error-details');
    expect(container.textContent).toContain('Read(same.ts)');
  } finally { act(() => root.unmount()); }
});

it('Terminal recolhe pelo corpo sem tomar o toque longo usado para selecionar', () => {
  const use: ChatEvent = { id: 'shell', kind: 'tool_use', tool_name: 'Bash', tool_input: { command: 'pwd' } };
  const result: ChatEvent = { id: 'output', kind: 'tool_result', result: '/repo\nsecond-output-line' };
  const container = document.createElement('div');
  const root = createRoot(container);
  try {
    act(() => root.render(createElement(ToolCard, { use, result, look: 'terminal', onPress: vi.fn() })));
    act(() => container.querySelector('button')!.click());
    vi.mocked(Pressable).mockClear();
    // Recolhe pelo cabeçalho e abre de novo para capturar os controles montados.
    act(() => container.querySelector('button')!.click());
    act(() => container.querySelector('button')!.click());
    const body = vi.mocked(Pressable).mock.calls.map(([props]) => props).find((props) => props.accessible === false && props.onLongPress);
    expect(body).toBeDefined();
    act(() => { body!.onPressIn?.({} as never); body!.onLongPress?.({} as never); body!.onPress?.({} as never); });
    expect(container.textContent).toContain('second-output-line');
    act(() => { body!.onPressIn?.({} as never); body!.onPress?.({} as never); });
    expect(container.textContent).not.toContain('second-output-line');
  } finally { act(() => root.unmount()); }
});

it('Terminal corta Write em dez linhas, limita a expansão e permite ver o restante separado', () => {
  const use: ChatEvent = { id: 'write', kind: 'tool_use', tool_name: 'Write', tool_input: { file_path: '/repo/large.ts',
    content: Array.from({ length: 450 }, (_, i) => `line-${i + 1}`).join('\n') } };
  const result: ChatEvent = { id: 'written', kind: 'tool_result', result: 'ok' };
  const container = document.createElement('div');
  const root = createRoot(container);
  const onPress = vi.fn(), onExpand = vi.fn();
  try {
    act(() => root.render(createElement(ToolCard, { use, result, look: 'terminal', onPress, onExpand })));
    expect(container.textContent).toContain('10 + line-10');
    expect(container.textContent).not.toContain('11 + line-11');
    act(() => container.querySelector('button')!.click());
    expect(container.textContent).toContain('400 + line-400');
    expect(container.textContent).not.toContain('401 + line-401');
    expect(container.textContent).toContain(m.native_side_diff_truncated());
    const detail = [...container.querySelectorAll('button')].find((b) => b.textContent === m.tool_ver_separado())!;
    act(() => detail.click());
    expect(onPress).toHaveBeenCalledExactlyOnceWith(use);
    expect(onExpand).toHaveBeenCalledTimes(1);
  } finally { act(() => root.unmount()); }
});

it('Terminal mantém expostas as ferramentas dentro de um pensamento', () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  const parts: ChatEvent[] = [{ id: 'think', kind: 'thinking', text: 'why' },
    { id: 'search', kind: 'tool_use', tool_name: 'WebSearch', tool_input: { query: 'native-appearance' } }];
  try {
    act(() => root.render(createElement(ToolGroup, { parts, resultOf: (): ChatEvent => ({ id: 'done', kind: 'tool_result', result: 'ok' }), onAbrir: vi.fn(), look: 'terminal' })));
    expect(container.textContent).toContain('WebSearch(native-appearance)');
  } finally { act(() => root.unmount()); }
});

it('avisa antes de expandir o grupo real, mas não ao recolher', () => {
  const parts: ChatEvent[] = [
    { id: 'first', kind: 'thinking', text: 'first-thought' },
    { id: 'second', kind: 'thinking', text: 'second-thought' },
  ];
  const container = document.createElement('div');
  const root = createRoot(container);
  const onExpand = vi.fn(() => expect(container.textContent).not.toContain('first-thought'));
  try {
    act(() => root.render(createElement(ToolGroup, { parts, resultOf: () => null, onAbrir: () => {}, onExpand })));
    const head = container.querySelector('button')!;
    act(() => head.click());
    expect(onExpand).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain('first-thought');
    act(() => head.click());
    expect(onExpand).toHaveBeenCalledTimes(1);
    expect(container.textContent).not.toContain('first-thought');
  } finally {
    act(() => root.unmount());
  }
});

it('avisa antes de Mostrar todas no grupo real', () => {
  const parts: ChatEvent[] = Array.from({ length: 41 }, (_, i) => ({ id: `thought-${i}`, kind: 'thinking', text: `thought-${i}-text` }));
  const container = document.createElement('div');
  const root = createRoot(container);
  const onExpand = vi.fn(() => expect(container.textContent).not.toContain('thought-40-text'));
  try {
    act(() => root.render(createElement(ToolGroup, { parts, resultOf: () => null, onAbrir: () => {}, onExpand })));
    act(() => container.querySelector('button')!.click());
    onExpand.mockClear();
    const showAll = [...container.querySelectorAll('button')].find(button => button.textContent === m.tool_mostrar_todas({ n: 41 }))!;
    expect(container.textContent).not.toContain('thought-40-text');
    act(() => showAll.click());
    expect(onExpand).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain('thought-40-text');
  } finally {
    act(() => root.unmount());
  }
});

it('trecho recolhido com 5000 chamadas e 200 appends não reconta o histórico', () => {
  let reads = 0;
  const counted = (ev: ChatEvent) => new Proxy(ev, { get(target, key, receiver) {
    if (typeof key === 'string') reads++;
    return Reflect.get(target, key, receiver);
  } });
  const pair = (n: number): ChatEvent[] => [
    counted({ id: `t${n}`, kind: 'tool_use', tool_use_id: `t${n}`, tool_name: 'Bash', tool_input: { command: 'pwd' } }),
    counted({ id: `r${n}`, kind: 'tool_result', tool_use_id: `t${n}`, result: 'ok' }),
  ];
  let events = Array.from({ length: 5000 }, (_, i) => pair(i)).flat();
  const options = (events: ChatEvent[]): ProjectionInput => ({ events, eventsRevision: 0, merge: true, pref: 'busca', tasksEnabled: false, pagesEnabled: true, running: new Set() });
  let projection = projectConversation(null, options(events));
  const container = document.createElement('div');
  const root = createRoot(container);
  const render = () => {
    const row = projection.data[0];
    if (row.type !== 'fold') throw new Error('expected fold');
    act(() => root.render(createElement(ToolGroup, { parts: row.parts, resultOf: row.resultOf!, summary: row.summary, onAbrir: () => {} } as ComponentProps<typeof ToolGroup>)));
  };
  try {
    render();
    reads = 0;
    for (let i = 0; i < 200; i++) {
      events = [...events, ...pair(5000 + i)];
      projection = projectConversation(projection, options(events));
      render();
    }
    const deltaReads = reads;
    expect(container.querySelectorAll('button')).toHaveLength(1);
    expect(container.textContent).toContain('5200');
    expect(deltaReads).toBeLessThan(200 * 100);
  } finally {
    act(() => root.unmount());
  }
});

it('Claude Desktop nasce recolhido e abre cartão, falha e código de saída', () => {
  const bash = (id: string): ChatEvent => ({ id, kind: 'tool_use', tool_use_id: id, tool_name: 'Bash', tool_input: { command: `run-${id}` } });
  const results: Record<string, ChatEvent> = {
    ok: { id: 'r-ok', kind: 'tool_result', tool_use_id: 'ok', result: 'done' },
    bad: { id: 'r-bad', kind: 'tool_result', tool_use_id: 'bad', result: 'Exit code 1\nboom', is_error: true },
  };
  const container = document.createElement('div');
  const root = createRoot(container);
  const onExpand = vi.fn();
  try {
    act(() => root.render(createElement(ToolGroup, { parts: [bash('ok'), bash('bad')], resultOf: (t) => results[t.id] ?? null, onAbrir: () => {}, onExpand, look: 'claudeDesktop' })));
    expect(container.textContent).toBe(`${m.ferr_desktop_rodou({ n: 2 })} ${m.ferr_desktop_falhas_1()}`);
    act(() => container.querySelector('button')!.click());
    expect(onExpand).toHaveBeenCalledTimes(1);
    expect(container.textContent).toContain('run-ok');
    expect(container.textContent).toContain(m.formato_tool_falhou());
    const failed = [...container.querySelectorAll('button')].find((b) => b.getAttribute('aria-label')?.includes('run-bad'))!;
    act(() => failed.click());
    expect(container.textContent).toContain(m.ferr_desktop_codigo_saida({ n: 1 }));
    expect(container.textContent).toContain('boom');
  } finally {
    act(() => root.unmount());
  }
});
