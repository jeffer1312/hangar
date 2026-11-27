import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import * as m from './paraglide/messages';
import { placementLimits, placementProjects, placementReasonText, placementScheduleRule, placementStatus, placementSuggested,
  placementSuggestionFor, readPlacementAll, samePlacementConfig, type Placement, type PlacementConfig } from './placement';

const config: PlacementConfig = {
  ui_mode: 'suggest', agent_mode: 'auto', limits: { mem_free_pct: 15, working: 6, cpu_pct: 80 },
  project_rules: [{ project: 'git:github.com/a/b', server: 'nb' }],
  schedule_rules: [{ days: [1, 5], from: '09:00', to: '18:00', tz: 'America/Sao_Paulo', avoid: 'nb' }],
  clone_dirs: { nb: '/h/Projetos', pc: '/h/src' },
};
const least: Placement = { server: 'nb', reason: { code: 'least_loaded', local_out: 'working' }, clone: null, warnings: [] };

beforeEach(() => {
  _resetApiEnvForTests();
  configureApi({ getBaseUrl: () => '', getToken: () => null, onUnauthorized() {}, origin: null, createEventSource: vi.fn() });
});
afterEach(() => { vi.restoreAllMocks(); _resetApiEnvForTests(); });

it('só marca a máquina com a tela em sugerir e o pedido dentro do balanceamento', () => {
  expect(placementSuggested({ ui_mode: 'suggest' }, least)).toBe(least);
  expect(placementSuggested({ ui_mode: 'off' }, least)).toBeNull();
  expect(placementSuggested({ ui_mode: 'ask' }, least)).toBeNull();
  expect(placementSuggested({ ui_mode: 'suggest' }, { ...least, reason: { code: 'not_balanced', why: 'mode_off' } })).toBeNull();
  expect(placementSuggested({ ui_mode: 'suggest' }, { ...least, server: '' })).toBeNull();
  expect(placementSuggested({ ui_mode: 'suggest' }, { server: 'pc' })).toBeNull();
});

it('monta o motivo dos códigos, com o nome da máquina, avisos e o clone', () => {
  const label = (id: string) => ({ nb: 'Notebook', vm: 'VM' })[id] ?? id;
  const full: Placement = { ...least, clone: { dir: '/h/Projetos' }, warnings: [{ code: 'rule_server_out', server: 'vm', why: 'missing' }, { code: 'all_over_limit' }] };
  expect(placementReasonText(full, label)).toBe([
    m.native_placement_reason_least_loaded({ motivo: m.native_placement_exclusion_working(), maquina: 'Notebook' }),
    m.native_placement_warning_rule_server_out({ maquina: 'VM', motivo: m.native_placement_exclusion_missing() }),
    m.native_placement_warning_all_over_limit(),
    m.native_placement_clone_note({ maquina: 'Notebook', dir: '/h/Projetos' }),
  ].join('; '));
  expect(placementReasonText({ ...least, reason: { code: 'project_rule', project: 'git:github.com/a/b' } }))
    .toBe(m.native_placement_reason_project_rule({ projeto: 'github.com/a/b', maquina: 'nb' }));
  expect(placementReasonText({ ...least, reason: { code: 'unpublished', why: 'no_upstream' } })).toBe(m.native_placement_reason_unpublished_no_upstream());
  // Código que o app não conhece aparece cru, não some.
  expect(placementReasonText({ ...least, reason: { code: 'not_balanced', why: 'novo' } })).toBe(m.native_placement_reason_not_balanced({ motivo: 'novo' }));
});

const pc = { id: 'pc-app', label: 'PC', baseUrl: 'https://pc.test', token: 't1' };
const nb = { id: 'nb-app', label: 'Notebook', baseUrl: 'https://nb.test', token: 't2' };
function routes(table: Record<string, unknown>) {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (input) => {
    const url = String(input);
    const hit = Object.entries(table).find(([path]) => url === path);
    return hit ? new Response(JSON.stringify(hit[1])) : new Response('{"detail":"Not Found"}', { status: 404 });
  });
}

it('a sugerida é achada pelo identificador de cada máquina e o motivo leva o nome dela', async () => {
  routes({
    'https://pc.test/api/placement/config': config,
    'https://pc.test/api/placement/suggest': least,
    'https://pc.test/api/peers/identificador': { identificador: 'pc' },
    'https://nb.test/api/peers/identificador': { identificador: 'nb' },
  });
  expect(await placementSuggestionFor(pc, [pc, nb])).toEqual({
    serverId: 'nb-app', text: m.native_placement_reason_least_loaded({ motivo: m.native_placement_exclusion_working(), maquina: 'Notebook' }),
  });
});

it('sugerida fora da lista do aparelho não marca nada; servidor sem a rota rejeita', async () => {
  routes({
    'https://pc.test/api/placement/config': config,
    'https://pc.test/api/placement/suggest': { ...least, server: 'vm' },
    'https://pc.test/api/peers/identificador': { identificador: 'pc' },
  });
  expect(await placementSuggestionFor(pc, [pc, nb])).toBeNull();
  routes({});
  await expect(placementSuggestionFor(pc, [pc])).rejects.toMatchObject({ status: 404 });
});

it('lê cada máquina: versão sem a rota é antiga, e as regras comparam sem a ordem das pastas', async () => {
  routes({
    'https://pc.test/api/placement/config': config,
    'https://pc.test/api/placement/facts': { server_id: 'pc', version: '1', repos: [{ project: 'git:x/y', root: '/r' }, { project: 'git:a/b', root: '/s' }], clone_dir: null },
    'https://pc.test/api/peers/identificador': { identificador: 'pc' },
  });
  const [here, old] = await readPlacementAll([pc, nb]);
  expect(here).toMatchObject({ serverId: 'pc-app', ident: 'pc', read: { state: 'ready', config } });
  expect(old).toMatchObject({ serverId: 'nb-app', ident: null, read: { state: 'old' }, facts: null });
  expect(placementProjects([here, old])).toEqual(['git:a/b', 'git:x/y']);
  const reordered = { ...config, clone_dirs: { pc: '/h/src', nb: '/h/Projetos' } };
  expect(samePlacementConfig(config, reordered)).toBe(true);
  expect(placementStatus(here.read, reordered)).toBe('same');
  expect(placementStatus(here.read, { ...config, ui_mode: 'off' })).toBe('different');
  expect(placementStatus(old.read, config)).toBe('old');
  expect(placementStatus({ state: 'failed', error: 'x' }, config)).toBe('silent');
});

it('limites e regra de horário seguem o que o servidor aceita', () => {
  expect(placementLimits('15', ' 6 ', '80')).toEqual({ ok: true, value: { mem_free_pct: 15, working: 6, cpu_pct: 80 } });
  expect(placementLimits('101', '6', '80')).toEqual({ ok: false, error: m.native_placement_limit_pct_invalid() });
  expect(placementLimits('15', '6', 'abc')).toEqual({ ok: false, error: m.native_placement_limit_pct_invalid() });
  expect(placementLimits('15', '0', '80')).toEqual({ ok: false, error: m.native_placement_limit_working_invalid() });
  expect(placementScheduleRule([5, 1], '9:00', '18:00', 'America/Sao_Paulo', 'nb'))
    .toEqual({ ok: true, value: { days: [1, 5], from: '09:00', to: '18:00', tz: 'America/Sao_Paulo', avoid: 'nb' } });
  expect(placementScheduleRule([], '9h', '18:00', 'UTC', 'nb')).toEqual({ ok: false, error: m.native_placement_schedule_time_invalid() });
  expect(placementScheduleRule([], '09:00', '18:00', 'Lua/Base', 'nb')).toEqual({ ok: false, error: m.native_placement_schedule_tz_invalid() });
  expect(placementScheduleRule([], '09:00', '18:00', 'UTC', null)).toEqual({ ok: false, error: m.native_placement_schedule_machine_missing() });
});
