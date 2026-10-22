// @vitest-environment happy-dom
import React, { act, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { AppState, type AppStateStatus } from 'react-native';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureLocale, localeAtual, mergeReports, zeroUso, type ComboRow, type CostReport, type Server, type UsoFiltros } from '@hangar/core';
const net = vi.hoisted(() => ({ costs: vi.fn(), usage: vi.fn(), push: vi.fn(), back: vi.fn(), screen: vi.fn(), canGoBack: false, focused: true }));
vi.mock('@hangar/core', async (original) => ({ ...await original<typeof import('@hangar/core')>(), fetchCostsForServer: net.costs, fetchUsoForServer: net.usage }));
vi.mock('react-native-unistyles', async (original) => {
  const module = await original<typeof import('react-native-unistyles')>();
  return { ...module, useUnistyles: () => { const value = module.useUnistyles(); return { ...value, theme: new Proxy(value.theme, {}) }; } };
});
const router = { push: net.push, back: net.back, canGoBack: () => net.canGoBack };
vi.mock('expo-router', () => ({
  useRouter: () => router,
  useIsFocused: () => net.focused,
  Stack: { Screen: ({ options }: { options: { title: string; headerLeft?: () => ReactNode; headerRight?: () => ReactNode } }) => {
    net.screen(options);
    return React.createElement(React.Fragment, null, options.headerLeft?.(), options.headerRight?.());
  } },
}));
vi.mock('expo-router/react-navigation', () => ({ useHeaderHeight: () => 72 }));
vi.mock('expo-blur', () => ({ BlurView: ({ children }: { children: ReactNode }) => children }));
vi.mock('expo-glass-effect', () => ({ GlassView: ({ children }: { children: ReactNode }) => children, isLiquidGlassAvailable: () => false }));
vi.mock('expo-secure-store', () => ({ getItemAsync: vi.fn(async () => null), setItemAsync: vi.fn(async () => {}), deleteItemAsync: vi.fn(async () => {}) }));
vi.mock('../../ui/Screen', () => ({ Screen: ({ children }: { children: ReactNode }) => children }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ open, children }: { open: boolean; children: ReactNode }) => React.createElement('section', { 'data-sheet-mounted': true }, open ? React.createElement('aside', null, children) : null) }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('react-native', async (original) => {
  const module = await original<typeof import('react-native')>();
  return { ...module,
    Pressable: ({ onPress, ...props }: React.ComponentProps<typeof module.Pressable>) => React.createElement(module.Pressable, { ...props, onPress: () => onPress?.({ nativeEvent: { locationX: 0 } } as never) }),
    TextInput: ({ value, onChangeText, accessibilityLabel }: { value: string; onChangeText: (text: string) => void; accessibilityLabel: string }) => React.createElement('input', { value, 'aria-label': accessibilityLabel, onInput: (event: React.FormEvent<HTMLInputElement>) => onChangeText(event.currentTarget.value) }),
  };
});
vi.mock('react-native-svg', () => {
  const tag = (name: string) => ({ children, ...props }: Record<string, unknown>) => React.createElement(name, props, children as ReactNode);
  return { default: tag('svg'), Rect: tag('rect'), Line: tag('line'), Polyline: tag('polyline'), Circle: tag('circle'), Text: tag('text') };
});
import CostsScreen from '../../../app/costs';
import StatsScreen from '../../../app/stats';
import { useServers } from '../../stores/servers';
import { useUsagePreferences } from './ReportUI';
import * as m from '../../paraglide/messages';
import { getLocale, overwriteGetLocale } from '../../paraglide/runtime';
(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const a: Server = { id: 'a', label: 'Machine A', baseUrl: 'https://a.test', token: 'token' };
const b: Server = { ...a, id: 'b', label: 'Machine B' };
const combo: ComboRow = { dia: '2026-10-01', provider: 'provider-one', source: 'claude', project: '/project-one', model: 'missing', subagente: false, sessions: 1, session_ids: ['one'], input: 100, output: 20, cache_write: 10, cache_read: 50, cost: 0, cost_input: 0, cost_output: 0, cost_cache_write: 0, cost_cache_read: 0 };
const costs = (period: string): Partial<CostReport> => {
  const full = mergeReports([{ id: a.id, report: { applied: { period }, totals: { key: 'totals', ...combo }, combos: [combo], by_model: [{ key: combo.model, ...combo }], by_provider: [{ key: combo.provider, ...combo }], by_project: [{ key: combo.project, ...combo }], by_day: [{ key: combo.dia, ...combo }], sessoes: [{ ...combo, session_id: 'one', inicio: combo.dia, fim: combo.dia, subagentes: 0, custo_regravado: 0 }] } }], period).report;
  return full;
};
const skill = { ...zeroUso('plugin:heavy'), plugin: 'plugin', chamadas: 4, ctx_tokens_est: 20, ocupados_eq_tokens_est: 800, input: 10, output: 2, cost: 0.1, sessions: 2 };
const usage = (period: string, filters: UsoFiltros) => ({ ...filters, applied: { period }, totals: skill, by_skill: [skill], by_plugin: [{ ...skill, key: 'plugin' }], by_conta: [{ ...zeroUso('account'), label: 'account' }], by_projeto: [zeroUso('/project-one')], by_modelo: [zeroUso('known')], by_agente: [{ ...skill, key: 'reviewer', input: 200, output: 40, cost: 2 }], by_tool: [{ ...skill, key: 'Read' }, { ...skill, key: 'Agent', chamadas: 100 }], by_contexto: [{ ...skill, key: 'instructions' }], by_area: [{ ...skill, key: 'front' }], by_day: filters.foco ? [{ ...skill, key: '2026-10-02' }] : [{ ...skill, key: '2026-10-01' }] });
let root: Root;
let node: HTMLDivElement;
const click = async (label: string) => {
  const find = () => [...node.querySelectorAll('button')].find((b) => b.getAttribute('aria-label') === label || b.getAttribute('aria-label')?.startsWith(`${label}: `) || b.textContent === label || /^\d{4}-\d{2}-\d{2}$/.test(label) && b.textContent?.startsWith(`${label}: `));
  if (!find() && !node.querySelector('aside')) {
    if ([m.custos_dim_provedor(), m.custos_dim_fonte(), m.custos_dim_projeto(), m.custos_dim_modelo(), m.custos_dim_maquina(), m.custos_servidores_relatorio(), m.custos_limpar_filtros(), m.custos_so_subagente(), m.custos_so_conversa()].some((value) => value === label)) await click(m.custos_filtros());
    else if ([m.custos_periodo_1d(), m.custos_periodo_7d(), m.custos_periodo_30d(), m.custos_periodo_90d(), m.custos_periodo_tudo()].some((value) => value === label)) await click(m.custos_periodo());
    else if (/^\d{4}-\d{2}-\d{2}$/.test(label)) await click(m.custos_dia());
  }
  const button = find();
  expect(button, label).toBeTruthy();
  await act(async () => button!.click());
};
const previousLocale = localeAtual();
const previousMessageLocale = getLocale();
beforeEach(() => {
  configureLocale({ getLocale: () => 'pt' });
  overwriteGetLocale(() => 'pt');
  vi.clearAllMocks(); net.focused = true; net.canGoBack = false;
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve(costs(period)));
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve(usage(period, filters)));
  useServers.setState({ servers: [a, b, { ...a, id: 'guest', invite: true }, { ...a, id: 'disabled', disabled: true }] });
  useUsagePreferences.setState({ period: '30d', off: [] });
  node = document.createElement('div'); root = createRoot(node);
});
afterEach(async () => { await act(async () => root.unmount()); configureLocale({ getLocale: () => previousLocale }); overwriteGetLocale(() => previousMessageLocale); });

it('atalhos CLI acumulam escolhas e removem só o valor tocado', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, source: 'claude', cost: 4 }, { ...combo, source: 'codex', cost: 7 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const shortcuts = () => [...node.querySelectorAll<HTMLButtonElement>('button')].filter((button) => /^(claude|codex) ·/.test(button.getAttribute('aria-label') ?? ''));
  const shortcut = (source: string) => shortcuts().find((button) => button.getAttribute('aria-label')?.startsWith(`${source} ·`))!;
  await act(async () => shortcut('claude').click());
  await act(async () => shortcut('codex').click());
  const summary = () => [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_custo_periodo())!.parentElement!;
  expect(summary().textContent).toContain('US$ 11,00');
  await act(async () => shortcut('claude').click());
  expect(summary().textContent).toContain('US$ 7,00');
});

it('recorte vazio usa ausência e aviso de tarifa considera só a seleção', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), sem_tarifa: ['missing'], combos: [{ ...combo, model: 'known', source: 'claude', cost: 4 }, { ...combo, model: 'missing', source: 'codex', cost: 0 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click('claude');
  expect(node.textContent).not.toContain(m.custos_estimativa_incompleta({ n: 1 }));
  await click(m.custos_so_subagente());
  const summary = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_custo_periodo())!.parentElement!;
  expect(summary.textContent).toContain(m.custos_recorte_vazio());
  expect(summary.textContent).not.toContain('US$ 0,00');
});

it('período novo remove escolhas indisponíveis sem apagar as que ainda existem', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, source: 'claude', project: period === '30d' ? '/old-project' : '/new-project', cost: 4 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_dim_projeto());
  await click(`${m.custos_dim_projeto()}: /old-project`);
  await click(m.native_close());
  await click('claude');
  await click(m.custos_periodo_7d());
  await click(m.custos_dim_projeto());
  expect(node.querySelector('aside')?.textContent).not.toContain('/old-project');
  expect(node.querySelector('aside')?.textContent).toContain('/new-project');
});

it('Custos começa compacto com filtros recolhidos e intervalo inteiro no gráfico', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: Array.from({ length: 90 }, (_, index) => ({ ...combo, dia: new Date(Date.UTC(2026, 7, 1 + index)).toISOString().slice(0, 10) })) }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  expect(node.querySelector(`button[aria-label="${m.custos_dim_projeto()}"]`)).toBeNull();
  await click(m.custos_filtros());
  expect(node.querySelector(`button[aria-label="${m.custos_dim_projeto()}"]`)).toBeTruthy();
  await click(m.custos_filtros());
  expect(node.querySelector(`button[aria-label="${m.custos_dim_projeto()}"]`)).toBeNull();
  const chart = node.querySelector(`[aria-label="${m.custos_grafico_diario({ metrica: m.ctx_tokens() })}"]`)!;
  expect(Number(chart.querySelector('svg')?.getAttribute('width'))).toBeLessThanOrEqual(264);
  expect(chart.querySelectorAll('rect')).toHaveLength(270);
});

it('topo de Custos prioriza período abrangência e indicadores e conserva contexto na metodologia', async () => {
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), usd_brl: 5 }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  expect(node.textContent).not.toContain(m.uso_painel_titulo());
  expect(node.textContent).not.toContain(m.custos_aviso_estimativa());
  expect(node.textContent).not.toContain(m.custos_usd_brl({ taxa: '5' }));
  expect(node.textContent).not.toContain(m.custos_preco_tabela());
  expect(node.textContent).toContain(`${m.custos_periodo_30d()} · ${m.custos_somando_maquinas({ n: 2, m: 2 })}`);
  const period = node.querySelector<HTMLButtonElement>(`button[aria-label="${m.custos_periodo()}"]`)!;
  expect(period.textContent).toBe(m.custos_periodo_30d());
  expect(net.screen.mock.calls.at(-1)![0].headerRight).toBeTypeOf('function');
  await click(m.uso_painel_titulo());
  expect(net.push).toHaveBeenCalledWith('/stats');
  await click(m.custos_como_calculamos());
  expect(node.textContent).toContain(m.custos_aviso_estimativa());
  expect(node.textContent).toContain(m.custos_usd_brl({ taxa: '5' }));
  expect(node.textContent).toContain(m.custos_preco_tabela());
  await act(async () => root.render(React.createElement(StatsScreen)));
  expect(net.screen.mock.calls.at(-1)![0].headerRight).toBeUndefined();
  expect(node.textContent).toContain(m.nav_custos());
});

it('atalho legado troca o único CLI selecionado quando não há detalhamento', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [], by_source: [{ ...combo, key: 'claude', cost: 4 }, { ...combo, key: 'codex', cost: 7 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const shortcut = (source: string) => [...node.querySelectorAll<HTMLButtonElement>('button')].find((button) => button.getAttribute('aria-label')?.startsWith(`${source} ·`))!;
  await act(async () => shortcut('claude').click());
  await act(async () => shortcut('codex').click());
  const summary = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_custo_periodo())!.parentElement!;
  expect(summary.textContent).toContain('US$ 7,00');
});

it('falha parcial ao atualizar conserva o projeto exclusivo da máquina ausente', async () => {
  let offline = false;
  net.costs.mockImplementation((server: Server, period: string) => server.id === b.id && offline ? Promise.reject(new Error('offline')) : Promise.resolve({ ...costs(period), combos: [{ ...combo, project: server.id === b.id ? '/wanted' : '/other', cost: 4 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_dim_projeto()); await click(`${m.custos_dim_projeto()}: /wanted`); await click(m.native_close());
  offline = true;
  await click(m.custos_atualizar());
  expect(node.textContent).toContain(`${m.custos_dim_projeto()}: /wanted`);
  expect(node.textContent).toContain(`Machine B: ${m.native_network_error()}`);
  const summary = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_custo_periodo())!.parentElement!;
  expect(summary.textContent).not.toContain('US$ 4,00');
});

it.each([CostsScreen, StatsScreen])('preserva opções do cabeçalho ao redesenhar relatório sem alteração', async (Screen) => {
  await act(async () => root.render(React.createElement(Screen)));
  const initial = net.screen.mock.calls.at(-1)![0];
  await act(async () => root.render(React.createElement(Screen)));
  expect(net.screen.mock.calls.at(-1)![0]).toBe(initial);
});

it.each([false, true])('relatórios configuram cabeçalho nativo e voltar alternativo somente sem histórico: %s', async (canGoBack) => {
  net.canGoBack = canGoBack;
  for (const [Screen, title, sibling] of [
    [CostsScreen, m.nav_custos(), m.uso_painel_titulo()],
    [StatsScreen, m.uso_painel_titulo(), m.uso_ir_custos()],
  ] as const) {
    await act(async () => root.render(React.createElement(Screen)));
    expect(net.screen).toHaveBeenLastCalledWith(expect.objectContaining({ title, headerShown: true, headerTransparent: true }));
    const options = net.screen.mock.calls.at(-1)![0];
    expect(typeof options.headerBackground).toBe('function');
    if (canGoBack) {
      expect(options.headerLeft).toBeUndefined();
      expect(node.querySelector(`button[aria-label="${m.comum_voltar()}"]`)).toBeNull();
    } else {
      expect(typeof options.headerLeft).toBe('function');
      const before = net.back.mock.calls.length;
      await click(m.comum_voltar());
      expect(net.back).toHaveBeenCalledTimes(before + 1);
    }
    await click(sibling);
  }
  expect(net.push).toHaveBeenCalledWith('/stats');
});

it.each(['costs', 'stats'] as const)('folhas fechadas não montam e desmontam ao fechar em %s', async (screen) => {
  await act(async () => root.render(React.createElement(screen === 'costs' ? CostsScreen : StatsScreen)));
  const sheets = () => node.querySelectorAll('[data-sheet-mounted]');
  expect(sheets()).toHaveLength(0);
  await click(m.custos_servidores_relatorio());
  expect(sheets()).toHaveLength(1);
  expect(node.querySelector('aside')?.textContent).toContain('Machine A');
  await click(m.native_close());
  expect(sheets()).toHaveLength(0);
  await click(screen === 'costs' ? '2026-10-01' : 'heavy');
  expect(sheets()).toHaveLength(1);
  expect(node.querySelector('aside')?.textContent).toContain(screen === 'costs' ? '360' : '02/10');
  if (screen === 'stats') expect(node.querySelector('aside button[aria-label^="2026-10-02:"]')).toBeTruthy();
  await click(m.native_close());
  expect(sheets()).toHaveLength(0);
});

it('opções mostram só o item e preservam contexto acessível e chave do filtro', async () => {
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_servidores_relatorio());
  const server = node.querySelector<HTMLButtonElement>(`aside button[aria-label="${m.custos_servidores_relatorio()}: Machine B"]`)!;
  expect(server.textContent).toBe('Machine B');
  await act(async () => server.click());
  expect(useUsagePreferences.getState().off).toEqual(['b']);
  await click(m.native_close());
  await click(m.custos_dim_projeto());
  const project = node.querySelector<HTMLButtonElement>(`aside button[aria-label="${m.custos_dim_projeto()}: /project-one"]`)!;
  expect(project.textContent).toBe('/project-one');
  await act(async () => project.click());
  expect(net.usage).toHaveBeenLastCalledWith(a, '30d', { projeto: ['/project-one'], modelo: [] }, false);
});

it('barras de custos preservam rótulo e valores grandes em linhas próprias', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, input: 786_211_945, cost: 2_730.88, cost_input: 2_730.88 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const bar = node.querySelector(`[aria-label^="${m.custos_input_sem_cache()}: 786.211.945"]`)!;
  const label = [...bar.querySelectorAll('span')].find((span) => span.textContent === m.custos_input_sem_cache())!;
  const detail = [...bar.querySelectorAll('span')].find((span) => span.textContent?.startsWith('786.211.945 · US$ 2.730,88'))!;
  expect(detail.parentElement!.style.flexDirection).not.toBe('row');
  expect(label.parentElement).toBe(detail.parentElement);
  expect(label.parentElement!.style.flex).not.toBe('1');
  expect(bar.getAttribute('aria-label')).toContain(`${m.custos_input_sem_cache()}: 786.211.945 · US$ 2.730,88`);
});

it('custos sequenciais preservam oito indicadores e análises com controles compactos', async () => {
  await act(async () => root.render(React.createElement(CostsScreen)));
  const headings = [m.custos_periodo_30d(), m.mobile_costs_filters(), m.custos_todas_fontes(), m.custos_custo_periodo(), m.custos_tokens_novos(), m.custos_tokens_processados(), m.custos_cache_na_entrada(), m.custos_economia_cache(), m.custos_custo_por_milhao(), m.custos_cache_perdido(), m.custos_cache_1h(), m.custos_uso_por_dia(), m.custos_para_onde_dolar(), m.custos_o_que_cache(), m.custos_por_provedor(), m.custos_por_fonte(), m.custos_por_maquina(), m.uso_graf_areas(), m.custos_por_projeto(), m.custos_sessoes_caras(), m.custos_por_modelo(), m.lista_comparar(), m.custos_como_calculamos()];
  const text = node.textContent!;
  let previous = -1;
  for (const label of headings) {
    const index = text.indexOf(label, previous + 1);
    expect(index, label).toBeGreaterThan(previous);
    previous = index;
  }
  for (const label of [m.custos_uso_por_dia(), m.custos_por_provedor(), m.custos_por_projeto(), m.custos_por_modelo(), m.lista_comparar()]) expect(node.querySelector(`button[aria-label="${label}"]`)).toBeNull();
  const kpi = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_tokens_novos())!;
  expect(kpi.parentElement?.parentElement?.parentElement?.style.flexWrap).toBe('wrap');
  expect(text).not.toContain(m.custos_equivalente_cobrado());
  expect(text).not.toContain(m.custos_metodo_standard());
  expect(net.usage).toHaveBeenCalledWith(a, '30d', { projeto: [], modelo: [] }, false);
  await click(m.custos_como_calculamos());
  expect(node.textContent).toContain(m.custos_equivalente_cobrado());
  expect(node.textContent).toContain(m.custos_metodo_standard());
  await click(m.custos_como_calculamos());
  expect(node.textContent).not.toContain(m.custos_metodo_standard());
});

it('custos rodapés identificam sessões, subagentes, média, comparação, máquinas e reuso diário', async () => {
  useUsagePreferences.setState({ off: ['b'] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), anterior: { ...combo, key: 'previous', cost: 2 }, combos: [{ ...combo, cost: 3, custo_sem_cache: 6 }, { ...combo, subagente: true, session_ids: ['child'], cost: 1, custo_sem_cache: 2 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const card = (label: string) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)!.parentElement!;
  expect(card(m.custos_custo_periodo()).textContent).toContain(`1 ${m.sessao_singular()} · 1 ${m.custos_subagente()}`);
  expect(card(m.custos_custo_periodo()).textContent).toContain(`US$ 0,13 ${m.custos_por_dia()}`);
  expect(card(m.custos_custo_periodo()).textContent).toContain(m.custos_veio_subagente({ pct: '25%' }));
  expect(card(m.custos_custo_periodo()).textContent).toContain(m.custos_delta_vs({ n: '100', rot: m.custos_periodo_30d() }));
  expect(card(m.custos_custo_periodo()).textContent).toContain(m.custos_somando_maquinas({ n: 1, m: 2 }));
  expect(card(m.custos_cache_na_entrada()).textContent).toContain(m.custos_cada_gravado_lido({ x: '5' }));
  expect(card(m.custos_economia_cache()).textContent).toContain(m.custos_abaixo_preco_cheio({ n: '50' }));
  await click('2026-10-01');
  expect(node.querySelector('aside')?.textContent).toContain(`1 ${m.sessao_singular()} · 1 ${m.custos_subagente()}`);
  expect(node.querySelector('aside')?.textContent).toContain(m.custos_dia_reuso({ pct: '31,3%' }));
});

it('custos grátis e estimativa hipotética ausente não se confundem com economia zero', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, model: 'free-model:free' }], sem_tarifa: ['free-model:free'] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const card = (label: string) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)!.parentElement!;
  expect(card(m.custos_custo_periodo()).textContent).toContain('US$ 0,00');
  expect(card(m.custos_custo_periodo()).textContent).toContain(m.custos_modelo_gratis_nada());
  expect(node.textContent).not.toContain(m.custos_estimativa_incompleta({ n: 1 }));
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, cost: 2 }] }));
  await click(m.custos_atualizar());
  expect(card(m.custos_economia_cache()).textContent).toContain('—');
  expect(card(m.custos_economia_cache()).textContent).not.toContain('US$ 0,00');
});

it('custos públicos têm cartões honestos, seções sequenciais, comparação, áreas e navegação inteira', async () => {
  await act(async () => root.render(React.createElement(CostsScreen)));
  expect(node.textContent).toContain(m.custos_custo_periodo());
  expect(node.textContent).toContain(m.custos_sem_tarifa());
  expect(net.costs).toHaveBeenCalledWith(a, '30d', false, false);
  await click('2026-10-01');
  expect(node.querySelector('aside')?.textContent).toContain('360');
  await click(m.native_close());
  expect(node.textContent).toContain('missing');
  await click(m.custos_tirar_da_lista({ nome: '/project-one' }));
  expect(node.textContent).toContain(m.custos_fora_da_lista({ n: 1, valor: '—' }));
  expect(node.textContent).toContain(m.custos_comparar_hint({ n: 4 }));
  expect(node.textContent).toContain(m.custos_sessoes_hint());
  expect(node.textContent).toContain('front');
  expect(net.usage).toHaveBeenCalledWith(a, '30d', { projeto: [], modelo: [] }, false);
  await click(m.custos_como_calculamos());
  expect(node.textContent).toContain(m.custos_metodo_standard());
  await click(m.uso_painel_titulo());
  expect(net.push).toHaveBeenCalledWith('/stats');
  await click(m.comum_voltar());
  expect(net.back).toHaveBeenCalledTimes(1);
});

it.each([
  ['pt', '311,8 mi', 'US$ 2,7 mil', 'R$ 13,7 mil'],
  ['en', '311.8M', 'US$ 2.7K', 'R$ 13.7K'],
] as const)('gráfico compacta eixo e datas sem perder moeda ou valores exatos em %s', async (locale, tokenAxis, usdAxis, brlAxis) => {
  configureLocale({ getLocale: () => locale }); overwriteGetLocale(() => locale);
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), usd_brl: 5, combos: ['2026-10-01', '2026-10-02', '2026-10-03'].map((dia) => ({ ...combo, dia, input: 311_768_499, output: 0, cache_write: 0, cache_read: 0, cost: 2_730.88 })) }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const chart = (metric: string) => node.querySelector(`[aria-label="${m.custos_grafico_diario({ metrica: metric })}"]`)!;
  const axis = (metric: string) => chart(metric).firstElementChild!.firstElementChild as HTMLElement;
  const text = (element: Element) => element.textContent!.replace(/ /g, ' ');
  expect(text(axis(m.ctx_tokens()).firstElementChild!)).toBe(tokenAxis);
  expect(axis(m.ctx_tokens()).style.flexShrink).toBe('0');
  const scroll = axis(m.ctx_tokens()).nextElementSibling as HTMLElement;
  expect(Number.parseFloat(scroll.style.minWidth)).toBe(0); expect(scroll.style.flexShrink).toBe('1');
  expect(chart(m.ctx_tokens()).textContent).toContain('01/10'); expect(chart(m.ctx_tokens()).textContent).toContain('03/10');
  await click(m.custos_dia());
  const last = [...node.querySelectorAll<HTMLButtonElement>('aside button')].find((button) => button.textContent?.startsWith('2026-10-03:'))!;
  expect(last.getAttribute('aria-label')).toContain(locale === 'pt' ? '311.768.499' : '311,768,499');
  await act(async () => last.click());
  expect(node.querySelector('aside')?.textContent).toContain('2026-10-03');
  expect(node.querySelector('aside')?.textContent).toContain(locale === 'pt' ? '311.768.499' : '311,768,499');
  await click(m.native_close());
  await click(m.custos_estimativa_api());
  expect(text(axis(m.custos_estimativa_api()).firstElementChild!)).toBe(usdAxis);
  expect(chart(m.custos_estimativa_api()).querySelector('button')!.getAttribute('aria-label')).toContain(locale === 'pt' ? 'US$ 2.730,88' : 'US$ 2,730.88');
  await click('BRL');
  expect(text(axis(m.custos_estimativa_api()).firstElementChild!)).toBe(brlAxis);
  expect(chart(m.custos_estimativa_api()).querySelector('button')!.getAttribute('aria-label')).toMatch(locale === 'pt' ? /R\$\s13\.654,40/ : /R\$13,654\.40/);
});

it.each([
  ['pt', 'R$ 13,7 mil', 'R$ 13.654,40'],
  ['en', 'R$ 13.7K', 'R$13,654.40'],
] as const)('eixo BRL compacto funciona sem compactação de moeda do Hermes em %s', async (locale, compact, detailed) => {
  const original = Number.prototype.toLocaleString;
  const formatter = vi.spyOn(Number.prototype, 'toLocaleString').mockImplementation(function (this: number, locales, options) {
    return original.call(this, locales, options?.style === 'currency' && options.notation === 'compact' ? { ...options, notation: 'standard' } : options);
  });
  try {
    configureLocale({ getLocale: () => locale }); overwriteGetLocale(() => locale);
    useServers.setState({ servers: [a] });
    net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), usd_brl: 5, combos: [{ ...combo, cost: 2_730.88 }] }));
    await act(async () => root.render(React.createElement(CostsScreen)));
    await click(m.custos_estimativa_api()); await click('BRL');
    const chart = node.querySelector(`[aria-label="${m.custos_grafico_diario({ metrica: m.custos_estimativa_api() })}"]`)!;
    const axis = chart.firstElementChild!.firstElementChild!;
    expect(axis.firstElementChild!.textContent?.replace(/ /g, ' ')).toBe(compact);
    expect(axis.lastElementChild!.textContent?.replace(/ /g, ' ')).toBe('R$ 0');
    const day = chart.querySelector<HTMLButtonElement>('button[aria-label^="2026-10-01:"]')!;
    expect(day.getAttribute('aria-label')?.replace(/ /g, ' ')).toContain(detailed);
    await act(async () => day.click());
    expect(node.querySelector('aside')?.textContent?.replace(/ /g, ' ')).toContain(detailed);
  } finally {
    formatter.mockRestore();
  }
});

it.each(['pt', 'en'] as const)('diário usa colunas empilhadas tocáveis, legenda e custo por fonte em %s', async (locale) => {
  configureLocale({ getLocale: () => locale }); overwriteGetLocale(() => locale);
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, cost: 4, cost_input: 1, cost_output: 2, cost_cache_write: 0.8, cost_cache_read: 0.2 }, { ...combo, dia: '2026-10-03', source: 'codex', cost: 6, cost_input: 2, cost_output: 3, cost_cache_write: 0.8, cost_cache_read: 0.2 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const chart = () => node.querySelector(`[aria-label="${m.custos_grafico_diario({ metrica: m.ctx_tokens() })}"]`)!;
  expect(node.textContent).toContain(m.custos_empilhado_por({ modo: m.custos_tipo_token() }));
  expect(node.textContent).toContain(m.custos_hover_detalhe());
  expect(node.textContent).not.toContain(m.mobile_costs_daily_hint());
  expect(chart()).toBeTruthy();
  expect(chart().querySelectorAll('rect')).toHaveLength(6);
  const dayButton = () => chart().querySelector('button[aria-label^="2026-10-01:"]')!;
  expect(dayButton().getAttribute('aria-label')).toContain('130');
  await click(m.custos_tipo_cache_lido());
  expect(chart().querySelectorAll('rect')).toHaveLength(8);
  expect(dayButton().getAttribute('aria-label')).toContain('180');
  await click('2026-10-02');
  expect(node.querySelector('aside')?.textContent).toContain(m.custos_tipo_cache_lido());
  expect(node.querySelector('aside')?.textContent).toContain('0');
  await click(m.native_close());
  await click(m.custos_estimativa_api());
  expect(node.textContent).toContain(m.custos_empilhado_por({ modo: m.custos_dim_fonte() }));
  const costChart = node.querySelector(`[aria-label="${m.custos_grafico_diario({ metrica: m.custos_estimativa_api() })}"]`)!;
  expect(costChart.querySelectorAll('rect')).toHaveLength(2);
  await click(m.custos_dia());
  expect(node.querySelector('aside')?.textContent).toContain('2026-10-03:');
  expect(node.querySelector('aside')?.textContent).toContain('codex');
  await click(m.native_close());
  await click('codex');
  expect(costChart.querySelectorAll('rect')).toHaveLength(1);
  await click('claude · 130');
  const kpi = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_tokens_processados())?.parentElement;
  expect(kpi?.textContent).toContain('180');
  expect(node.textContent).toContain(m.custos_por_modelo());
  await click(m.custos_todas_fontes());
  expect(kpi?.textContent).toContain('360');
});

it('diário legado estima por tipo e não desenha custo desconhecido como zero', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_estimativa_api());
  expect(node.textContent).toContain(m.custos_recorte_parcial());
  expect(node.textContent).toContain(m.custos_empilhado_por({ modo: m.custos_tipo_token() }));
  const chart = node.querySelector(`[aria-label="${m.custos_grafico_diario({ metrica: m.custos_estimativa_api() })}"]`)!;
  expect(chart.querySelectorAll('rect')).toHaveLength(0);
  expect(chart.querySelector('button')?.getAttribute('aria-label')).toContain('—');
});

it('comparação desenha linhas e pontos diários ou semanais com métrica independente', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [
    { ...combo, cost: 3 }, { ...combo, provider: 'provider-two', dia: '2026-10-03', cost: 7 },
    { ...combo, dia: '2026-10-09', input: 200, cost: 5 }, { ...combo, provider: 'provider-two', project: '/other', cost: 99 },
  ] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_dim_provedor()); await click(`${m.custos_dim_provedor()}: provider-one`); await click(m.native_close());
  await click(m.custos_dim_projeto()); await click(`${m.custos_dim_projeto()}: /project-one`); await click(m.native_close());
  await click(m.custos_entidades_comparadas()); await click(`${m.custos_entidades_comparadas()}: provider-one`); await click(`${m.custos_entidades_comparadas()}: provider-two`); await click(m.native_close());
  const chart = (metric: string, mode: string) => node.querySelector(`[aria-label="${m.custos_grafico_comp_aria({ metrica: metric, modo: mode })}"]`)!;
  expect(chart(m.ctx_tokens(), m.custos_dia())).toBeTruthy();
  expect(chart(m.ctx_tokens(), m.custos_dia()).querySelectorAll('polyline')).toHaveLength(2);
  expect(chart(m.ctx_tokens(), m.custos_dia()).querySelectorAll('circle')).toHaveLength(18);
  const section = [...node.querySelectorAll('span')].find((span) => span.textContent === m.lista_comparar())!.parentElement!;
  const costMetric = [...section.querySelectorAll('button')].find((button) => button.getAttribute('aria-label') === m.custos_estimativa_api())!;
  await act(async () => costMetric.click());
  expect(chart(m.custos_estimativa_api(), m.custos_dia()).querySelectorAll('circle')).toHaveLength(18);
  expect(node.querySelector(`[aria-label="${m.custos_grafico_diario({ metrica: m.ctx_tokens() })}"]`)).toBeTruthy();
  const entity = [...section.querySelectorAll('span')].find((span) => span.textContent === 'provider-two')!.parentElement!;
  expect(entity.textContent).toContain('US$ 7,00'); expect(entity.textContent).not.toContain('US$ 106,00');
  await click(m.custos_periodo_90d());
  expect(chart(m.custos_estimativa_api(), m.custos_semana()).querySelectorAll('circle')).toHaveLength(4);
  const weeklySection = chart(m.custos_estimativa_api(), m.custos_semana()).parentElement!;
  expect(weeklySection.textContent).toContain('28/09'); expect(weeklySection.textContent).toContain('05/10');
  expect(weeklySection.querySelector('button[aria-label^="2026-09-28:"]')).toBeTruthy();
  await act(async () => weeklySection.querySelector<HTMLButtonElement>(`button[aria-label="${m.custos_dia()}"]`)!.click());
  expect(node.querySelector('aside')?.textContent).toContain('2026-10-05:');
  await click(m.native_close());
  expect(weeklySection.textContent).toContain(m.custos_ponto_semana());
});

it.each([true, false])('provedores usam nomes das contas e prefixo único sem perder chave no filtro cruzado=%s', async (crossed) => {
  useServers.setState({ servers: [a] });
  const providers = [
    ['anthropic:11111111-1111-4111-8111-111111111111', 'Conta Azul', 'Claude Code · Conta Azul'],
    ['anthropic:22222222-2222-4222-8222-222222222222', 'Claude Code · Conta Verde', 'Claude Code · Conta Verde'],
    ['codex:33333333-3333-4333-8333-333333333333', 'Codex · Conta Ouro', 'Codex · Conta Ouro'],
    ['codex:44444444-4444-4444-8444-444444444444', 'Conta Prata', 'Codex · Conta Prata'],
    ['anthropic', 'Anthropic direto', 'Anthropic direto'],
  ];
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: crossed ? providers.map(([provider]) => ({ ...combo, provider, cost: 2 })) : [], by_provider: providers.map(([key, label]) => ({ ...combo, key, label, cost: 2 })) }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const ranking = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_por_provedor())!.parentElement!;
  for (const [, , label] of providers) {
    expect(ranking.querySelector(`button[aria-label^="${label}:"]`), label).toBeTruthy();
    expect(ranking.querySelectorAll(`button[aria-label^="${label}:"]`), label).toHaveLength(1);
  }
  expect(ranking.textContent).not.toContain('Claude Code · Claude Code ·');
  expect(ranking.textContent).not.toContain('Codex · Codex ·');
  expect(ranking.textContent).not.toContain('Claude Code · Anthropic direto');
  await click(m.custos_dim_provedor());
  const option = node.querySelector<HTMLButtonElement>(`aside button[aria-label="${m.custos_dim_provedor()}: Claude Code · Conta Azul"]`)!;
  expect(option.textContent).toBe('Claude Code · Conta Azul');
  await act(async () => option.click()); await click(m.native_close());
  const total = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_tokens_processados())!.parentElement!;
  expect([...total.querySelectorAll('span')].some((span) => span.textContent === '180')).toBe(true);
  await click(m.custos_limpar_filtros());
  await click(m.custos_entidades_comparadas());
  await click(`${m.custos_entidades_comparadas()}: Claude Code · Conta Azul`);
  await click(`${m.custos_entidades_comparadas()}: Codex · Conta Ouro`); await click(m.native_close());
  const comparison = [...node.querySelectorAll('span')].find((span) => span.textContent === m.lista_comparar())!.parentElement!;
  expect(comparison.textContent).toContain('Claude Code · Conta Azul'); expect(comparison.textContent).toContain('Codex · Conta Ouro');
  if (crossed) expect(comparison.querySelector('button[aria-label^="2026-10-01:"]')?.getAttribute('aria-label')).toContain('Claude Code · Conta Azul: 180');
});

it.each([
  ['Conta Azul', 'claude · Claude Code · Conta Azul · missing · Machine A'],
  [undefined, 'claude · Claude Code · anthropic:11111111-1111-4111-8111-111111111111 · missing · Machine A'],
] as const)('sessões caras usam conta legível quando disponível sem inventar rótulo: %s', async (label, expected) => {
  useServers.setState({ servers: [a] });
  const provider = 'anthropic:11111111-1111-4111-8111-111111111111';
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period),
    by_provider: [{ ...combo, key: provider, label }],
    sessoes: [{ ...combo, provider, session_id: 'one', inicio: combo.dia, fim: combo.dia, subagentes: 0, custo_regravado: 0 }],
  }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const sessions = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_sessoes_caras())!.parentElement!;
  const line = [...sessions.querySelectorAll('span')].find((span) => span.textContent?.startsWith('claude · '))!;
  expect(line.textContent).toBe(expected);
  if (label) expect(line.textContent).not.toContain('11111111-1111-4111-8111-111111111111');
  else expect(line.textContent).not.toContain('Conta');
});

it('comparação mostra média diária de tokens inteira sem arredondar total ou custo', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: ['provider-one', 'provider-two'].map((provider) => ({ ...combo, provider, input: 320, output: 0, cache_write: 0, cache_read: 0, cost: 2.73 })) }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_entidades_comparadas()); await click(`${m.custos_entidades_comparadas()}: provider-one`); await click(`${m.custos_entidades_comparadas()}: provider-two`); await click(m.native_close());
  const section = [...node.querySelectorAll('span')].find((span) => span.textContent === m.lista_comparar())!.parentElement!;
  const entity = [...section.querySelectorAll('span')].find((span) => span.textContent === 'provider-one')!.parentElement!;
  expect(entity.textContent).toContain(`11 ${m.custos_por_dia()}`);
  expect([...entity.querySelectorAll('span')].some((span) => span.textContent === '320')).toBe(true);
  expect(entity.textContent).toContain('US$ 2,73');
  const costMetric = [...section.querySelectorAll('button')].find((button) => button.getAttribute('aria-label') === m.custos_estimativa_api())!;
  await act(async () => costMetric.click());
  expect(entity.textContent).toContain(`US$ 0,09 ${m.custos_por_dia()}`);
});

it('comparação sem detalhamento conserva cartões sem inventar série', async () => {
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [], by_provider: [{ ...combo, key: 'provider-one' }, { ...combo, key: 'provider-two' }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_entidades_comparadas()); await click(`${m.custos_entidades_comparadas()}: provider-one`); await click(`${m.custos_entidades_comparadas()}: provider-two`); await click(m.native_close());
  const section = [...node.querySelectorAll('span')].find((span) => span.textContent === m.lista_comparar())!.parentElement!;
  expect(section.textContent).toContain(m.custos_sem_detalhamento()); expect(section.textContent).toContain('provider-two');
  expect(section.querySelector('polyline')).toBeNull(); expect(section.querySelector('circle')).toBeNull();
});

it('custos composição conserva custos e percentuais, economia e áreas ordenadas', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, cost: 10, cost_input: 2, cost_output: 6, cost_cache_write: 1, cost_cache_read: 1, custo_sem_cache: 20 }] }));
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_area: [{ ...skill, key: 'conversa', input: 2, output: 0 }, { ...skill, key: 'back', input: 6, output: 0 }, { ...skill, key: 'front', input: 2, output: 0 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const composition = node.querySelector(`[aria-label^="${m.custos_para_onde_dolar()}:"]`);
  expect(composition).toBeTruthy();
  expect(composition?.getAttribute('aria-label')).toContain('60%');
  expect([...composition!.children].map((part) => (part as HTMLElement).style.width)).toEqual(['20%', '60%', '10%', '10%']);
  const cache = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_o_que_cache())?.parentElement;
  expect(cache?.textContent).toContain(`${m.custos_se_nada_cache()}: US$ 20,00`);
  expect(cache?.textContent).toContain(`${m.custos_economizado()}: US$ 10,00`);
  expect(cache?.textContent).toContain('27,8%');
  const areas = [...node.querySelectorAll('span')].find((span) => span.textContent === m.uso_graf_areas())?.parentElement;
  const areaLabels = [...areas!.querySelectorAll('[aria-label]')].map((row) => row.getAttribute('aria-label')).filter((label) => /^(front|back|conversa):/.test(label ?? ''));
  expect(areaLabels.map((label) => label?.split(':')[0])).toEqual(['front', 'back', 'conversa']);
  expect(areaLabels[0]).toContain('20%');
  expect(node.querySelector('button[aria-label="' + m.custos_por_maquina() + '"]')).toBeNull();
});

it('réguas de custo usam participação em dólares na tela e no detalhe diário sem alterar tokens', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, cost: 10, cost_input: 2, cost_output: 6, cost_cache_write: 1, cost_cache_read: 1 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const check = (section: Element) => {
    [
      [m.custos_input_sem_cache(), '100 · US$ 2,00', '20%'],
      [m.custos_output_total(), '20 · US$ 6,00', '60%'],
      [m.custos_tipo_cache_escrito(), '10 · US$ 1,00', '10%'],
      [m.custos_tipo_cache_lido(), '50 · US$ 1,00', '10%'],
    ].forEach(([label, detail, width]) => {
      const bar = section.querySelector(`[aria-label^="${label}:"]`)!;
      expect(bar.getAttribute('aria-label')).toBe(`${label}: ${detail} · ${m.custos_col_pct_conta()}: ${width}`);
      expect((bar.firstElementChild!.lastElementChild!.firstElementChild as HTMLElement).style.width).toBe(width);
    });
  };
  const costsSection = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_para_onde_dolar())!.parentElement!;
  check(costsSection);
  await click('2026-10-01');
  check(node.querySelector('aside')!);
});

it('projetos carregam doze e mais cinquenta por toque com recolher antes das linhas expandidas', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: Array.from({ length: 1_338 }, (_, index) => ({ ...combo, project: `/project-${String(index).padStart(4, '0')}`, cost: 2 })) }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const projects = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_por_projeto())!.parentElement!;
  const rows = () => [...projects.querySelectorAll('button[aria-label^="/project-"]')];
  expect(rows()).toHaveLength(12);
  await click(m.custos_ver_projetos({ n: 1_338 }));
  expect(rows()).toHaveLength(62);
  await click(m.uso_mostrar_mais({ n: 50 }));
  expect(rows()).toHaveLength(112);
  const collapse = projects.querySelector<HTMLButtonElement>(`button[aria-label="${m.custos_mostrar_menos()}"]`)!;
  expect(collapse.compareDocumentPosition(rows()[12]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  await click(m.custos_tirar_da_lista({ nome: '/project-0050' }));
  expect(rows().some((row) => row.getAttribute('aria-label')?.startsWith('/project-0050:'))).toBe(false);
  await click(m.custos_mostrar_todos());
  expect(rows()).toHaveLength(112);
  await act(async () => collapse.click());
  expect(rows()).toHaveLength(12);
  expect(projects.textContent).not.toContain('/project-0111');
});

it.each(['atualizar', 'segundo plano'] as const)('projetos preservam ocultos e expansão sem mostrar dados velhos ao %s', async (transition) => {
  const listeners = new Set<(state: AppStateStatus) => void>();
  const appState = vi.spyOn(AppState, 'addEventListener').mockImplementation((_event, listener) => {
    listeners.add(listener);
    return { remove: () => { listeners.delete(listener); } };
  });
  try {
    useServers.setState({ servers: [a] });
    const rows = Array.from({ length: 120 }, (_, index) => ({ ...combo, project: `/project-${String(index).padStart(4, '0')}`, cost: 2 }));
    net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: rows }));
    await act(async () => root.render(React.createElement(CostsScreen)));
    await click(m.custos_tirar_da_lista({ nome: '/project-0000' }));
    await click(m.custos_ver_projetos({ n: 119 }));
    const section = () => [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_por_projeto())?.parentElement;
    expect(section()!.querySelectorAll('button[aria-label^="/project-"][aria-label*=":"]')).toHaveLength(62);
    let finish!: (report: Partial<CostReport>) => void;
    net.costs.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
    if (transition === 'atualizar') await click(m.custos_atualizar());
    else {
      await act(async () => listeners.forEach((listener) => listener('background')));
      expect(section()).toBeUndefined();
      expect(node.textContent).not.toContain('/project-0001');
      expect(node.textContent).not.toContain(m.custos_custo_periodo());
      await act(async () => listeners.forEach((listener) => listener('active')));
    }
    expect(node.textContent).toContain(m.custos_carregando_maquinas({ n: 1 }));
    expect(section()).toBeUndefined();
    expect(node.textContent).not.toContain('/project-0001');
    expect(node.textContent).not.toContain(m.custos_custo_periodo());
    await act(async () => finish({ ...costs('30d'), combos: rows.map((row) => ({ ...row, cost: 3 })) }));
    expect(section()!.querySelector('button[aria-label^="/project-0000:"]')).toBeNull();
    expect(section()!.querySelectorAll('button[aria-label^="/project-"][aria-label*=":"]')).toHaveLength(62);
    expect(section()!.textContent).toContain(m.custos_fora_da_lista({ n: 1, valor: 'US$ 3,00' }));
    const total = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_custo_periodo())!.parentElement!;
    expect(total.textContent).toContain('US$ 360,00');
  } finally {
    appState.mockRestore();
  }
});

it('projetos locais não recalculam o relatório ao esconder, restaurar, expandir e recolher', async () => {
  useServers.setState({ servers: [a] });
  const rows = Array.from({ length: 120 }, (_, index) => ({ ...combo, project: `/project-${String(index).padStart(4, '0')}`, cost: 2 }));
  let comboReads = 0;
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), get combos() { comboReads++; return rows; } }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const projects = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_por_projeto())!.parentElement!;
  const total = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_custo_periodo())!.parentElement!;
  const visible = () => [...projects.querySelectorAll('button[aria-label^="/project-"]')];
  const loadedReads = comboReads;
  expect(loadedReads).toBeGreaterThan(0);
  const unchanged = () => { expect(comboReads).toBe(loadedReads); expect(total.textContent).toContain('US$ 240,00'); };
  expect(visible()).toHaveLength(12);
  await click(m.custos_tirar_da_lista({ nome: '/project-0000' }));
  expect(visible().some((row) => row.getAttribute('aria-label')?.startsWith('/project-0000:'))).toBe(false);
  expect(projects.textContent).toContain(m.custos_fora_da_lista({ n: 1, valor: 'US$ 2,00' }));
  unchanged();
  await click(m.custos_mostrar_todos());
  expect(visible().some((row) => row.getAttribute('aria-label')?.startsWith('/project-0000:'))).toBe(true);
  expect(projects.textContent).not.toContain(m.custos_fora_da_lista({ n: 1, valor: 'US$ 2,00' }));
  unchanged();
  await click(m.custos_ver_projetos({ n: 120 }));
  expect(visible()).toHaveLength(62); unchanged();
  await click(m.uso_mostrar_mais({ n: 50 }));
  expect(visible()).toHaveLength(112); unchanged();
  const collapse = projects.querySelector<HTMLButtonElement>(`button[aria-label="${m.custos_mostrar_menos()}"]`)!;
  expect(collapse.compareDocumentPosition(visible()[12]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  await act(async () => collapse.click());
  expect(visible()).toHaveLength(12); unchanged();
});

it('modelos com cache lido preservam asteriscos e mostram uma nota depois da lista', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: ['model-one', 'model-two'].map((model) => ({ ...combo, model, cache_write: 0, cache_read: 50, cost: 2 })) }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const models = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_por_modelo())!.parentElement!;
  const notes = [...models.querySelectorAll('span')].filter((span) => span.textContent === m.custos_sem_cache_escrito_ajuda());
  expect(notes).toHaveLength(1);
  expect([...models.querySelectorAll('span')].filter((span) => span.textContent === `${m.custos_col_entrada()}: 150 / 100*`)).toHaveLength(2);
  const second = models.querySelector('button[aria-label^="model-two:"]')!;
  expect(second.compareDocumentPosition(notes[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
  expect(notes[0].parentElement).toBe(models);
});

it('custos listas expandem projetos e sessões e modelos conservam todos os campos', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period),
    combos: Array.from({ length: 14 }, (_, index) => ({ ...combo, project: `/project-${String(index).padStart(2, '0')}`, model: 'known', cost: 20 - index, cost_input: 2, cost_output: 3, cost_cache_write: 1, cost_cache_read: 1 })),
    rates: [{ model: 'known', provider: 'catalog', input: 1, output: 5, cache_write: 1.25, cache_read: 0.1, origin: 'snapshot', cache_estimado: true }], sem_tarifa: ['known'],
    sessoes: Array.from({ length: 31 }, (_, index) => ({ ...combo, model: 'known', session_id: `session-${index}`, project: `/session-${String(index).padStart(2, '0')}`, inicio: '2026-10-01', fim: '2026-10-02', cost: 40 - index, subagentes: 2, custo_regravado: 0.25 })),
  }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const section = (title: string) => [...node.querySelectorAll('span')].find((span) => span.textContent === title)?.parentElement!;
  const projects = section(m.custos_por_projeto());
  expect(projects.textContent).not.toContain('/project-12');
  await click(m.custos_ver_projetos({ n: 14 }));
  expect(projects.textContent).toContain('/project-13');
  await click(m.custos_tirar_da_lista({ nome: '/project-13' }));
  expect(projects.textContent).toContain(m.custos_fora_da_lista({ n: 1, valor: 'US$ 7,00' }));
  await click(m.custos_mostrar_todos());
  expect(projects.textContent).not.toContain(m.custos_fora_da_lista({ n: 1, valor: 'US$ 7,00' }));
  const sessions = section(m.custos_sessoes_caras());
  expect(sessions.textContent).toContain('/session-09');
  expect(sessions.textContent).not.toContain('/session-10');
  expect(sessions.textContent).toContain(m.custos_sessao_tokens_novos({ tokens: '130' }));
  expect(sessions.textContent).toContain(m.custos_sessao_cache_perdido({ valor: 'US$ 0,25' }));
  expect(sessions.textContent).toContain(`${m.custos_subagentes()}: 2`);
  await click(m.custos_ver_sessoes({ n: 30 }));
  expect(sessions.textContent).toContain('/session-29');
  expect(sessions.textContent).not.toContain('/session-30');
  const models = section(m.custos_por_modelo());
  for (const label of [m.custos_col_entrada(), m.custos_output_total(), m.custos_tipo_cache_lido(), m.custos_tarifa_in_out(), m.custos_origem(), m.custos_col_pct_conta(), m.custos_preco_parcial(), m.custos_cache_estimado()]) expect(models.textContent).toContain(label);
  expect(models.textContent).toContain('2.240 / 1.400');
  expect(models.textContent).toContain('snapshot');
  expect(models.textContent).toContain(`${m.custos_tipo_cache_escrito()}: US$ 1,25`);
  expect(models.textContent).toContain(`${m.custos_tipo_cache_lido()}: US$ 0,10`);
  await click(m.custos_como_calculamos());
  for (const label of [m.custos_fontes(), m.custos_tarifas(), m.custos_modelos_sem_tarifa(), m.custos_equivalente_cobrado()]) expect(node.textContent).toContain(label);
});

it('modelo com tarifas diferentes entre provedores mantém aviso de preço parcial', async () => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period),
    combos: [{ ...combo, model: 'shared-model', cost: 3 }], sem_tarifa: ['shared-model'],
    rates: [{ model: 'shared-model', provider: 'provider-one', input: 1, output: 5, cache_write: 1.25, cache_read: 0.1, origin: 'snapshot' }, { model: 'shared-model', provider: 'provider-two', input: 2, output: 10, cache_write: 2.5, cache_read: 0.2, origin: 'snapshot' }],
  }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const models = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_por_modelo())!.parentElement!;
  const model = models.querySelector('button[aria-label^="shared-model:"]')!.parentElement!.parentElement!;
  expect(model.textContent).toContain('US$ 3,00');
  expect(model.textContent).toContain('provider-one'); expect(model.textContent).toContain('provider-two');
  expect(model.textContent).toContain(m.custos_preco_parcial());
});

it('custos máquina mantém total do período mesmo após recortar projeto', async () => {
  net.costs.mockImplementation((server: Server, period: string) => Promise.resolve({ ...costs(period), totals: { ...combo, key: 'totals', cost: server.id === 'a' ? 10 : 20 }, combos: [{ ...combo, cost: 2 }, { ...combo, project: '/other', cost: 8 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_dim_projeto()); await click(`${m.custos_dim_projeto()}: /project-one`); await click(m.native_close());
  const machines = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_por_maquina())?.parentElement;
  expect(machines?.textContent).toContain(m.custos_onde_sessao());
  expect(machines?.querySelector('button[aria-label^="Machine A:"]')?.getAttribute('aria-label')).toContain('US$ 10,00');
  expect(machines?.querySelector('button[aria-label^="Machine B:"]')?.getAttribute('aria-label')).toContain('US$ 20,00');
});

it('estatísticas mostram seis respostas na ordem desktop e excluem grupos internos e valores zero', async () => {
  useServers.setState({ servers: [a] });
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters),
    by_skill: [{ ...skill, key: '@repo:internal', plugin: '@repo', ocupados_eq_tokens_est: 2_000 }, { ...skill, key: 'real:heavy', plugin: 'real', ocupados_eq_tokens_est: 800, chamadas: 2 }, { ...skill, key: 'real:loaded', plugin: 'real', ocupados_eq_tokens_est: 100, chamadas: 30 }],
    by_agente: [{ ...skill, key: '@nativo:internal-agent', plugin: '@nativo', cost: 20 }, { ...skill, key: 'real:review', plugin: 'real', cost: 4, chamadas: 3 }],
  }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  const labels = [m.uso_resp_plugin_skills(), m.uso_resp_skill_contexto(), m.uso_resp_skill_usada(), m.uso_resp_plugin_agentes(), m.uso_resp_agente_caro(), m.uso_resp_ferramenta()];
  const cards = labels.map((label) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)!.parentElement!);
  expect(cards.map((card) => card.querySelectorAll('span')[1].textContent)).toEqual(['real', 'internal', 'loaded', 'real', 'internal-agent', 'Read']);
  expect(cards[0].textContent).toContain(m.uso_resp_plugin_skills_sub({ n: '900', pct: '31' }));
  expect(cards[2].textContent).toContain(m.uso_resp_grupo_vezes({ grupo: 'real', n: '30' }));
  expect(cards[3].textContent).toContain(m.uso_resp_agente_custo_sub({ custo: 'US$ 4,00', n: '3' }));
  let index = -1; for (const label of labels) { const next = node.textContent!.indexOf(label); expect(next).toBeGreaterThan(index); index = next; }
  expect(cards[0].parentElement?.parentElement?.style.flexWrap).toBe('wrap');
  expect(node.textContent).not.toContain(m.mobile_usage_most_loaded_plugin());
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...filters, applied: { period }, totals: zeroUso('totals'), by_skill: [zeroUso('empty')], by_agente: [zeroUso('empty')], by_tool: [zeroUso('empty')] }));
  await click(m.custos_atualizar());
  for (const label of labels) expect([...node.querySelectorAll('span')].find((span) => span.textContent === label)?.parentElement?.textContent).toBe(`${label}—`);
});

it('estatísticas principais mantêm skills duplas, ferramentas e agentes visíveis durante busca e expansão', async () => {
  useServers.setState({ servers: [a] });
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters),
    by_skill: [{ ...skill, key: 'first:heavy', plugin: 'first', ocupados_eq_tokens_est: 800, chamadas: 2 }, { ...skill, key: 'first:light', plugin: 'first', ocupados_eq_tokens_est: 20, chamadas: 12 }, { ...skill, key: 'second:hidden-skill', plugin: 'second', ocupados_eq_tokens_est: 50 }],
    by_tool: [{ ...skill, key: 'Bash', chamadas: 100 }, ...Array.from({ length: 12 }, (_, index) => ({ ...skill, key: `tool-${index}`, chamadas: 20 - index })), { ...skill, key: 'Skill', chamadas: 999 }, { ...skill, key: 'Agent', chamadas: 999 }],
    by_bash: Array.from({ length: 6 }, (_, index) => ({ ...skill, key: `command-${index}`, chamadas: 10 - index })),
    by_agente: [{ ...skill, key: 'agents:review', plugin: 'agents', cost: 4, input: 101, output: 22, cache_write: 33, cache_read: 44, cost_input: 1, cost_output: 2, cost_cache_write: 0.6, cost_cache_read: 0.4 }],
  }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  const section = (label: string) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)!.parentElement!;
  const skills = () => section(m.uso_sec_por_plugin());
  expect(skills().querySelector('button[aria-label^="heavy:"]')).toBeTruthy();
  expect(skills().querySelector('button[aria-label^="hidden-skill:"]')).toBeNull();
  const heavy = skills().querySelector('button[aria-label^="heavy:"]')!;
  expect(heavy.textContent).toContain(`${m.uso_col_peso()}≈ 800`); expect(heavy.textContent).toContain(`${m.uso_col_cargas()}2`);
  await click('second'); expect(skills().querySelector('button[aria-label^="hidden-skill:"]')).toBeTruthy();
  await click(m.uso_ordenar_por({ col: m.uso_col_vezes() }));
  expect([...skills().querySelectorAll('button')].filter((button) => /^(heavy|light):/.test(button.getAttribute('aria-label') ?? '')).map((button) => button.getAttribute('aria-label')?.split(':')[0])).toEqual(['light', 'heavy']);
  const tools = section(m.uso_rank_ferramentas());
  expect(tools.querySelectorAll('button')).toHaveLength(12); expect(tools.textContent).toContain(m.uso_mais_na_tabela({ n: 1 }));
  expect(tools.textContent).toContain('command-4'); expect(tools.textContent).not.toContain('command-5');
  expect(tools.querySelector('button[aria-label^="Skill:"]')).toBeNull(); expect(tools.querySelector('button[aria-label^="Agent:"]')).toBeNull();
  const agents = section(m.uso_sub_titulo());
  expect(agents.textContent).toContain(m.uso_cache_legado_nota());
  for (const text of ['agents', 'review', '101 · US$ 1,00', '22 · US$ 2,00', '33 · US$ 0,60', '44 · US$ 0,40', `${m.uso_col_sessoes()}: 2`]) expect(agents.textContent).toContain(text);
  const search = node.querySelector<HTMLInputElement>('input')!;
  await act(async () => { search.value = 'hidden-skill'; search.dispatchEvent(new Event('input', { bubbles: true })); });
  expect(skills().textContent).not.toContain('heavy'); expect(skills().textContent).toContain('hidden-skill');
  expect(section(m.uso_rank_ferramentas()).textContent).toContain('Bash'); expect(section(m.uso_sub_titulo()).textContent).toContain('review');
  expect(section(m.uso_resp_skill_contexto()).textContent).toContain('heavy');
  for (const label of [m.uso_aba_skills(), m.uso_aba_tools(), m.uso_aba_agentes()]) expect(node.querySelector(`button[aria-label="${label}"]`)).toBeNull();
});

it('estatísticas avançadas reúnem colunas diárias, composição por sessão e oito tabelas ordenáveis de vinte itens', async () => {
  useServers.setState({ servers: [a] });
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters),
    totals: { ...skill, sessions: 2 },
    by_skill: Array.from({ length: 25 }, (_, index) => ({ ...skill, key: `plugin:item-${String(index).padStart(2, '0')}`, ocupados_eq_tokens_est: index + 1, chamadas: 25 - index, respostas: 100 + index })),
    by_contexto: [{ ...skill, key: 'instructions', ctx_tokens_est: 60 }, { ...skill, key: 'hooks', ctx_tokens_est: 40 }],
    by_day: [{ ...skill, key: '2026-10-01', chamadas: 2 }, { ...skill, key: '2026-10-03', chamadas: 8 }],
  }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  expect(node.textContent).not.toContain(m.uso_graf_chamadas_dia());
  await click(m.uso_avancado());
  const heading = (label: string) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)!.parentElement!;
  const daily = heading(m.uso_graf_chamadas_dia());
  expect(daily.querySelectorAll('rect')).toHaveLength(2);
  expect([...daily.querySelectorAll('rect')].map((rect) => rect.getAttribute('height'))).toEqual(['45', '180']);
  const composition = node.querySelector(`[aria-label^="${m.uso_graf_ctx()}:"]`)!;
  expect([...composition.children].map((part) => (part as HTMLElement).style.width)).toEqual(['60%', '0%', '40%', '0%', '0%']);
  expect(heading(m.uso_graf_ctx()).textContent).toContain(`≈ 50 ${m.uso_por_sessao()}`);
  expect(heading(m.uso_graf_ctx()).textContent).toContain('≈ 30 · 60%');
  for (const label of [m.uso_aba_skills(), m.uso_aba_agentes(), m.uso_aba_plugins(), m.uso_aba_tools(), m.uso_aba_bash(), m.uso_aba_mcp(), m.uso_aba_contexto(), m.uso_aba_imagens()]) expect(node.querySelector(`button[aria-label="${label}"]`)).toBeTruthy();
  const table = () => node.querySelector(`div[aria-label="${m.uso_aba_skills()}"]`)!;
  expect(table().querySelectorAll('button[aria-label^="plugin:item-"]')).toHaveLength(20);
  expect(table().textContent).toContain(`${m.uso_col_respostas()}: 124`);
  expect(table().textContent).toContain(`${m.uso_col_tamanho()}: ≈ 20`);
  expect(table().textContent).not.toContain(m.uso_sec_por_plugin());
  const sort = async (column: string) => { const button = table().querySelector<HTMLButtonElement>(`button[aria-label="${m.uso_ordenar_por({ col: column })}"]`)!; expect(button).toBeTruthy(); await act(async () => button.click()); };
  const first = () => table().querySelector('button[aria-label^="plugin:item-"]')!.getAttribute('aria-label');
  await sort(m.uso_col_cargas()); expect(first()).toMatch(/^plugin:item-00:/);
  await sort(m.uso_col_cargas()); expect(first()).toMatch(/^plugin:item-24:/);
  await sort(m.uso_col_respostas()); expect(first()).toMatch(/^plugin:item-24:/);
  const more = table().querySelector<HTMLButtonElement>(`button[aria-label="${m.uso_mostrar_mais({ n: 5 })}"]`)!;
  await act(async () => more.click()); expect(table().querySelectorAll('button[aria-label^="plugin:item-"]')).toHaveLength(25);
  const less = table().querySelector<HTMLButtonElement>(`button[aria-label="${m.uso_mostrar_menos()}"]`)!;
  await act(async () => less.click()); expect(table().querySelectorAll('button[aria-label^="plugin:item-"]')).toHaveLength(20);
  await click(m.uso_aba_agentes());
  expect(node.querySelector(`div[aria-label="${m.uso_aba_agentes()}"]`)?.textContent).toContain(`${m.uso_col_tok_chamada()}: 60`);
  await click(m.uso_avancado()); expect(node.querySelector('svg')).toBeNull(); expect(node.querySelector(`div[aria-label="${m.uso_aba_agentes()}"]`)).toBeNull();
});

it.each(['by_skill', 'by_agente'] as const)('detalhe de item agrupado usa o registro da chave de foco, não a soma, em %s', async (tab) => {
  useServers.setState({ servers: [a] });
  const chosen = { ...skill, key: 'plugin:heavy', input: 800, output: 0, cost: 8 };
  const alias = { ...skill, key: 'heavy', chamadas: 1, input: 200, output: 0, cost: 2, ocupados_eq_tokens_est: 200 };
  let finish!: (report: ReturnType<typeof usage>) => void;
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => filters.foco
    ? new Promise((resolve) => { finish = resolve; })
    : Promise.resolve({ ...usage(period, filters), [tab]: [chosen, alias] }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  const section = [...node.querySelectorAll('span')].find((span) => span.textContent === (tab === 'by_skill' ? m.uso_sec_por_plugin() : m.uso_sub_titulo()))!.parentElement!;
  const grouped = section.querySelector<HTMLButtonElement>('button[aria-label^="heavy:"]')!;
  expect(grouped.textContent).toContain(tab === 'by_skill' ? '≈ 1.000' : `${m.custos_input_sem_cache()}: 1.000`);
  await act(async () => grouped.click());
  const detail = () => node.querySelector('aside')!;
  expect(net.usage).toHaveBeenLastCalledWith(a, '30d', { foco: 'plugin:heavy' }, false);
  expect(detail().textContent).toContain(`${m.uso_col_chamadas()}: 4`);
  expect(detail().textContent).toContain(tab === 'by_skill' ? `${m.uso_col_ocupados_eq()}: ≈ 800` : `${m.custos_input_sem_cache()}: 800`);
  await act(async () => finish({ ...usage('30d', { foco: chosen.key }), [tab]: [chosen], by_day: [{ ...chosen, key: '2026-10-02' }] }));
  expect(detail().textContent).toContain(`${m.uso_col_chamadas()}: 4`);
  expect(detail().querySelector('[aria-label^="2026-10-02:"]')?.getAttribute('aria-label')).toContain('800');
});

it('atualizar detalhe renova resumo e série sem recarregar o relatório principal', async () => {
  useServers.setState({ servers: [a] });
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click('heavy');
  const detail = () => node.querySelector('aside')!;
  expect(detail().textContent).toContain(`${m.uso_col_chamadas()}: 4`);
  expect(detail().textContent).toContain(`${m.uso_col_ocupados_eq()}: ≈ 800`);
  const mainCalls = () => net.usage.mock.calls.filter((call) => !call[2].foco && !call[2].plugin?.length).length;
  const before = mainCalls();
  let finish!: (report: ReturnType<typeof usage>) => void;
  net.usage.mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const refresh = detail().querySelector<HTMLButtonElement>(`button[aria-label="${m.custos_atualizar()}"]`)!;
  await act(async () => refresh.click());
  expect(detail().textContent).toContain(`${m.uso_col_chamadas()}: 4`);
  expect(detail().textContent).toContain(m.custos_carregando_maquinas({ n: 1 }));
  const updated = { ...skill, chamadas: 5, ocupados_eq_tokens_est: 1_000 };
  await act(async () => finish({ ...usage('30d', { foco: skill.key }), by_skill: [updated], by_day: [{ ...updated, key: '2026-10-02' }, { ...updated, key: '2026-10-03', ocupados_eq_tokens_est: 200 }] }));
  expect(detail().textContent).toContain(`${m.uso_col_chamadas()}: 5`);
  expect(detail().textContent).toContain(`${m.uso_col_ocupados_eq()}: ≈ 1.000`);
  expect([...detail().querySelectorAll('rect')].map((rect) => rect.getAttribute('height'))).toEqual(['180', '36']);
  expect(net.usage).toHaveBeenLastCalledWith(a, '30d', { foco: skill.key }, true);
  expect(mainCalls()).toBe(before);
});

it.each(['comparação', 'diário', 'detalhe'] as const)('tocar ponto ou coluna mostra data e valores exatos sem leitor de tela em %s', async (chartType) => {
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, cost: 1.25 }, { ...combo, provider: 'provider-two', cost: 0 }] }));
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_day: [{ ...skill, key: '2026-10-01', chamadas: 3, ocupados_eq_tokens_est: 150 }, { ...skill, key: '2026-10-02', chamadas: 9, ocupados_eq_tokens_est: 800 }] }));
  await act(async () => root.render(React.createElement(chartType === 'comparação' ? CostsScreen : StatsScreen)));
  let label: string;
  let expected: string;
  if (chartType === 'comparação') {
    await click(m.custos_entidades_comparadas()); await click(`${m.custos_entidades_comparadas()}: provider-one`); await click(`${m.custos_entidades_comparadas()}: provider-two`); await click(m.native_close());
    const comparison = [...node.querySelectorAll('span')].find((span) => span.textContent === m.lista_comparar())!.parentElement!;
    await act(async () => comparison.querySelector<HTMLButtonElement>(`button[aria-label="${m.custos_estimativa_api()}"]`)!.click());
    label = m.custos_grafico_comp_aria({ metrica: m.custos_estimativa_api(), modo: m.custos_dia() });
    expected = '2026-10-01: —; provider-one: US$ 1,25 · provider-two: —';
  } else {
    if (chartType === 'diário') await click(m.uso_avancado()); else await click('heavy');
    label = chartType === 'diário' ? m.uso_graf_chamadas_dia() : m.uso_detalhe_por_dia();
    expected = `2026-10-01: ${chartType === 'diário' ? '3' : '150'}; ${chartType === 'diário' ? m.uso_col_chamadas() : m.uso_col_peso()}: ${chartType === 'diário' ? '3' : '150'}`;
  }
  const chart = node.querySelector(`[aria-label="${label}"]`)!;
  expect(chart.textContent).not.toContain(expected);
  const point = chart.querySelector<HTMLButtonElement>('button[aria-label^="2026-10-01:"]');
  expect(point).toBeTruthy();
  expect(point!.getAttribute('aria-label')).toBe(expected);
  await act(async () => point!.click());
  expect(chart.textContent).toContain(expected);
  if (chartType !== 'detalhe') expect(node.querySelector('aside')).toBeNull();
});

it('detalhes principais guardam identidade e valores do item com série em colunas sem refazer o relatório', async () => {
  useServers.setState({ servers: [a] });
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters),
    by_skill: [{ ...skill, pedidas: 1, ocupados_tokens_est: 400, respostas: 8 }],
    by_agente: [{ ...skill, key: 'agents:review', plugin: 'agents', pedidas: 2, input: 100, output: 20, cache_write: 30, cache_read: 40, cost: 4, cost_input: 1, cost_output: 2, cost_cache_write: 0.6, cost_cache_read: 0.4 }],
    by_day: [{ ...skill, key: '2026-10-02', ocupados_eq_tokens_est: 100, ctx_tokens_est: 8, input: 6, output: 2 }, { ...skill, key: '2026-10-03', ocupados_eq_tokens_est: 400, ctx_tokens_est: 16, input: 18, output: 2 }],
  }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click(m.uso_avancado()); await click(m.uso_aba_plugins());
  const mainCalls = () => net.usage.mock.calls.filter((call) => !call[2].foco && !call[2].plugin).length;
  const before = mainCalls();
  await click('heavy');
  expect(net.usage).toHaveBeenLastCalledWith(a, '30d', { foco: 'plugin:heavy' }, false);
  const detail = () => node.querySelector('aside')!;
  expect(detail().textContent).toContain(`${m.uso_col_origem_skill()}: 1 / 3`);
  expect(detail().textContent).toContain(`${m.uso_col_ocupados_eq()}: ≈ 800`);
  expect(detail().textContent).toContain(`${m.uso_col_ocupados()}: ≈ 400`);
  expect(detail().textContent).toContain(`${m.uso_col_respostas()}: 8`);
  expect(detail().textContent).toContain(`${m.uso_detalhe_ctx_chamada()}: ≈ 5`);
  expect(detail().textContent).toContain(`${m.uso_detalhe_media_sessao()}: ≈ 10`);
  expect([...detail().querySelectorAll('rect')].map((rect) => rect.getAttribute('height'))).toEqual(['45', '180']);
  await click(m.native_close());
  await click(m.uso_aba_imagens()); await click('Read');
  expect(net.usage).toHaveBeenLastCalledWith(a, '30d', { foco: 'Read' }, false);
  expect(detail().textContent).toContain(`${m.uso_col_ctx()}: ≈ 20`);
  expect(detail().textContent).not.toContain(m.uso_col_ocupados_eq());
  expect([...detail().querySelectorAll('rect')].map((rect) => rect.getAttribute('height'))).toEqual(['90', '180']);
  await click(m.native_close());
  await click(m.uso_aba_plugins()); await click('review');
  expect(net.usage).toHaveBeenLastCalledWith(a, '30d', { foco: 'agents:review' }, false);
  for (const value of [`${m.uso_col_origem_agente()}: 2 / 2`, '100 · US$ 1,00', '20 · US$ 2,00', '30 · US$ 0,60', '40 · US$ 0,40', `${m.uso_col_tok_sem_cache_chamada()}: 30`]) expect(detail().textContent).toContain(value);
  expect([...detail().querySelectorAll('rect')].map((rect) => rect.getAttribute('height'))).toEqual(['72', '180']);
  expect(mainCalls()).toBe(before);
  await click(m.native_close()); expect(node.querySelector('aside')).toBeNull();
});

it('estatísticas conservam filtros, detalhe usa foco sem refazer tabela e abas não ficam escondidas montadas', async () => {
  await act(async () => root.render(React.createElement(StatsScreen)));
  expect(node.textContent).toContain(m.uso_resp_skill_contexto());
  expect(node.textContent).toContain('≈ 1.600');
  await click(m.uso_conta());
  await click(`${m.uso_conta()}: account`);
  await click(m.native_close());
  expect(net.usage).toHaveBeenLastCalledWith(b, '30d', { conta: ['account'] }, false);
  const before = net.usage.mock.calls.filter((call) => !call[2].foco).length;
  await click('heavy');
  expect(net.usage).toHaveBeenLastCalledWith(b, '30d', { conta: ['account'], foco: 'plugin:heavy' }, false);
  expect(net.usage.mock.calls.filter((call) => !call[2].foco)).toHaveLength(before);
  expect(node.querySelector('aside')?.textContent).toContain('02/10');
  expect(node.querySelector('aside button[aria-label^="2026-10-02:"]')).toBeTruthy();
  await click(m.native_close());
  await click(m.uso_avancado()); await click(m.uso_aba_agentes());
  expect(node.querySelector(`div[aria-label="${m.uso_aba_agentes()}"]`)?.textContent).toContain('480');
  expect(node.textContent).toContain(m.uso_sec_por_plugin());
  await click(m.uso_ir_custos());
  expect(net.back).toHaveBeenCalledTimes(1);
});

it('imagens exibem rótulos humanos na lista e no detalhe mantendo o foco técnico na API', async () => {
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters),
    by_imagem: [{ ...skill, key: 'lida:Read' }, { ...skill, key: 'enviada' }, { ...skill, key: 'lida:mcp:captura' }],
    by_tool: [{ ...skill, key: 'Read', label: 'ferramenta preservada' }],
    by_plugin: [{ ...skill, key: '@repo', label: 'plugin preservado' }],
  }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click(m.uso_avancado()); await click(m.uso_aba_imagens());
  expect(node.textContent).toContain('lidas por Read');
  expect(node.textContent).toContain('enviadas por você');
  expect(node.textContent).toContain('lidas por mcp:captura');
  expect(node.innerHTML).not.toContain('lida:');
  const before = net.usage.mock.calls.filter((call) => !call[2].foco).length;
  await click('lidas por Read');
  expect(net.usage).toHaveBeenCalledWith(a, '30d', { foco: 'lida:Read' }, false);
  expect(net.usage).toHaveBeenLastCalledWith(b, '30d', { foco: 'lida:Read' }, false);
  expect(net.usage.mock.calls.filter((call) => !call[2].foco)).toHaveLength(before);
  expect(node.querySelector('aside')?.textContent).toContain('foco: lidas por Read');
  expect(node.querySelector('aside')?.textContent).not.toContain('lida:Read');
  await click(m.native_close());
  await click('enviadas por você');
  expect(net.usage).toHaveBeenLastCalledWith(b, '30d', { foco: 'enviada' }, false);
  expect(node.querySelector('aside')?.textContent).toContain('foco: enviadas por você');
  await click(m.native_close());
  for (const [tab, label] of [[m.uso_aba_tools(), 'ferramenta preservada'], [m.uso_aba_plugins(), 'plugin preservado'], [m.uso_aba_agentes(), 'reviewer'], [m.uso_aba_contexto(), 'instructions']] as const) {
    await click(tab);
    expect(node.textContent).toContain(label);
  }
});

it.each(['pt', 'en'] as const)('busca de imagens inclui rótulos exibidos e preserva chaves e metadados em %s', async (locale) => {
  configureLocale({ getLocale: () => locale });
  overwriteGetLocale(() => locale);
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters),
    by_imagem: [{ ...skill, key: 'lida:Read', label: 'read-metadata', plugin: 'read-plugin' }, { ...skill, key: 'enviada', label: 'sent-metadata', plugin: 'sent-plugin' }],
  }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click(m.uso_avancado()); await click(m.uso_aba_imagens());
  const readLabel = m.uso_img_lida({ tool: 'Read' });
  const sentLabel = m.uso_img_enviada();
  const search = node.querySelector<HTMLInputElement>(`input[aria-label="${m.uso_busca()}"]`);
  expect(search).toBeTruthy();
  for (const [query, expected, hidden] of [
    [readLabel, readLabel, sentLabel], [sentLabel, sentLabel, readLabel],
    ['lida:Read', readLabel, sentLabel], ['enviada', sentLabel, readLabel],
    ['read-metadata', readLabel, sentLabel], ['sent-metadata', sentLabel, readLabel],
    ['read-plugin', readLabel, sentLabel], ['sent-plugin', sentLabel, readLabel],
  ]) {
    await act(async () => { search!.value = query; search!.dispatchEvent(new Event('input', { bubbles: true })); });
    expect(node.querySelectorAll(`button[aria-label^="${expected}: "]`)).toHaveLength(1);
    expect(node.querySelector(`button[aria-label^="${hidden}: "]`)).toBeNull();
  }
});

it('cartões dividem custo por tokens processados e cache de uma hora pela escrita, como no desktop', async () => {
  net.costs.mockImplementation((_server: Server, period: string) => {
    const priced = { ...combo, input: 100_000, output: 100_000, cache_write: 100_000, cache_read: 700_000, cache_write_1h: 50_000, regravado: 10_000, custo_regravado: 0.25, cost: 2, cost_input: 0.4, cost_output: 1, cost_cache_write: 0.5, cost_cache_read: 0.1 };
    return Promise.resolve({ ...costs(period), totals: { ...priced, key: 'totals' }, combos: [priced] });
  });
  await act(async () => root.render(React.createElement(CostsScreen)));
  const card = (label: string) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)?.parentElement;
  expect(card(m.custos_custo_por_milhao())?.textContent?.replace(/\s/g, '')).toContain('US$2,00');
  expect(card(m.custos_cache_1h())?.textContent).toContain('50%');
});

it.each([
  ['pt', 0, 'US$ 0,00', 'R$ 0,00'],
  ['pt', 91.9602, 'US$ 91,96', 'R$ 459,80'],
  ['pt', 1_234_567.8912, 'US$ 1.234.567,89', 'R$ 6.172.839,46'],
  ['en', 0, 'US$ 0.00', 'R$0.00'],
  ['en', 91.9602, 'US$ 91.96', 'R$459.80'],
  ['en', 1_234_567.8912, 'US$ 1,234,567.89', 'R$6,172,839.46'],
] as const)('moedas dos relatórios mantêm duas casas em %s para %s', async (locale, value, usd, brl) => {
  configureLocale({ getLocale: () => locale });
  overwriteGetLocale(() => locale);
  useServers.setState({ servers: [a] });
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), usd_brl: 5, combos: [{ ...combo, input: value === 0 ? 0 : 100, output: 0, cache_write: 0, cache_read: 0, cost: value, cost_input: value }] }));
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_agente: [{ ...skill, key: 'reviewer', cost: value }], by_day: [{ ...skill, key: '2026-10-02', cost: value }] }));
  const amount = (label: string, index = 1) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)?.parentElement?.querySelectorAll('span')[index]?.textContent?.replace(/\s/g, ' ');
  await act(async () => root.render(React.createElement(CostsScreen)));
  expect.soft(amount(m.custos_custo_periodo())).toBe(usd);
  await click('BRL');
  expect.soft(amount(m.custos_custo_periodo())).toBe(brl);
  await act(async () => root.render(React.createElement(StatsScreen)));
  const agentCard = [...node.querySelectorAll('span')].find((span) => span.textContent === m.uso_resp_agente_caro())!.parentElement!;
  if (value > 0) expect.soft(agentCard.textContent?.replace(/\s/g, ' ')).toContain(usd);
  else expect.soft(agentCard.textContent).toBe(`${m.uso_resp_agente_caro()}—`);
  await click('reviewer');
  expect(node.querySelector('aside')?.textContent?.replace(/\s/g, ' ')).toContain(`${m.uso_col_custo_real()}: ${usd}`);
});

it('cache perdido medido sem tarifa mostra traço em vez de custo zero', async () => {
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), cache_detalhado: true, combos: [{ ...combo, regravado: 50, custo_regravado: 0 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const card = [...node.querySelectorAll('span')].find((span) => span.textContent === m.custos_cache_perdido())?.parentElement;
  expect(card?.textContent).toContain('—');
  expect(card?.textContent).not.toContain('US$');
});

it('resumo distingue presença de cargas sem cartão extra e agente mostra tokens reais por chamada', async () => {
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_skill: [{ ...skill, key: 'frequent:loaded', plugin: 'frequent', chamadas: 30, ocupados_eq_tokens_est: 10 }, { ...skill, key: 'heavy-plugin:heavy', plugin: 'heavy-plugin', chamadas: 1, ocupados_eq_tokens_est: 900 }] }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  const card = (label: string) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)!.parentElement!;
  expect(card(m.uso_resp_plugin_skills()).textContent).toContain('heavy-plugin');
  expect(card(m.uso_resp_skill_usada()).textContent).toContain('loaded');
  expect(card(m.uso_resp_skill_usada()).textContent).toContain(m.uso_resp_grupo_vezes({ grupo: 'frequent', n: '60' }));
  expect(node.textContent).not.toContain(m.mobile_usage_most_loaded_plugin());
  await click(m.uso_avancado()); await click(m.uso_aba_agentes());
  expect(node.textContent).toContain(`${m.uso_col_tok_chamada()}: 60`);
});

it('dia sem movimento entre dois dias abre detalhe com zero explícito', async () => {
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [combo, { ...combo, dia: '2026-10-03' }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click('2026-10-02');
  const detail = node.querySelector('aside');
  expect(detail?.textContent?.replace(/\s/g, '')).toContain('0·US$0,00');
  expect(detail?.textContent).toContain(m.custos_tipo_cache_lido());
});

it('avançado mostra chamadas diárias e contexto sem enviar data como foco de item', async () => {
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click(m.uso_avancado());
  expect(node.textContent).toContain('01/10');
  expect(node.querySelector('button[aria-label^="2026-10-01:"]')).toBeTruthy();
  expect(node.textContent).toContain(m.uso_ctx_cat_instrucoes());
  expect(node.querySelector('button[aria-label="2026-10-01"]')).toBeNull();
  expect(net.usage.mock.calls.every((call) => !call[2].foco)).toBe(true);
});

it.each([
  ['pt', 'Sem resposta do servidor.'],
  ['en', 'The server did not respond.'],
] as const)('custos distinguem carregamento, sucesso incremental, parcial, falha total e período vazio em %s', async (locale, networkError) => {
  configureLocale({ getLocale: () => locale });
  overwriteGetLocale(() => locale);
  const notebook = { ...b, label: 'notebook-lhais' };
  useServers.setState({ servers: [a, notebook] });
  const canceled = 'fetch failed: Fetch request has been canceled';
  let finishA!: (report: Partial<CostReport>) => void;
  let failB!: (error: Error) => void;
  net.costs.mockImplementation((server: Server) => server.id === 'a' ? new Promise((resolve) => { finishA = resolve; }) : new Promise((_resolve, reject) => { failB = reject; }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  expect(node.textContent).toContain(m.custos_carregando_maquinas({ n: 2 }));
  expect(node.textContent).not.toContain(m.custos_custo_periodo());
  await act(async () => finishA(costs('30d')));
  expect(node.textContent).toContain(m.custos_custo_periodo());
  expect(node.textContent).toContain(m.custos_carregando_maquinas({ n: 1 }));
  await act(async () => failB(new Error(canceled)));
  expect(node.textContent).toContain(`notebook-lhais: ${networkError}`);
  expect(node.textContent).not.toMatch(/fetch failed|Fetch request has been canceled/);
  expect(node.textContent).toContain(m.custos_total_parcial());
  expect(node.textContent).toContain(m.custos_custo_periodo());
  net.costs.mockRejectedValue(new Error(canceled));
  await click(m.custos_atualizar());
  expect(net.costs).toHaveBeenLastCalledWith(notebook, '30d', true, false);
  expect(node.textContent).toContain(`Machine A: ${networkError}`);
  expect(node.textContent).toContain(`notebook-lhais: ${networkError}`);
  expect(node.textContent).not.toMatch(/fetch failed|Fetch request has been canceled/);
  expect(node.textContent).toContain(m.home_usage_load_failed());
  expect(node.textContent).not.toContain(m.custos_total_parcial());
  expect(node.textContent).not.toContain(m.custos_custo_periodo());
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve(mergeReports([{ id: a.id, report: { applied: { period } } }], period).report));
  await click(m.custos_atualizar());
  expect(node.textContent).toContain(m.custos_sem_dados_periodo());
  expect(node.textContent).toContain(m.custos_custo_periodo());
  expect(node.textContent).not.toContain(networkError);
  expect(node.textContent).not.toContain(m.home_usage_load_failed());
});

it.each(['stats', 'areas'] as const)('rede de uso preserva sucesso parcial e identifica a máquina em %s', async (screen) => {
  net.usage.mockImplementation((server: Server, period: string, filters: UsoFiltros) => server.id === 'b'
    ? Promise.reject(new Error('fetch failed: Fetch request has been canceled'))
    : Promise.resolve(usage(period, filters)));
  await act(async () => root.render(React.createElement(screen === 'stats' ? StatsScreen : CostsScreen)));
  expect(node.textContent).toContain(`Machine B: ${m.native_network_error()}`);
  expect(node.textContent).not.toMatch(/fetch failed|Fetch request has been canceled/);
  expect(node.textContent).toContain(m.custos_total_parcial());
  expect(node.textContent).toContain(screen === 'stats' ? 'heavy' : 'front');
});

it('detalhe de uso separa vazio, parcial e erro e opções sobrevivem ao recorte vazio', async () => {
  net.usage.mockImplementation((server: Server, period: string, filters: UsoFiltros) => filters.foco
    ? server.id === 'b' ? Promise.reject(new Error('fetch failed: Fetch request has been canceled')) : Promise.resolve({ ...usage(period, filters), by_day: [] })
    : Promise.resolve(usage(period, filters)));
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click('heavy');
  expect(node.querySelector('aside')?.textContent).toContain(m.uso_detalhe_sem_serie());
  expect(node.querySelector('aside')?.textContent).toContain(m.custos_total_parcial());
  expect(node.querySelector('aside')?.textContent).toContain(`Machine B: ${m.native_network_error()}`);
  expect(node.querySelector('aside')?.textContent).not.toMatch(/fetch failed|Fetch request has been canceled/);
  net.usage.mockRejectedValue(new Error('fetch failed: Fetch request has been canceled'));
  const refresh = node.querySelector('aside button[aria-label="' + m.custos_atualizar() + '"]') as HTMLButtonElement;
  await act(async () => refresh.click());
  expect(node.querySelector('aside')?.textContent).toContain(`Machine A: ${m.native_network_error()}`);
  expect(node.querySelector('aside')?.textContent).toContain(`Machine B: ${m.native_network_error()}`);
  expect(node.querySelector('aside')?.textContent).not.toMatch(/fetch failed|Fetch request has been canceled/);
  expect(node.querySelector('aside')?.textContent).toContain(m.home_usage_load_failed());
  expect(node.querySelector('aside')?.textContent).not.toContain(m.uso_detalhe_por_dia());
  await click(m.native_close());
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...filters, applied: { period }, totals: zeroUso('totals') }));
  await click(m.uso_conta());
  await click(`${m.uso_conta()}: account`);
  await click(m.native_close());
  await click(m.uso_conta());
  expect(node.querySelector('aside')?.textContent).toContain('account');
});

it.each([false, true])('comparação usa os trinta dias do período nas médias de tokens e dinheiro, legado=%s', async (legacy) => {
  useServers.setState({ servers: [a] });
  const rows = ['provider-one', 'provider-two'].map((provider) => ({ ...combo, provider, input: 300, output: 0, cache_write: 0, cache_read: 0, cost: 30 }));
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: legacy ? [] : rows, by_provider: rows.map((row) => ({ ...row, key: row.provider })) }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_entidades_comparadas());
  await click(`${m.custos_entidades_comparadas()}: provider-one`);
  await click(`${m.custos_entidades_comparadas()}: provider-two`);
  await click(m.native_close());
  const comparison = () => [...node.querySelectorAll('span')].find((span) => span.textContent === m.lista_comparar())!.parentElement!;
  const entity = () => [...comparison().querySelectorAll('span')].find((span) => span.textContent === 'provider-one' && span.style.fontWeight === '600')!.parentElement!;
  expect(entity().textContent).toContain(`10 ${m.custos_por_dia()}`);
  const money = comparison().querySelector<HTMLButtonElement>(`button[aria-label="${m.custos_estimativa_api()}"]`)!;
  await act(async () => money.click());
  expect(entity().textContent).toContain(`US$ 1,00 ${m.custos_por_dia()}`);
  if (legacy) expect(comparison().textContent).toContain(m.custos_sem_detalhamento());
});

it('comparação pública ignora provedor selecionado e conserva o projeto; áreas atualizam frescas', async () => {
  net.costs.mockImplementation((_server: Server, period: string) => Promise.resolve({ ...costs(period), combos: [{ ...combo, cost: 3 }, { ...combo, provider: 'provider-two', cost: 7 }, { ...combo, provider: 'provider-two', project: '/other', cost: 99 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_dim_provedor()); await click(`${m.custos_dim_provedor()}: provider-one`); await click(m.native_close());
  await click(m.custos_dim_projeto()); await click(`${m.custos_dim_projeto()}: /project-one`); await click(m.native_close());
  await click(m.custos_entidades_comparadas());
  await click(`${m.custos_entidades_comparadas()}: provider-one`);
  await click(`${m.custos_entidades_comparadas()}: provider-two`);
  await click(m.native_close());
  const entity = [...node.querySelectorAll('span')].find((span) => span.textContent === 'provider-two' && span.style.fontWeight === '600')?.parentElement;
  expect(entity?.textContent?.replace(/\s/g, '')).toContain('US$14,00');
  await click(m.custos_atualizar());
  expect(net.usage).toHaveBeenLastCalledWith(b, '30d', { projeto: ['/project-one'], modelo: [] }, true);
});

it('atualização preserva filtros cruzados enquanto outra máquina ainda entrega os detalhes', async () => {
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_dim_provedor()); await click(`${m.custos_dim_provedor()}: provider-one`); await click(m.native_close());
  await click(m.custos_dim_projeto()); await click(`${m.custos_dim_projeto()}: /project-one`); await click(m.native_close());
  let finish!: (report: Partial<CostReport>) => void;
  net.costs.mockImplementation((server: Server, period: string) => server.id === 'a' ? Promise.resolve({ applied: { period }, totals: { ...combo, key: 'totals', sessions: 0, input: 0, output: 0, cache_write: 0, cache_read: 0 }, combos: [] }) : new Promise((resolve) => { finish = resolve; }));
  await click(m.custos_atualizar());
  expect(node.textContent).toContain('/project-one');
  await act(async () => finish(costs('30d')));
  expect(node.textContent).toContain('/project-one');
});

it('detalhe diário de plugin usa chamadas quando a API não fornece presença e skill continua usando presença', async () => {
  useServers.setState({ servers: [a] });
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters),
    by_plugin: [{ ...skill, key: 'usage-plugin', ocupados_eq_tokens_est: 900 }],
    by_day: [{ ...skill, key: '2026-10-01', chamadas: 2, ocupados_eq_tokens_est: filters.foco ? 200 : 0 }, { ...skill, key: '2026-10-02', chamadas: 8, ocupados_eq_tokens_est: filters.foco ? 400 : 0 }],
  }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click(m.uso_avancado()); await click(m.uso_aba_plugins()); await click('usage-plugin');
  const detail = () => node.querySelector('aside')!;
  expect([...detail().querySelectorAll('rect')].map((rect) => rect.getAttribute('height'))).toEqual(['45', '180']);
  expect(detail().textContent).toContain(`${m.uso_detalhe_por_dia()} (${m.uso_col_chamadas()})`);
  expect(detail().querySelector('[aria-label^="2026-10-01:"]')?.getAttribute('aria-label')).toContain(`${m.uso_col_chamadas()}: 2`);
  expect(detail().textContent).toContain(`${m.uso_col_ocupados_eq()}: ≈ 900`);
  await click(m.native_close()); await click('heavy');
  expect(detail().textContent).toContain(`${m.uso_detalhe_por_dia()} (${m.uso_col_peso()})`);
  expect([...detail().querySelectorAll('rect')].map((rect) => rect.getAttribute('height'))).toEqual(['90', '180']);
});

it('plugin abre série pelo filtro de plugin sem transformar nome do grupo em foco de item', async () => {
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_plugin: [{ ...skill, key: 'usage-plugin', plugin: 'usage-plugin' }] }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click(m.uso_avancado()); await click(m.uso_aba_plugins());
  await click('usage-plugin');
  expect(net.usage).toHaveBeenLastCalledWith(b, '30d', { plugin: ['usage-plugin'] }, false);
  expect(node.querySelector('aside')?.textContent).toContain('01/10');
  expect(node.querySelector('aside button[aria-label^="2026-10-01:"]')).toBeTruthy();
});

it.each(['skill', 'plugin', 'tool', 'context', 'image', 'agent'] as const)('detalhe mostra custo somente para agente inclusive zero: %s', async (kind) => {
  useServers.setState({ servers: [a] });
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_agente: [{ ...skill, key: 'reviewer', cost: 0 }], by_imagem: [{ ...skill, key: 'enviada' }] }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  if (kind === 'plugin' || kind === 'context' || kind === 'image') {
    await click(m.uso_avancado());
    await click(kind === 'plugin' ? m.uso_aba_plugins() : kind === 'context' ? m.uso_aba_contexto() : m.uso_aba_imagens());
  }
  const label = { skill: 'heavy', plugin: 'plugin', tool: 'Read', context: 'instructions', image: m.uso_img_enviada(), agent: 'reviewer' }[kind];
  if (kind === 'plugin') {
    const list = node.querySelector(`div[aria-label="${m.uso_aba_plugins()}"]`)!;
    await act(async () => list.querySelector<HTMLButtonElement>('button[aria-label^="plugin:"]')!.click());
  } else await click(label);
  const detail = node.querySelector('aside')!;
  expect(detail.textContent).toContain(`${m.uso_foco()}: ${label}`);
  expect(detail.textContent).toContain(`${m.uso_col_chamadas()}: 4`);
  if (kind === 'agent') expect(detail.textContent).toContain(`${m.uso_col_custo_real()}: US$ 0,00`);
  else {
    expect(detail.textContent).not.toContain(m.uso_col_custo_real());
    expect(detail.textContent).not.toContain('US$');
  }
});

it('detalhe de plugin conserva chamadas e contexto sem mostrar custo ou inventar tokens reais', async () => {
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_plugin: [{ ...skill, key: 'detail-plugin' }], by_day: [{ ...skill, key: '2026-10-01', input: 0, output: 0 }] }));
  await act(async () => root.render(React.createElement(StatsScreen)));
  await click(m.uso_avancado()); await click(m.uso_aba_plugins()); await click('detail-plugin');
  const detail = node.querySelector('aside');
  expect(detail?.textContent).not.toContain(m.uso_detalhe_tokens());
  expect(detail?.textContent).toContain(`${m.uso_col_chamadas()}: 8`);
  expect(detail?.textContent).toContain(`${m.uso_col_ctx()}: ≈ 40`);
  expect(detail?.textContent).not.toContain(m.uso_col_custo_real());
  expect(detail?.textContent).not.toContain('US$');
});

it('áreas mostram composição e ranking do período sem montar lista diária ou mais 171', async () => {
  useServers.setState({ servers: [a] });
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_area: [
    { ...zeroUso('back'), input: 153_583_509, cost: 91.96 },
    { ...zeroUso('front'), output: 43, cache_read: 7, cost: 2.5 },
  ], by_area_dia: Array.from({ length: 191 }, (_, index) => ({ ...zeroUso(`2026-09-17|daily-area-${index}`), input: 10, cost: 1 })) }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const areas = [...node.querySelectorAll('span')].find((span) => span.textContent === m.uso_graf_areas())!.parentElement!;
  expect(areas.textContent).not.toContain('2026-09-17');
  expect(areas.textContent).not.toContain('daily-area-');
  expect(areas.querySelector(`button[aria-label="${m.uso_mostrar_mais({ n: 171 })}"]`)).toBeNull();
  expect(areas.querySelector(`[role="image"][aria-label^="${m.uso_graf_areas()}: front: 50"]`)).toBeTruthy();
  expect(areas.querySelector('[aria-label^="back: 153.583.509 · US$ 91,96"]')).toBeTruthy();
  expect(areas.querySelector('[aria-label^="front: 50 · US$ 2,50"]')).toBeTruthy();
});

it('áreas sem tarifa mantêm traço e barras interativas anunciam valores além do nome', async () => {
  net.usage.mockImplementation((_server: Server, period: string, filters: UsoFiltros) => Promise.resolve({ ...usage(period, filters), by_area: [{ ...skill, key: 'front', cost: 0 }], by_area_dia: [{ ...skill, key: '2026-10-01|front', cost: 0 }] }));
  await act(async () => root.render(React.createElement(CostsScreen)));
  const bar = [...node.querySelectorAll('button')].find((button) => button.getAttribute('aria-label')?.startsWith('provider-one'));
  expect(bar?.getAttribute('aria-label')).toContain('360');
  const areas = [...node.querySelectorAll('span')].find((span) => span.textContent === m.uso_graf_areas())!.parentElement!;
  const area = areas.querySelector('[aria-label^="front:"]');
  expect(area?.getAttribute('aria-label')).toContain('—');
  expect(area?.getAttribute('aria-label')).toContain('24 · —');
  expect(areas.textContent).not.toContain('2026-10-01');
});

it('áreas respeitam a máquina do filtro sem consultar ou somar a outra máquina', async () => {
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_dim_maquina()); await click(`${m.custos_dim_maquina()}: Machine A`); await click(m.native_close());
  let finish!: (report: ReturnType<typeof usage>) => void;
  net.usage.mockClear();
  net.usage.mockImplementation((server: Server, period: string, filters: UsoFiltros) => server.id === 'a' ? new Promise((resolve) => { finish = resolve; }) : Promise.resolve({ ...usage(period, filters), by_area: [{ ...skill, key: 'area-from-b' }] }));
  await click(m.custos_atualizar());
  expect(net.usage.mock.calls.map((call) => call[0].id)).toEqual(['a']);
  expect(node.textContent).toContain(m.custos_carregando_maquinas({ n: 1 }));
  await act(async () => finish({ ...usage('30d', {}), by_area: [{ ...skill, key: 'area-from-a' }] }));
  expect(node.textContent).toContain('area-from-a');
  expect(node.textContent).not.toContain('area-from-b');
});

it.each([[false, true, true], [true, true, true], [false, true, false], [false, false, true]] as const)('presença ausente preserva dados e usa chamadas na malha; somente antiga: %s, skill: %s, plugin: %s', async (onlyOld, skillMissing, pluginMissing) => {
  useServers.setState({ servers: onlyOld ? [b] : [a, b] });
  net.usage.mockImplementation((server: Server, period: string, filters: UsoFiltros) => {
    const report = usage(period, filters);
    const legacy = { ...skill, key: 'legacy-plugin:legacy', plugin: 'legacy-plugin', chamadas: 12, ctx_tokens_est: 60, ocupados_eq_tokens_est: undefined };
    return Promise.resolve(server.id === 'b' ? { ...report, by_skill: [{ ...legacy, ocupados_eq_tokens_est: skillMissing ? undefined : 800 }], by_plugin: [{ ...legacy, key: 'legacy-plugin', ocupados_eq_tokens_est: pluginMissing ? undefined : 800 }] } : report);
  });
  await act(async () => root.render(React.createElement(StatsScreen)));
  expect(node.textContent).toContain(m.mobile_usage_presence_unavailable({ machines: 'Machine B' }));
  const card = (label: string) => [...node.querySelectorAll('span')].find((span) => span.textContent === label)?.parentElement;
  expect(card(m.uso_resp_skill_contexto())?.textContent).toBe(`${m.uso_resp_skill_contexto()}—`);
  expect(card(m.uso_resp_plugin_skills())?.textContent).toBe(`${m.uso_resp_plugin_skills()}—`);
  expect(card(m.uso_resp_skill_usada())?.textContent).toContain('legacy');
  expect(card(m.uso_resp_skill_usada())?.textContent).toContain(m.uso_resp_grupo_vezes({ grupo: 'legacy-plugin', n: '12' }));
  const weightButton = node.querySelector<HTMLButtonElement>(`button[aria-label="${m.uso_ordenar_por({ col: m.uso_col_peso() })}"]`);
  expect(weightButton?.disabled).toBe(true);
  const skillRows = [...node.querySelectorAll('button')].filter((button) => ['legacy:', 'heavy:'].some((prefix) => button.getAttribute('aria-label')?.startsWith(prefix)));
  expect(skillRows[0]?.getAttribute('aria-label')).toBe('legacy: — · 12');
  await click(m.uso_avancado()); await click(m.uso_aba_plugins());
  const pluginRows = [...node.querySelectorAll('button')].filter((button) => ['legacy-plugin:', 'plugin:'].some((prefix) => button.getAttribute('aria-label')?.startsWith(prefix)));
  expect(pluginRows[0]?.getAttribute('aria-label')).toBe('legacy-plugin: —');
  expect(pluginRows[0]?.parentElement?.textContent).toContain(`${m.uso_col_cargas()}: 12`);
  expect(pluginRows[0]?.parentElement?.textContent).toContain('≈ 5');
  await click(m.uso_aba_agentes());
  expect(node.textContent).toContain(`${m.uso_col_tok_chamada()}: 60`);
  expect(node.textContent?.replace(/\s/g, '')).toContain(onlyOld ? 'US$2,00' : 'US$4,00');
});

it('escolha de máquinas e período é compartilhada, sem consultas em tela sem foco', async () => {
  await act(async () => root.render(React.createElement(CostsScreen)));
  await click(m.custos_servidores_relatorio());
  await click(`${m.custos_servidores_relatorio()}: Machine B`);
  await click(m.native_close());
  await click(m.custos_periodo_7d());
  await act(async () => root.render(React.createElement(StatsScreen)));
  expect(net.usage).toHaveBeenLastCalledWith(a, '7d', {}, false);
  net.usage.mockClear(); net.focused = false;
  await act(async () => root.render(React.createElement(StatsScreen)));
  expect(net.usage).not.toHaveBeenCalled();
});
