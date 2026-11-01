// @vitest-environment happy-dom
import { act, createElement, forwardRef, type FormEvent, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, type SearchHit, type Server } from '@hangar/core';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const h = vi.hoisted(() => ({ params: {} as { q?: string }, search: vi.fn(), ask: vi.fn(), context: vi.fn(), push: vi.fn(), replace: vi.fn(), dismiss: vi.fn(), focused: true, canGoBack: true,
  options: null as { title?: string; headerShown?: boolean; headerTransparent?: boolean; headerLeft?: () => ReactNode } | null,
  edges: undefined as string[] | undefined, listInset: undefined as string | undefined }));
const state = vi.hoisted(() => ({ activeId: 'b', servers: [] as Server[] }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(),
  searchTranscriptsForServer: h.search, askHistoryForServer: h.ask, getSearchContextForServer: h.context,
}));
vi.mock('../../stores/servers', () => ({ useServers: Object.assign((select: (s: typeof state) => unknown) => select(state), { getState: () => state }) }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: h.push, replace: h.replace, back: () => {}, canGoBack: () => h.canGoBack }), useIsFocused: () => h.focused,
  useLocalSearchParams: () => h.params,
  Stack: { Screen: ({ options }: { options: NonNullable<typeof h.options> }) => { h.options = options; return options.headerLeft?.() ?? null; } } }));
vi.mock('expo-router/react-navigation', () => ({ useHeaderHeight: () => 88 }));
vi.mock('expo-blur', async () => ({ BlurView: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../ui/Screen', () => ({ Screen: ({ children, edges }: { children: ReactNode; edges?: string[] }) => {
  h.edges = edges; return createElement('div', null, children);
} }));
vi.mock('../../chat/AssistantBubble', () => ({ AssistantBubble: ({ text }: { text: string }) => createElement('article', { 'data-markdown': true }, text) }));
vi.mock('react-native', async (original) => ({ ...await original<typeof import('react-native')>(),
  Platform: { OS: 'ios' },
  Keyboard: { dismiss: h.dismiss },
  FlatList: ({ data, keyExtractor, renderItem, ListHeaderComponent, contentInsetAdjustmentBehavior }: { data: { server: Server; hit: SearchHit }[]; keyExtractor: (item: { server: Server; hit: SearchHit }) => string; renderItem: (row: { item: unknown }) => ReactNode; ListHeaderComponent: ReactNode; contentInsetAdjustmentBehavior?: string }) => {
    h.listInset = contentInsetAdjustmentBehavior;
    return createElement('div', { 'data-search-list': true }, ListHeaderComponent, ...data.map((item) => createElement('div', { key: keyExtractor(item) }, renderItem({ item }))));
  },

  TextInput: forwardRef<HTMLInputElement, { value: string; onChangeText: (s: string) => void; accessibilityLabel: string; autoFocus?: boolean }>(function SearchInput({ value, onChangeText, accessibilityLabel, autoFocus }, ref) {
    return createElement('input', { ref, value, autoFocus, 'aria-label': accessibilityLabel, onInput: (e: FormEvent<HTMLInputElement>) => onChangeText(e.currentTarget.value) });
  }),
}));
import SearchScreen from '../../../app/search';

const roots: Root[] = [];
const hit = (patch: Partial<SearchHit> = {}): SearchHit => ({ project: 'proj', session_id: 'sid', session_name: 'live-a',
  cwd: '/repo', line: 'agulha azul', mtime: 3, live: true, event_id: 'ev-a', role: 'assistant', ...patch });
function deferred<T>() { let resolve!: (v: T) => void; let reject!: (e: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; }
async function mount() { const container = document.createElement('div'); document.body.append(container); const root = createRoot(container); roots.push(root);
  await act(async () => root.render(createElement(SearchScreen))); return { container, root }; }
async function typeQuery(container: HTMLElement, value: string) {
  const input = container.querySelector<HTMLInputElement>('input');
  expect(input, 'campo de busca acessível').not.toBeNull();
  await act(async () => { Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value);
    input!.dispatchEvent(new Event('input', { bubbles: true })); input!.dispatchEvent(new Event('change', { bubbles: true })); });
}
const buttons = (container: HTMLElement, label: string) => [...container.querySelectorAll<HTMLButtonElement>('button')].filter((b) => b.getAttribute('aria-label') === label || b.textContent === label);
async function tick(ms = 250) { await act(async () => vi.advanceTimersByTimeAsync(ms)); }

beforeEach(() => { vi.resetAllMocks(); vi.useFakeTimers(); h.params = {}; h.focused = true; h.canGoBack = true; h.options = null; h.edges = undefined; h.listInset = undefined;
  state.activeId = 'b'; state.servers = [
    { id: 'a', label: 'A', baseUrl: 'https://a.test', token: 'a' },
    { id: 'b', label: 'B', baseUrl: 'https://b.test', token: 'b' },
    { id: 'guest', label: 'guest', baseUrl: 'https://guest.test', token: 'g', invite: true },
    { id: 'removed', label: 'removed', baseUrl: 'https://removed.test', token: 'r', removed: true },
    { id: 'disabled', label: 'disabled', baseUrl: 'https://disabled.test', token: 'd', disabled: true },
  ];
  configureApi({ getBaseUrl: () => state.servers[1].baseUrl, getToken: () => 'b', onUnauthorized: () => {}, origin: null,
    createEventSource: () => { throw new Error('unexpected SSE'); } });
  h.search.mockResolvedValue([]); h.context.mockResolvedValue([]);
});
afterEach(() => { act(() => { for (const root of roots.splice(0)) root.unmount(); }); document.body.replaceChildren(); vi.useRealTimers(); vi.unstubAllGlobals(); });

it('busca usa cabeçalho nativo e recuo medido, com voltar próprio só em link direto', async () => {
  const { container, root } = await mount();
  expect(h.options?.headerShown).toBe(true);
  expect(h.options?.headerTransparent).toBe(true);
  expect(h.options?.title).toBe(m.switcher_buscar_conversas());
  expect(h.options?.headerLeft).toBeUndefined();
  expect(container.querySelector('[role="header"]')).toBeNull();
  expect(h.edges).not.toContain('top');
  expect(container.querySelector('input')?.parentElement?.style.marginTop).toBe('88px');
  expect(h.listInset).toBe('automatic');
  h.canGoBack = false; await act(async () => root.render(createElement(SearchScreen)));
  const back = buttons(container, m.comum_voltar())[0]; expect(back).toBeDefined();
  await act(async () => back.click()); expect(h.replace).toHaveBeenCalledExactlyOnceWith('/');
});

it('abre já buscando o termo vindo da lista', async () => {
  h.params = { q: 'agulha' };
  const { container } = await mount();
  expect(container.querySelector<HTMLInputElement>('input')?.value).toBe('agulha');
  await tick();
  expect(h.search.mock.calls.map(([server, term]) => [server.id, term])).toEqual([['a', 'agulha'], ['b', 'agulha']]);
});

it('recuperação contextual e controles têm alvo mínimo de toque', async () => {
  h.search.mockImplementation((server: Server) => Promise.resolve(server.id === 'a' ? [hit()] : []));
  h.context.mockRejectedValueOnce(new Error('503 contexto indisponível'));
  const { container } = await mount(); await typeQuery(container, 'agulha'); await tick();
  const snippet = [...container.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.includes('agulha azul'))!;
  await act(async () => snippet.click());
  const retry = buttons(container, m.busca_tentar_de_novo())[0]; expect(retry, 'repetir contexto é botão').toBeDefined();
  for (const control of container.querySelectorAll<HTMLButtonElement>('button')) {
    expect(parseFloat(control.style.minHeight || control.style.height)).toBeGreaterThanOrEqual(44);
    expect(parseFloat(control.style.minWidth || control.style.width)).toBeGreaterThanOrEqual(44);
  }
  h.context.mockResolvedValueOnce([{ id: 'ev-a', kind: 'assistant_msg', text: 'recuperado' }]);
  await act(async () => retry.click()); expect(container.textContent).toContain('recuperado');
});

it.each([
  [Object.assign(new Error('The operation timed out'), { name: 'TimeoutError' }), () => m.busca_motivo_timeout()],
  [new Error('503'), () => m.busca_motivo_http({ status: '503' })],
  [Object.assign(new Error('The operation was aborted'), { name: 'AbortError' }), () => m.busca_motivo_cancelamento()],
  [new Error('fetch failed: Fetch request has been canceled'), () => m.busca_motivo_cancelamento()],
  [new TypeError('Network request failed'), () => m.falha_conexao()],
])('traduz causa da máquina sem expor erro técnico: %s', async (cause, reason) => {
  h.search.mockImplementation((server: Server) => server.id === 'a' ? Promise.reject(cause) : Promise.resolve([]));
  const { container } = await mount(); await typeQuery(container, 'agulha'); await tick();
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.busca_servidor_falhou_motivo({ servidor: 'A', motivo: reason() }));
  expect(buttons(container, m.busca_tentar_de_novo())).toHaveLength(1);
  h.search.mockResolvedValueOnce([hit()]);
  await act(async () => buttons(container, m.busca_tentar_de_novo())[0].click());
  expect(h.search.mock.calls.map(([server]) => server.id)).toEqual(['a', 'b', 'a']);
  expect(container.querySelector('[role="alert"]')).toBeNull();
  expect(container.textContent).toContain('agulha azul');
});

it('busca progressiva espera debounce, preserva sucessos e repete só a máquina que falhou', async () => {
  const b = deferred<SearchHit[]>();
  h.search.mockImplementation((server: Server) => server.id === 'a' ? Promise.resolve([hit(), hit(), hit({ event_id: 'ev-old', line: 'antiga', ts: 1 })]) : b.promise);
  const { container } = await mount(); await typeQuery(container, 'agulha');
  await tick(249); expect(h.search).not.toHaveBeenCalled(); await tick(1);
  expect(h.search.mock.calls.map(([s]) => s.id)).toEqual(['a', 'b']);
  expect(container.textContent).toContain('agulha azul');
  expect(buttons(container, m.busca_abrir_ponto())).toHaveLength(2);
  await act(async () => b.reject(new Error('503')));
  expect(container.textContent).toContain(m.busca_motivo_http({ status: '503' }));
  expect(container.textContent).toContain('agulha azul');
  h.search.mockResolvedValueOnce([hit({ event_id: 'ev-b', line: 'mais recente', ts: 9 })]);
  await act(async () => buttons(container, m.busca_tentar_de_novo())[0].click());
  expect(h.search.mock.calls.map(([s]) => s.id)).toEqual(['a', 'b', 'b']);
  expect(container.textContent!.indexOf('mais recente')).toBeLessThan(container.textContent!.indexOf('agulha azul'));
  expect(container.textContent).not.toContain(m.busca_motivo_http({ status: '503' }));
});

it.each(['todas falham', 'vazio com falha', 'sem máquinas'])('não anuncia ausência de resultados quando %s', async (scenario) => {
  if (scenario === 'sem máquinas') state.servers = [];
  h.search.mockImplementation((server: Server) => scenario === 'vazio com falha' && server.id === 'a'
    ? Promise.resolve([]) : Promise.reject(new Error('503')));
  const { container } = await mount(); await typeQuery(container, 'agulha'); await tick();
  expect(container.textContent).not.toContain(m.busca_nenhum_resultado());
  expect(container.textContent).toContain(scenario === 'sem máquinas' ? m.busca_sem_servidores() : m.busca_motivo_http({ status: '503' }));
});

it('vazio parcial só aparece após sucesso vazio e falha com todas as máquinas concluídas', async () => {
  const pending = deferred<SearchHit[]>();
  h.search.mockImplementation((server: Server) => server.id === 'a' ? Promise.reject(new Error('503')) : pending.promise);
  const { container } = await mount(); await typeQuery(container, 'agulha'); await tick();
  expect(container.textContent).not.toContain(m.mobile_busca_vazio_parcial());
  await act(async () => pending.resolve([]));
  expect(container.textContent).toContain(m.mobile_busca_vazio_parcial());
  expect(container.textContent).not.toContain(m.busca_nenhum_resultado());
  h.search.mockRejectedValue(new Error('503'));
  await typeQuery(container, 'outra'); await tick();
  expect(container.textContent).not.toContain(m.mobile_busca_vazio_parcial());
  expect(container.textContent).not.toContain(m.busca_nenhum_resultado());
});

it('voltar do ponto conserva campo, lista, ordem, expansão e rolagem sem repetir busca completa', async () => {
  h.search.mockImplementation((server: Server) => Promise.resolve(server.id === 'a'
    ? [hit({ line: 'mais antiga', event_id: 'old', ts: 1 }), hit({ line: 'mais recente', event_id: 'new', ts: 9 })] : []));
  h.context.mockResolvedValue([{ id: 'new', kind: 'assistant_msg', text: 'contexto preservado' }]);
  const { container, root } = await mount(); await typeQuery(container, 'agulha'); await tick();
  const input = container.querySelector('input')!;
  const list = container.querySelector<HTMLElement>('[data-search-list]')!;
  const snippets = [...container.querySelectorAll<HTMLButtonElement>('button')].filter((button) => ['mais recente', 'mais antiga'].includes(button.textContent ?? ''));
  expect(snippets.map((button) => button.textContent)).toEqual(['mais recente', 'mais antiga']);
  await act(async () => snippets[0].click()); list.scrollTop = 360;
  expect(document.activeElement).toBe(input);
  await act(async () => buttons(container, m.busca_abrir_ponto())[0].click());
  h.focused = false; await act(async () => root.render(createElement(SearchScreen)));
  expect(container.querySelector('input'), 'campo permanece montado no blur').toBe(input);
  expect(container.querySelector('[data-search-list]')).toBe(list);
  expect(document.activeElement).not.toBe(input);
  expect(h.dismiss).toHaveBeenCalled();
  h.focused = true; await act(async () => root.render(createElement(SearchScreen))); await tick();
  expect(container.querySelector('input')).toBe(input);
  expect(container.querySelector('[data-search-list]')).toBe(list);
  expect(list.scrollTop).toBe(360);
  expect(document.activeElement).not.toBe(input);
  expect(container.textContent).toContain('contexto preservado');
  const returned = [...container.querySelectorAll<HTMLButtonElement>('button')].filter((button) => ['mais recente', 'mais antiga'].includes(button.textContent ?? ''));
  expect(returned).toEqual(snippets);
  expect(h.context).toHaveBeenCalledTimes(1);
  expect(h.search).toHaveBeenCalledTimes(2);
});

it('busca incompleta retoma só máquinas pendentes sem apagar sucessos nem ressuscitar resposta anterior', async () => {
  const old = deferred<SearchHit[]>(), resumed = deferred<SearchHit[]>();
  let bCalls = 0;
  h.search.mockImplementation((server: Server) => server.id === 'a' ? Promise.resolve([hit()]) : ++bCalls === 1 ? old.promise : resumed.promise);
  const { container, root } = await mount(); await typeQuery(container, 'agulha'); await tick();
  const success = [...container.querySelectorAll('button')].find((button) => button.textContent === 'agulha azul');
  expect(success).toBeDefined();
  h.focused = false; await act(async () => root.render(createElement(SearchScreen)));
  h.focused = true; await act(async () => root.render(createElement(SearchScreen)));
  await act(async () => old.resolve([hit({ line: 'resposta invalidada', event_id: 'invalid' })]));
  expect(container.textContent).not.toContain('resposta invalidada');
  expect(container.textContent).toContain('agulha azul');
  await tick();
  expect(h.search.mock.calls.map(([server]) => server.id)).toEqual(['a', 'b', 'b']);
  expect([...container.querySelectorAll('button')].find((button) => button.textContent === 'agulha azul')).toBe(success);
  await act(async () => resumed.resolve([hit({ line: 'resposta retomada', event_id: 'resumed' })]));
  expect(container.textContent).toContain('resposta retomada');
  expect(container.textContent).not.toContain(m.switcher_buscando());
});

it('resposta velha não entra durante debounce novo nem depois de perder foco', async () => {
  const old = deferred<SearchHit[]>(); h.search.mockReturnValue(old.promise);
  const { container, root } = await mount(); await typeQuery(container, 'velha'); await tick();
  await typeQuery(container, 'nova'); await act(async () => old.resolve([hit({ line: 'não mostrar' })]));
  expect(container.textContent).not.toContain('não mostrar');
  h.search.mockResolvedValue([hit({ line: 'nova resposta' })]); await tick(); expect(container.textContent).toContain('nova resposta');
  const hidden = deferred<SearchHit[]>(); h.search.mockReturnValue(hidden.promise);
  await typeQuery(container, 'oculta'); await tick(); h.focused = false;
  await act(async () => root.render(createElement(SearchScreen)));
  await act(async () => hidden.resolve([hit({ line: 'resposta escondida' })]));
  expect(container.textContent).not.toContain('resposta escondida');
  expect(buttons(container, m.busca_abrir_ponto())).toHaveLength(0);
});

it('Perguntar separa ícone do texto e do nome acessível sem símbolo duplicado', async () => {
  const { container } = await mount(); await typeQuery(container, 'agulha'); await tick();
  const ask = buttons(container, m.mobile_busca_perguntar())[0];
  expect(ask, 'Perguntar sem símbolo no nome acessível').toBeDefined();
  expect(ask.getAttribute('aria-label')).toBe(m.mobile_busca_perguntar());
  expect(ask.textContent).toBe(m.mobile_busca_perguntar());
  expect(ask.textContent).not.toContain('✦');
});

it('Perguntar abandona tentativa pendente no blur mas conserva resposta concluída ao voltar', async () => {
  const old = deferred<{ answer: string; hits: SearchHit[] }>(); h.ask.mockReturnValueOnce(old.promise);
  const { container, root } = await mount(); await typeQuery(container, 'agulha'); await tick();
  await act(async () => buttons(container, m.mobile_busca_perguntar())[0].click());
  h.focused = false; await act(async () => root.render(createElement(SearchScreen)));
  h.focused = true; await act(async () => root.render(createElement(SearchScreen)));
  await act(async () => old.resolve({ answer: 'tentativa invalidada', hits: [] }));
  expect(container.textContent).not.toContain('tentativa invalidada');
  const retry = buttons(container, m.mobile_busca_perguntar())[0];
  expect(retry, 'tentativa interrompida libera Perguntar').toBeDefined();
  expect(retry.disabled).toBe(false);
  h.ask.mockResolvedValueOnce({ answer: '**resposta preservada**', hits: [hit({ event_id: 'ask', line: 'fonte preservada' })] });
  await act(async () => retry.click());
  const answer = container.querySelector('[data-markdown]'); expect(answer?.textContent).toBe('**resposta preservada**');
  h.focused = false; await act(async () => root.render(createElement(SearchScreen)));
  h.focused = true; await act(async () => root.render(createElement(SearchScreen))); await tick();
  expect(container.querySelector('[data-markdown]')).toBe(answer);
  expect(container.textContent).toContain('fonte preservada');
  expect(h.ask).toHaveBeenCalledTimes(2);
  expect(h.search).toHaveBeenCalledTimes(2);
});

it('Perguntar usa só ativo próprio e não publica resposta de pergunta anterior', async () => {
  const old = deferred<{ answer: string; hits: SearchHit[] }>(); h.ask.mockReturnValueOnce(old.promise);
  const { container } = await mount(); await typeQuery(container, 'primeira'); await tick();
  await act(async () => buttons(container, m.mobile_busca_perguntar())[0].click());
  expect(h.ask).toHaveBeenCalledWith(state.servers[1], 'primeira');
  await typeQuery(container, 'segunda'); await act(async () => old.resolve({ answer: '**antiga**', hits: [hit()] }));
  expect(container.textContent).not.toContain('antiga'); await tick();
  h.ask.mockResolvedValueOnce({ answer: '**nova**', hits: [hit({ line: 'fonte da resposta', event_id: 'ask-hit' })] });
  await act(async () => buttons(container, m.mobile_busca_perguntar())[0].click());
  expect(container.querySelector('[data-markdown]')?.textContent).toBe('**nova**');
  expect(container.textContent).toContain('fonte da resposta');
});

it.each(['foco', 'query'])('Perguntar permite nova tentativa ao retornar pela mesma %s', async (change) => {
  const old = deferred<{ answer: string; hits: SearchHit[] }>();
  h.ask.mockReturnValueOnce(old.promise);
  const { container, root } = await mount(); await typeQuery(container, 'agulha'); await tick();
  await act(async () => buttons(container, m.mobile_busca_perguntar())[0].click());
  if (change === 'foco') {
    h.focused = false; await act(async () => root.render(createElement(SearchScreen)));
    h.focused = true; await act(async () => root.render(createElement(SearchScreen)));
  } else {
    await typeQuery(container, 'outra'); await typeQuery(container, 'agulha');
  }
  const retry = buttons(container, m.mobile_busca_perguntar())[0];
  expect(retry, 'Perguntar disponível ao voltar').toBeDefined();
  expect(retry.disabled).toBe(false);
  h.ask.mockResolvedValueOnce({ answer: '**nova tentativa**', hits: [] });
  await act(async () => retry.click());
  expect(h.ask).toHaveBeenCalledTimes(2);
  await act(async () => old.resolve({ answer: 'obsoleta', hits: [] }));
  expect(container.querySelector('[data-markdown]')?.textContent).toBe('**nova tentativa**');
  expect(container.textContent).not.toContain('obsoleta');
});

it.each([
  [true, 'conversa viva', 'conversa viva', 'conversa viva'],
  [false, 'conversa arquivada', '', 'conversa arquivada'],
  [false, null, '', '12345678'],
])('ponto recebe título separado do nome da sessão viva: %s / %s', async (live, sessionName, routeName, conversationTitle) => {
  h.search.mockImplementation((server: Server) => Promise.resolve(server.id === 'a'
    ? [hit({ live, session_name: sessionName, session_id: '123456789abcdef' })] : []));
  const { container } = await mount(); await typeQuery(container, 'agulha'); await tick();
  await act(async () => buttons(container, m.busca_abrir_ponto())[0].click());
  expect(h.push).toHaveBeenCalledWith({ pathname: '/search-point', params: {
    server: 'a', project: 'proj', sid: '123456789abcdef', eventId: 'ev-a', name: routeName, conversationTitle,
  } });
});

it('contexto do assistente usa Markdown e abre ponto com identidade explícita', async () => {
  h.search.mockImplementation((server: Server) => Promise.resolve(server.id === 'a' ? [hit()] : []));
  h.context.mockResolvedValue([{ id: 'ev-a', kind: 'assistant_msg', text: '**agulha azul**' }]);
  const { container } = await mount(); await typeQuery(container, 'agulha'); await tick();
  const snippet = [...container.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.includes('agulha azul'))!;
  await act(async () => snippet.click());
  expect(container.querySelector('[data-markdown]')?.textContent).toBe('**agulha azul**');
  await act(async () => buttons(container, m.busca_abrir_ponto())[0].click());
  expect(h.push).toHaveBeenCalledExactlyOnceWith({ pathname: '/search-point', params: {
    server: 'a', project: 'proj', sid: 'sid', eventId: 'ev-a', name: 'live-a', conversationTitle: 'live-a',
  } });
});

it('expande vizinhança de A sem trocar servidor ativo', async () => {
  h.search.mockImplementation((server: Server) => Promise.resolve(server.id === 'a' ? [hit()] : []));
  h.context.mockResolvedValue([{ id: 'before', kind: 'user_msg', text: 'vizinha anterior' }, { id: 'ev-a', kind: 'assistant_msg', text: 'agulha azul' }]);
  const { container } = await mount(); await typeQuery(container, 'agulha'); await tick();
  const snippet = [...container.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent?.includes('agulha azul'))!;
  expect(snippet).toBeDefined(); await act(async () => snippet.click());
  expect(h.context).toHaveBeenCalledExactlyOnceWith(state.servers[0], 'proj', 'sid', 'ev-a', undefined, undefined);
  expect(container.textContent).toContain('vizinha anterior'); expect(state.activeId).toBe('b');
});
