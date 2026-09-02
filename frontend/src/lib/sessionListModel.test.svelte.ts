// @vitest-environment happy-dom
// (o vitest roda em `node` por padrão — sem isto não há localStorage; precedente: serverConfig.test.ts)
// frontend/src/lib/sessionListModel.test.svelte.ts
import { describe, it, expect, vi, beforeEach } from 'vitest';

// Store plano (sem reatividade), como o Sidebar.test.ts faz: preencher ANTES de criar o modelo.
const store = vi.hoisted(() => ({
  rows: [] as any[], byServer: [] as any[], servers: [] as any[], loading: false,
  retain: vi.fn(), release: vi.fn(), markDeleting: vi.fn(), unmarkDeleting: vi.fn(),
}));
vi.mock('./sessionsStore.svelte', () => ({ sessionsStore: store }));
vi.mock('./api', () => ({
  deleteSession: vi.fn(), renameSession: vi.fn(), resumeSession: vi.fn(), broadcast: vi.fn(),
}));
vi.mock('./auth', () => ({
  getActiveId: vi.fn(() => 'srv-a'), selectServer: vi.fn(), serverColor: (id: string) => `c-${id}`,
}));

import { createSessionListModel } from './sessionListModel.svelte';

const sess = (name: string, serverId: string, over: Record<string, unknown> = {}) => ({
  name, serverId, serverLabel: serverId.toUpperCase(), serverColor: `c-${serverId}`,
  state: 'idle', cwd: `/repo/${name}`, tracked: true, ...over,
});
function comServidores(buckets: { id: string; label: string; sessions: any[]; error?: string | null; loaded?: boolean }[]) {
  store.servers = buckets.map((b) => ({ id: b.id, label: b.label, baseUrl: 'http://x', token: 't' }));
  // Mesma forma do aggregateSessions: a row carrega o rótulo/cor do servidor dono.
  for (const b of buckets) for (const s of b.sessions) { s.serverLabel = b.label; s.serverColor = `c-${b.id}`; }
  store.byServer = buckets.map((b) => ({
    server: { id: b.id, label: b.label }, sessions: b.sessions,
    error: b.error ?? null, loaded: b.loaded ?? true,
  }));
  store.rows = buckets.flatMap((b) => b.sessions);
}
const opts = (variant: 'desktop' | 'mobile') => ({ variant, onOpen: vi.fn(), onCompare: vi.fn() });

beforeEach(() => { localStorage.clear(); vi.clearAllMocks(); });

describe('modo efetivo de agrupamento (divergência #1/#2 da spec)', () => {
  it('pref "none" com 2 servidores: desktop lista lisa, celular agrupa por servidor', () => {
    localStorage.setItem('cp_group_by', 'none');
    comServidores([
      { id: 'srv-a', label: 'A', sessions: [sess('x', 'srv-a')] },
      { id: 'srv-b', label: 'B', sessions: [sess('y', 'srv-b')] },
    ]);
    expect(createSessionListModel(opts('desktop')).groupMode).toBe('none');
    expect(createSessionListModel(opts('mobile')).groupMode).toBe('server');
  });
  it('pref "server" com 1 servidor cai pra "none" nas duas', () => {
    localStorage.setItem('cp_group_by', 'server');
    comServidores([{ id: 'srv-a', label: 'A', sessions: [sess('x', 'srv-a')] }]);
    expect(createSessionListModel(opts('desktop')).groupMode).toBe('none');
    expect(createSessionListModel(opts('mobile')).groupMode).toBe('none');
  });
  it('setGroupBy persiste e valor inválido no storage vira "server"', () => {
    localStorage.setItem('cp_group_by', 'lixo');
    comServidores([{ id: 'srv-a', label: 'A', sessions: [] }]);
    const m = createSessionListModel(opts('desktop'));
    expect(m.groupBy).toBe('server');
    m.setGroupBy('project');
    expect(localStorage.getItem('cp_group_by')).toBe('project');
    expect(m.groupMode).toBe('project');
  });
  it('"none" gravado pelo desktop: o celular LÊ como "server" (o toggle dele só conhece dois valores)', () => {
    localStorage.setItem('cp_group_by', 'none');
    comServidores([{ id: 'srv-a', label: 'A', sessions: [] }]);
    expect(createSessionListModel(opts('desktop')).groupBy).toBe('none');
    expect(createSessionListModel(opts('mobile')).groupBy).toBe('server');
  });
});

describe('grupos por servidor (divergência #3)', () => {
  beforeEach(() => {
    localStorage.setItem('cp_group_by', 'server');
    comServidores([
      { id: 'srv-z', label: 'Zeta', sessions: [sess('s1', 'srv-z')] },
      { id: 'srv-v', label: 'Vazio', sessions: [] },
      { id: 'srv-a', label: 'Alfa', sessions: [sess('s2', 'srv-a')] },
      { id: 'srv-o', label: 'Off', sessions: [], error: 'offline', loaded: false },
    ]);
  });
  it('desktop: ordem do store, grupos vazios e offline presentes (com o erro no grupo)', () => {
    const m = createSessionListModel(opts('desktop'));
    expect(m.allGroups.map((g) => g.label)).toEqual(['Zeta', 'Vazio', 'Alfa', 'Off']);
    expect(m.allGroups[3].error).toBe('offline');
    expect(m.allGroups[1].error).toBeNull();
  });
  it('contagens saem dos GRUPOS, não das rows cruas (o Sidebar.test só preenche byServer)', () => {
    store.rows = [];
    const m = createSessionListModel(opts('desktop'));
    expect(m.allSessions.map((s) => s.name)).toEqual(['s1', 's2']);
    expect(m.showFilter).toBe(false);
    store.byServer[0].sessions = Array.from({ length: 7 }, (_, i) => sess(`s${i}`, 'srv-z'));
    expect(createSessionListModel(opts('desktop')).showFilter).toBe(true);
  });
  it('celular: alfabético, só grupos com sessão', () => {
    const m = createSessionListModel(opts('mobile'));
    expect(m.groups.map((g) => g.label)).toEqual(['Alfa', 'Zeta']);
  });
  it('as duas: cor do servidor no grupo, e sessões aguardando sobem dentro do grupo', () => {
    store.byServer[0].sessions = [sess('b', 'srv-z'), sess('a', 'srv-z', { state: 'awaiting_input' })];
    store.rows = store.byServer.flatMap((b: any) => b.sessions);
    for (const v of ['desktop', 'mobile'] as const) {
      const g = createSessionListModel(opts(v)).groups.find((x) => x.label === 'Zeta')!;
      expect(g.color).toBe('c-srv-z');
      expect(g.sessions.map((s) => s.name)).toEqual(['a', 'b']);
    }
  });
});

describe('filtro (divergência #4)', () => {
  beforeEach(() => {
    localStorage.setItem('cp_group_by', 'project');
    comServidores([
      { id: 'srv-a', label: 'Alfa', sessions: [sess('um', 'srv-a', { cwd: '/w/hangar' }), sess('dois', 'srv-a', { cwd: '/w/outro' })] },
      { id: 'srv-b', label: 'Beta', sessions: [sess('tres', 'srv-b', { cwd: '/w/hangar' })] },
    ]);
  });
  it('desktop casa o rótulo do grupo (projeto); celular casa o rótulo do servidor', () => {
    const d = createSessionListModel(opts('desktop'));
    d.filterText = 'hangar';
    // sortSessions: alfabético dentro do grupo
    expect(d.groups.flatMap((g) => g.sessions.map((s) => s.name))).toEqual(['tres', 'um']);
    const c = createSessionListModel(opts('mobile'));
    c.filterText = 'beta';
    expect(c.groups.flatMap((g) => g.sessions.map((s) => s.name))).toEqual(['tres']);
    d.filterText = 'beta';
    expect(d.groups).toEqual([]);
    expect(d.filterEmpty).toBe(true);
  });
  it('showFilter só com mais de 6 sessões; sem filtro allGroups === groups', () => {
    const m = createSessionListModel(opts('desktop'));
    expect(m.showFilter).toBe(false);
    expect(m.groups).toBe(m.allGroups);
  });
  it('flatRows é a lista achatada na ordem dos grupos', () => {
    const m = createSessionListModel(opts('mobile'));
    // grupos em ordem alfabética (hangar, outro) e, dentro do grupo, sortSessions (tres < um)
    expect(m.flatRows.map((s) => s.name)).toEqual(['tres', 'um', 'dois']);
  });
});

describe('colapso de grupo', () => {
  it('persiste servidor/projeto e NÃO persiste cluster de pareamento', () => {
    comServidores([{ id: 'srv-a', label: 'A', sessions: [] }]);
    const m = createSessionListModel(opts('mobile'));
    m.toggleGroup('srv-a'); m.toggleGroup('pair:g1');
    expect(m.collapsed.has('srv-a')).toBe(true);
    expect(m.collapsed.has('pair:g1')).toBe(true);
    expect(JSON.parse(localStorage.getItem('cp_collapsed_servers')!)).toEqual(['srv-a']);
    m.toggleGroup('srv-a');
    expect(m.collapsed.has('srv-a')).toBe(false);
  });
  it('mount faz retain e o cleanup faz release', () => {
    comServidores([]);
    const off = createSessionListModel(opts('desktop')).mount();
    expect(store.retain).toHaveBeenCalledTimes(1);
    off();
    expect(store.release).toHaveBeenCalledTimes(1);
  });
});
