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

import { broadcast, deleteSession, renameSession, resumeSession } from './api';
import { getActiveId, selectServer } from './auth';
import * as m from '../paraglide/messages';

describe('seleção, broadcast e comparar (divergência #6 e #15)', () => {
  beforeEach(() => {
    localStorage.setItem('cp_group_by', 'server');
    comServidores([
      { id: 'srv-b', label: 'Beta', sessions: [sess('b1', 'srv-b')] },
      { id: 'srv-a', label: 'Alfa', sessions: [sess('a1', 'srv-a'), sess('semid', 'srv-a', { tracked: false })] },
    ]);
  });
  it('toggleSelectMode limpa seleção e texto; toggleSelected alterna a chave', () => {
    const m1 = createSessionListModel(opts('desktop'));
    m1.toggleSelectMode();
    m1.toggleSelected('srv-a:a1'); m1.broadcastText = 'oi';
    expect(m1.selected.has('srv-a:a1')).toBe(true);
    m1.toggleSelectMode();
    expect(m1.selectMode).toBe(false);
    expect(m1.selected.size).toBe(0);
    expect(m1.broadcastText).toBe('');
    m1.openSelectMode(); m1.openSelectMode();
    expect(m1.selectMode).toBe(true);
  });
  it('selectGroupForBroadcast marca o grupo inteiro menos "sem id"', () => {
    const m1 = createSessionListModel(opts('mobile'));
    m1.selectGroupForBroadcast(m1.groups.find((g) => g.label === 'Alfa')!);
    expect([...m1.selected]).toEqual(['srv-a:a1']);
    expect(m1.selectMode).toBe(true);
  });
  it('slash-command e seleção vazia desabilitam o envio; comparar pede 2+', () => {
    const m1 = createSessionListModel(opts('desktop'));
    m1.toggleSelectMode();
    m1.broadcastText = '/clear';
    expect(m1.broadcastDisabled).toBe(true);
    m1.toggleSelected('srv-a:a1'); m1.broadcastText = 'oi';
    expect(m1.broadcastDisabled).toBe(false);
    expect(m1.compareDisabled).toBe(true);
    m1.toggleSelected('srv-b:b1');
    expect(m1.compareDisabled).toBe(false);
  });
  it('openCompare: por servidor as duas ordens coincidem (Beta antes de Alfa no store e nos grupos)', () => {
    const od = opts('desktop'); const d = createSessionListModel(od);
    d.toggleSelectMode(); d.toggleSelected('srv-a:a1'); d.toggleSelected('srv-b:b1');
    d.openCompare();
    expect(od.onCompare).toHaveBeenCalledWith([{ serverId: 'srv-b', name: 'b1' }, { serverId: 'srv-a', name: 'a1' }]);
    const oc = opts('mobile'); const c = createSessionListModel(oc);
    c.toggleSelectMode(); c.toggleSelected('srv-a:a1'); c.toggleSelected('srv-b:b1');
    c.openCompare();
    expect(oc.onCompare).toHaveBeenCalledWith([{ serverId: 'srv-b', name: 'b1' }, { serverId: 'srv-a', name: 'a1' }]);
  });
  it('sendBroadcast: uma chamada por servidor, restaura o ativo, agrega falhas por nome', async () => {
    vi.mocked(broadcast).mockImplementation(async (names: string[]) =>
      Object.fromEntries(names.map((n) => [n, { ok: n !== 'b1' }])) as any);
    const m1 = createSessionListModel(opts('desktop'));
    m1.toggleSelectMode(); m1.toggleSelected('srv-a:a1'); m1.toggleSelected('srv-b:b1');
    m1.broadcastText = 'oi';
    await m1.sendBroadcast();
    expect(broadcast).toHaveBeenCalledTimes(2);
    expect(selectServer).toHaveBeenLastCalledWith('srv-a');           // getActiveId mockado = 'srv-a'
    expect(m1.broadcastMsg).toBe(m.lista_broadcast_falha({ nomes: 'b1' }));
    expect(m1.selectMode).toBe(true);                                  // falhou: continua selecionando
  });
  it('sendBroadcast sem falha: limpa texto, seleção e sai do modo', async () => {
    vi.mocked(broadcast).mockResolvedValue({ a1: { ok: true } } as any);
    const m1 = createSessionListModel(opts('mobile'));
    m1.toggleSelectMode(); m1.toggleSelected('srv-a:a1'); m1.broadcastText = 'oi';
    await m1.sendBroadcast();
    expect(m1.selectMode).toBe(false);
    expect(m1.broadcastText).toBe('');
  });
  it('servidor offline (broadcast rejeita): o lote inteiro dele conta como falho', async () => {
    vi.mocked(broadcast).mockRejectedValue(new Error('offline'));
    const m1 = createSessionListModel(opts('desktop'));
    m1.toggleSelectMode(); m1.toggleSelected('srv-a:a1'); m1.broadcastText = 'oi';
    await m1.sendBroadcast();
    expect(m1.broadcastMsg).toBe(m.lista_broadcast_falha({ nomes: 'a1' }));
  });
  // A diferença de regra só aparece quando o agrupamento reordena: por projeto.
  it('compareOrder por projeto: desktop segue a ordem alfabética dos grupos, celular a das rows', () => {
    localStorage.setItem('cp_group_by', 'project');
    store.byServer[0].sessions[0].cwd = '/w/zz'; store.byServer[1].sessions[0].cwd = '/w/aa';
    const od = opts('desktop'); const d = createSessionListModel(od);
    d.toggleSelectMode(); d.toggleSelected('srv-a:a1'); d.toggleSelected('srv-b:b1'); d.openCompare();
    expect(od.onCompare).toHaveBeenCalledWith([{ serverId: 'srv-a', name: 'a1' }, { serverId: 'srv-b', name: 'b1' }]);
    const oc = opts('mobile'); const c = createSessionListModel(oc);
    c.toggleSelectMode(); c.toggleSelected('srv-a:a1'); c.toggleSelected('srv-b:b1'); c.openCompare();
    expect(oc.onCompare).toHaveBeenCalledWith([{ serverId: 'srv-b', name: 'b1' }, { serverId: 'srv-a', name: 'a1' }]);
  });
});

describe('abrir sessão (divergência #18)', () => {
  beforeEach(() => comServidores([{ id: 'srv-a', label: 'A', sessions: [] }]));
  it('rastreada abre; "sem id" bloqueia, exceto Kimi', () => {
    const o = opts('desktop'); const m1 = createSessionListModel(o);
    expect(m1.open({ name: 'x', serverId: 'srv-a', tracked: true })).toBe(true);
    expect(selectServer).toHaveBeenCalledWith('srv-a');
    expect(o.onOpen).toHaveBeenCalledWith('x');
    expect(m1.open({ name: 'y', serverId: 'srv-a', tracked: false, provider: 'claude' })).toBe(false);
    expect(o.onOpen).toHaveBeenCalledTimes(1);
    expect(m1.open({ name: 'k', serverId: 'srv-a', tracked: false, provider: 'kimi' })).toBe(true);
  });
});

describe('excluir (divergências #7 e #8)', () => {
  beforeEach(() => {
    comServidores([{ id: 'srv-b', label: 'B', sessions: [sess('s', 'srv-b')] }]);
    vi.mocked(getActiveId).mockReturnValue('srv-a');
  });
  it('otimista: marca, chama a API e devolve ok; desktop restaura o ativo, celular não', async () => {
    vi.mocked(deleteSession).mockResolvedValue({ ok: true, warning: null });
    const d = createSessionListModel(opts('desktop'));
    d.requestDelete('s', 'srv-b', 'idle');
    expect(d.confirmDel).toEqual({ name: 's', serverId: 'srv-b', state: 'idle' });
    expect(await d.doDelete()).toEqual({ ok: true, erro: '' });
    expect(d.confirmDel).toBeNull();
    expect(store.markDeleting).toHaveBeenCalledWith('srv-b', 's');
    expect(vi.mocked(selectServer).mock.calls.map((c) => c[0])).toEqual(['srv-b', 'srv-a']);
    vi.mocked(selectServer).mockClear();
    const c = createSessionListModel(opts('mobile'));
    c.requestDelete('s', 'srv-b');
    await c.doDelete();
    expect(vi.mocked(selectServer).mock.calls.map((x) => x[0])).toEqual(['srv-b']);
  });
  it('falha: desmarca (a linha reaparece) e devolve o erro em vez de lançar', async () => {
    vi.mocked(deleteSession).mockRejectedValue(new Error('psmux não matou'));
    const d = createSessionListModel(opts('desktop'));
    d.requestDelete('s', 'srv-b');
    expect(await d.doDelete()).toEqual({ ok: false, erro: 'psmux não matou' });
    expect(store.unmarkDeleting).toHaveBeenCalledWith('srv-b', 's');
  });
  it('doDelete sem confirmação pendente é no-op', async () => {
    expect(await createSessionListModel(opts('desktop')).doDelete()).toEqual({ ok: false, erro: '' });
    expect(deleteSession).not.toHaveBeenCalled();
  });
  it('kill com aviso de saída falho: sessão morta (ok) mas mostra o motivo', async () => {
    vi.mocked(deleteSession).mockResolvedValue({
      ok: true,
      warning: { code: 'erro_pareamento_saida_falhou', params: { avisos: [] }, msg: 'aviso de saída falhou' },
    });
    const d = createSessionListModel(opts('desktop'));
    d.requestDelete('s', 'srv-b');
    const r = await d.doDelete();
    expect(r.ok).toBe(true);
    expect(r.erro).not.toBe('');
  });
});

describe('renomear (divergências #9 e #10)', () => {
  beforeEach(() => { comServidores([{ id: 'srv-b', label: 'B', sessions: [] }]); vi.mocked(getActiveId).mockReturnValue('srv-a'); });
  it('sucesso: devolve o nome; a sessão ABERTA renomeada troca a rota (só desktop)', async () => {
    vi.mocked(renameSession).mockResolvedValue({ ok: true, name: 'novo' } as any);
    const o = { ...opts('desktop'), currentSession: () => 'velho' };
    const d = createSessionListModel(o);
    expect(await d.rename('novo', 'velho', 'srv-b')).toEqual({ ok: true, name: 'novo', erro: '' });
    expect(o.onOpen).toHaveBeenCalledWith('novo');
    expect(vi.mocked(selectServer).mock.calls.map((c) => c[0])).toEqual(['srv-b', 'srv-a']);
    const oc = opts('mobile'); const c = createSessionListModel(oc);
    await c.rename('novo2', 'velho', 'srv-b');
    expect(oc.onOpen).not.toHaveBeenCalled();
  });
  it('falha: devolve {ok:false, erro} com o nome antigo', async () => {
    vi.mocked(renameSession).mockRejectedValue(new Error('já existe'));
    const d = createSessionListModel(opts('desktop'));
    expect(await d.rename('x', 'velho', 'srv-b')).toEqual({ ok: false, name: 'velho', erro: 'já existe' });
  });
});

describe('retomar (divergências #11 e #12)', () => {
  beforeEach(() => { comServidores([{ id: 'srv-b', label: 'B', sessions: [] }]); vi.mocked(getActiveId).mockReturnValue('srv-a'); });
  it('caso seguro: religa e limpa candidatos; desktop restaura o ativo, celular não', async () => {
    vi.mocked(resumeSession).mockResolvedValue({ ok: true } as any);
    const d = createSessionListModel(opts('desktop'));
    expect(await d.resume('s', 'srv-b')).toEqual({ ok: true, ambiguous: false, erro: '' });
    expect(d.resumeCandidates).toBeNull();
    expect(d.resumeBusy).toBe('');
    expect(vi.mocked(selectServer).mock.calls.map((c) => c[0])).toEqual(['srv-b', 'srv-a']);
    vi.mocked(selectServer).mockClear();
    await createSessionListModel(opts('mobile')).resume('s', 'srv-b');
    expect(vi.mocked(selectServer).mock.calls.map((c) => c[0])).toEqual(['srv-b']);
  });
  it('ambíguo: guarda os candidatos com nome e servidor', async () => {
    const cands = [{ session_id: '1', preview: 'p', mtime: 1, in_use: false }];
    vi.mocked(resumeSession).mockResolvedValue({ ambiguous: true, candidates: cands } as any);
    const d = createSessionListModel(opts('desktop'));
    expect(await d.resume('s', 'srv-b')).toEqual({ ok: true, ambiguous: true, erro: '' });
    expect(d.resumeCandidates).toEqual({ name: 's', serverId: 'srv-b', candidates: cands });
  });
  it('falha: resumeError preenchido e devolvido; busy volta a vazio', async () => {
    vi.mocked(resumeSession).mockRejectedValue(new Error('pane morto'));
    const d = createSessionListModel(opts('desktop'));
    expect(await d.resume('s', 'srv-b')).toEqual({ ok: false, ambiguous: false, erro: 'pane morto' });
    expect(d.resumeError).toBe('pane morto');
    expect(d.resumeBusy).toBe('');
  });
});

describe('Git mira o servidor dono e restaura ao fechar (nas duas)', () => {
  it('openGit/closeGit', () => {
    comServidores([{ id: 'srv-b', label: 'B', sessions: [] }]);
    vi.mocked(getActiveId).mockReturnValue('srv-a');
    for (const v of ['desktop', 'mobile'] as const) {
      vi.mocked(selectServer).mockClear();
      const m1 = createSessionListModel(opts(v));
      m1.openGit('s', 'srv-b');
      expect(m1.gitSheet).toEqual({ name: 's' });
      m1.closeGit();
      expect(m1.gitSheet).toBeNull();
      expect(vi.mocked(selectServer).mock.calls.map((c) => c[0])).toEqual(['srv-b', 'srv-a']);
    }
  });
});
