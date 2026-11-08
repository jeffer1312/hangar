// @vitest-environment happy-dom
// (o vitest roda em `node` por padrão — sem isto não há localStorage; precedente: serverConfig.test.ts)
// frontend/src/lib/sessionListModel.test.svelte.ts
import { describe, it, expect, vi, beforeEach } from 'vitest';

// Store plano (sem reatividade), como o Sidebar.test.ts faz: preencher ANTES de criar o modelo.
const store = vi.hoisted(() => ({
  rows: [] as any[], byServer: [] as any[], servers: [] as any[], loading: false,
  identities: undefined as ReadonlyMap<string, string> | undefined,
  retain: vi.fn(), release: vi.fn(), markDeleting: vi.fn(), unmarkDeleting: vi.fn(),
}));
vi.mock('./sessionsStore.svelte', () => ({ sessionsStore: store }));
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  deleteSession: vi.fn(), renameSession: vi.fn(), resumeSession: vi.fn(), broadcast: vi.fn(), reopenFallenSession: vi.fn(),
}));
vi.mock('./auth', () => ({
  getActiveId: vi.fn(() => 'srv-a'), selectServer: vi.fn(), serverColor: (id: string) => `c-${id}`,
  findServer: (id: string) => ({ id, label: id, baseUrl: 'http://x', token: 't' }),
}));

import { clusterByPair } from '@hangar/core';
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

beforeEach(() => { localStorage.clear(); vi.clearAllMocks(); store.identities = undefined; });

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
  it('preserva o erro do servidor mesmo com sessões da última resposta boa', () => {
    store.byServer[0].error = 'offline';
    for (const variant of ['desktop', 'mobile'] as const) {
      const group = createSessionListModel(opts(variant)).allGroups.find(g => g.id === 'srv-z')!;
      expect(group.error).toBe('offline');
      expect(group.sessions.map(s => s.name)).toEqual(['s1']);
    }
  });
});

describe('grupo entre máquinas fica na máquina de cada membro, com selo (I-9A)', () => {
  beforeEach(() => {
    store.identities = new Map([['srv-a', 'notebook-jefferson'], ['srv-b', 'delphi-02']]);
    comServidores([
      { id: 'srv-a', label: 'Notebook', sessions: [
        sess('jefferson-2', 'srv-a', { pair_gid: 'b745f21b', pair_peers: ['delphi-02::setup-vm'], pair_task: 'TASK-123 Configurar VM', cwd: '/w/web' }),
        sess('solo-a', 'srv-a'),
      ] },
      { id: 'srv-b', label: 'Delphi', sessions: [
        sess('setup-vm', 'srv-b', { pair_gid: '3881e52a', pair_peers: ['notebook-jefferson::jefferson-2'], cwd: '/w/delphi' }),
        sess('solo-b', 'srv-b'),
      ] },
    ]);
  });
  const grupoDe = (model: ReturnType<typeof createSessionListModel>, name: string) =>
    model.allGroups.find(g => g.sessions.some(s => s.name === name))!;

  it.each((['desktop'] as const).flatMap(variant =>
    ['server', 'project'].map(mode => ({ variant, mode }))))('$variant/$mode: sem bloco no topo, cada membro no grupo da sua máquina ou projeto', ({ variant, mode }) => {
    localStorage.setItem('cp_group_by', mode);
    const model = createSessionListModel(opts(variant));
    expect(model.allGroups.every(g => g.label !== '')).toBe(true);
    expect(grupoDe(model, 'jefferson-2').id).toBe(mode === 'server' ? 'srv-a' : '/w/web');
    expect(grupoDe(model, 'setup-vm').id).toBe(mode === 'server' ? 'srv-b' : '/w/delphi');
    expect(model.allSessions).toHaveLength(4);
    expect(model.groupBadge(store.rows[0])).toBe('TASK-123');
    expect(model.groupBadge(store.rows[2])).toBe('TASK-123');
    expect(model.groupBadge(store.rows[1])).toBeNull();
  });

  it('grupo sem código de ticket mostra quantas máquinas, contando a desconectada', () => {
    comServidores([
      { id: 'srv-a', label: 'Notebook', sessions: [
        sess('x', 'srv-a', { pair_gid: 'g1', pair_owner: 'notebook-jefferson', pair_peers: ['delphi-02::y', 'lab::z'] }),
      ] },
      { id: 'srv-b', label: 'Delphi', sessions: [
        sess('y', 'srv-b', { pair_gid: 'g1', pair_owner: 'notebook-jefferson', pair_peers: ['notebook-jefferson::x', 'lab::z'] }),
      ] },
    ]);
    const model = createSessionListModel(opts('mobile'));
    expect(model.groupBadge(store.rows[0])).toBe(m.grupo_selo_maquinas({ n: 3 }));
    expect(model.groupBadge(store.rows[1])).toBe(m.grupo_selo_maquinas({ n: 3 }));
  });

  it('grupo de uma máquina só não ganha selo', () => {
    comServidores([{ id: 'srv-a', label: 'Notebook', sessions: [
      sess('x', 'srv-a', { pair_gid: 'g1', pair_peers: ['y'] }), sess('y', 'srv-a', { pair_gid: 'g1', pair_peers: ['x'] }),
    ] }]);
    expect(createSessionListModel(opts('mobile')).groupBadge(store.rows[0])).toBeNull();
  });

  it.each(['server', 'project'])('celular/%s: o par antigo entre máquinas é um bloco só, na posição do primeiro membro', (mode) => {
    localStorage.setItem('cp_group_by', mode);
    const model = createSessionListModel(opts('mobile'));
    const comPar = model.allGroups.filter(g => g.sessions.some(s => s.pair_gid));
    expect(comPar).toHaveLength(1);
    const headers = model.cluster(comPar[0].sessions).filter(i => i.kind === 'header');
    expect(headers).toEqual([expect.objectContaining({ count: 2, label: 'TASK-123 Configurar VM' })]);
    expect(model.pairMembers('3881e52a').map(s => s.name).sort()).toEqual(['jefferson-2', 'setup-vm']);
  });

  it('sem identidade confirmada o par antigo não vira selo e fica no cluster da máquina', () => {
    store.identities = undefined;
    const model = createSessionListModel(opts('desktop'));
    expect(model.groupBadge(store.rows[0])).toBeNull();
    const local = model.allGroups.find(g => g.id === 'srv-a')!;
    expect(clusterByPair(local.sessions).find(item => item.kind === 'header')).toMatchObject({ gid: 'b745f21b', count: 1 });
    expect(model.pairMembers('b745f21b').map(s => s.name)).toEqual(['jefferson-2']);
  });
});

describe('celular: grupo entre máquinas é um bloco só, onde o primeiro membro apareceria', () => {
  const peers = ['work::ABC-1-coord', 'work::ABC-1-proxy',
    'vm::ABC-1-app', 'vm::abc2-app'];
  const membro = (name: string, serverId: string, cwd: string) => sess(name, serverId, {
    pair_gid: 'g1', pair_owner: 'work', pair_task: 'ABC-1 Bulas', cwd,
    pair_peers: peers.filter(p => !p.endsWith(`::${name}`)),
  });
  const MEMBROS = ['ABC-1-coord', 'ABC-1-proxy', 'ABC-1-app', 'abc2-app'];
  beforeEach(() => {
    store.identities = new Map([['srv-n', 'work'], ['srv-d', 'vm']]);
    comServidores([
      { id: 'srv-n', label: 'work', sessions: [
        sess('aa-solo-n', 'srv-n', { cwd: '/w/aa' }),
        membro('ABC-1-coord', 'srv-n', '/w/hangar'), membro('ABC-1-proxy', 'srv-n', '/w/proxy'),
      ] },
      { id: 'srv-d', label: 'vm', sessions: [
        sess('aa-solo-d', 'srv-d', { cwd: '/w/aa' }), sess('zz-solo-d', 'srv-d', { cwd: '/w/zz' }),
        membro('ABC-1-app', 'srv-d', '/w/web-app'), membro('abc2-app', 'srv-d', '/w/web-app'),
      ] },
    ]);
  });

  it.each([
    // Máquina: "vm" vem antes; o primeiro membro é o 1º da vm depois de "aa-solo-d".
    { mode: 'server', grupo: 'srv-d', antes: ['aa-solo-d'] },
    // Projeto: "aa" vem antes de "hangar"; o primeiro membro é o do projeto "hangar".
    { mode: 'project', grupo: '/w/hangar', antes: [] as string[] },
  ])('$mode: 4 membros de 2 máquinas num bloco só', ({ mode, grupo, antes }) => {
    localStorage.setItem('cp_group_by', mode);
    const model = createSessionListModel(opts('mobile'));
    const comMembro = model.allGroups.filter(g => g.sessions.some(s => s.pair_gid));
    expect(comMembro.map(g => g.id)).toEqual([grupo]);
    const nomes = comMembro[0].sessions.map(s => s.name);
    expect(nomes.slice(0, antes.length)).toEqual(antes);
    expect(nomes.slice(antes.length, antes.length + 4).sort()).toEqual([...MEMBROS].sort());
    const itens = model.cluster(comMembro[0].sessions);
    expect(itens.filter(i => i.kind === 'header')).toEqual([expect.objectContaining({ count: 4, label: 'ABC-1 Bulas' })]);
    expect(itens.findIndex(i => i.kind === 'header')).toBe(antes.length);
    // Nenhuma seção fica vazia, e nenhum membro aparece fora do bloco.
    expect(model.allGroups.every(g => g.sessions.length > 0)).toBe(true);
    expect(model.allSessions).toHaveLength(7);
    expect(model.pairSeal('g1')).toBe(m.grupo_selo_maquinas({ n: 2 }));
    expect(model.memberMachine(store.rows.find(s => s.name === 'ABC-1-app'))).toBe('vm');
    expect(model.memberMachine(store.rows.find(s => s.name === 'aa-solo-d'))).toBeNull();
  });

  it('busca que acha um membro mostra o bloco inteiro', () => {
    localStorage.setItem('cp_group_by', 'server');
    const model = createSessionListModel(opts('mobile'));
    model.filterText = 'abc2';
    expect(model.groups).toHaveLength(1);
    expect(model.groups[0].sessions.map(s => s.name).sort()).toEqual([...MEMBROS].sort());
    expect(model.cluster(model.groups[0].sessions).find(i => i.kind === 'header')).toMatchObject({ count: 4 });
  });

  it('seção com erro de servidor fica, mesmo esvaziada pelo bloco', () => {
    localStorage.setItem('cp_group_by', 'server');
    const n = store.byServer.find((b: any) => b.server.id === 'srv-n');
    n.sessions = n.sessions.filter((s: any) => s.pair_gid);
    n.error = 'offline';
    store.rows = store.byServer.flatMap((b: any) => b.sessions);
    const secao = createSessionListModel(opts('mobile')).allGroups.find(g => g.id === 'srv-n');
    expect(secao).toMatchObject({ error: 'offline', sessions: [] });
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
  it('persiste servidor/projeto e cluster de pareamento VIVO; gid que não está na lista é podado', () => {
    comServidores([{ id: 'srv-a', label: 'A', sessions: [sess('um', 'srv-a', { pair_gid: 'g1' })] }]);
    const m = createSessionListModel(opts('mobile'));
    m.toggleGroup('srv-a'); m.toggleGroup('pair:g1'); m.toggleGroup('pair:morto');
    expect(m.collapsed.has('srv-a')).toBe(true);
    expect(m.collapsed.has('pair:g1')).toBe(true);
    expect(JSON.parse(localStorage.getItem('cp_collapsed_servers')!)).toEqual(['srv-a', 'pair:g1']);
    // Nova instância (lista remontada no celular): o grupo continua colapsado.
    expect(createSessionListModel(opts('mobile')).collapsed.has('pair:g1')).toBe(true);
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

import { broadcast, deleteSession, renameSession, reopenFallenSession, resumeSession } from '@hangar/core';
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
  it('orquestrador fica fora da seleção (clique e "enviar p/ todas")', () => {
    comServidores([
      { id: 'srv-a', label: 'Alfa', sessions: [sess('a1', 'srv-a'), sess('g1-orq', 'srv-a', { provider: 'orq' })] },
    ]);
    const m1 = createSessionListModel(opts('mobile'));
    // Um servidor só: a lista não agrupa por servidor e o grupo único não tem rótulo.
    m1.selectGroupForBroadcast(m1.groups.find((g) => g.sessions.some((s) => s.name === 'a1'))!);
    expect([...m1.selected]).toEqual(['srv-a:a1']);
    m1.toggleSelected('srv-a:g1-orq');
    expect(m1.selected.has('srv-a:g1-orq')).toBe(false);
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
  it('Codex "sem id" abre: a TUI pode estar parada num prompt e o terminal é a única saída', () => {
    const o = opts('desktop'); const m1 = createSessionListModel(o);
    expect(m1.open({ name: 'c', serverId: 'srv-a', tracked: false, provider: 'codex' })).toBe(true);
    expect(o.onOpen).toHaveBeenCalledWith('c');
  });
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

describe('sessão caída: reabrir e dispensar', () => {
  beforeEach(() => { comServidores([{ id: 'srv-b', label: 'B', sessions: [] }]); vi.mocked(getActiveId).mockReturnValue('srv-a'); });
  it('reabrir vai ao servidor da linha sem trocar o ativo', async () => {
    vi.mocked(reopenFallenSession).mockResolvedValue({ ok: true });
    const d = createSessionListModel(opts('desktop'));
    expect(await d.reopen('s', 'srv-b')).toBe('');
    expect(vi.mocked(reopenFallenSession).mock.calls[0]).toEqual(['s', expect.objectContaining({ id: 'srv-b' })]);
    expect(selectServer).not.toHaveBeenCalled();
    expect(d.reopenBusy).toBe('');
  });
  it('reabrir com erro devolve a mensagem do servidor', async () => {
    vi.mocked(reopenFallenSession).mockRejectedValue(new Error('nome em uso'));
    expect(await createSessionListModel(opts('desktop')).reopen('s', 'srv-b')).toBe('nome em uso');
  });
  it('dispensar apaga sem pedir confirmação', async () => {
    vi.mocked(deleteSession).mockResolvedValue({ ok: true, warning: null });
    const d = createSessionListModel(opts('desktop'));
    expect(await d.dismiss('s', 'srv-b')).toEqual({ ok: true, erro: '' });
    expect(d.confirmDel).toBeNull();
    expect(deleteSession).toHaveBeenCalledWith('s');
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

describe('par externo: sessão de fora dentro do grupo', () => {
  const PAR = { alias: 'casa', owner: 'Ana', session: 'api', address: 'https://ana.ts.net:8443', level: 'view', pending: false };
  function montar() {
    localStorage.setItem('cp_group_by', 'server');
    const local = sess('hangar', 'srv-a', { pair_gid: 'g1', pair_peers: ['casa::api'], pair_externals: [PAR] });
    const fora = sess('api', 'pair-1', { guest_kind: 'pair', serverInvite: true });
    store.servers = [
      { id: 'srv-a', label: 'Alfa', baseUrl: 'http://a', token: 't' },
      { id: 'pair-1', label: 'Par · Ana', baseUrl: 'https://ana.ts.net:8443', token: 'tp', invite: true, ephemeral: true },
    ];
    store.byServer = [
      { server: store.servers[0], sessions: [local], error: null, loaded: true },
      { server: store.servers[1], sessions: [fora], error: null, loaded: true },
    ];
    store.rows = [local, fora];
  }

  it('celular: a linha de fora entra no grupo local e a entrada temporária não vira seção', () => {
    montar();
    const m = createSessionListModel(opts('mobile'));
    expect(m.groupMode).toBe('none');
    expect(m.allGroups.flatMap((g) => g.sessions).map((s) => `${s.serverId}:${s.name}`).sort()).toEqual(['pair-1:api', 'srv-a:hangar']);
    const fora = m.allSessions.find((s) => s.name === 'api')!;
    expect(fora.pair_gid).toBe('g1');
    expect(fora.outside).toMatchObject({ owner: 'Ana', localSession: 'hangar', localServerId: 'srv-a' });
    expect(m.pairLabel('g1', 'hangar, api')).toBe('hangar ↔ casa::api (Ana)');
  });

  it('desktop web: nada novo, a entrada temporária fica de fora', () => {
    montar();
    const m = createSessionListModel(opts('desktop'));
    expect(m.allSessions.map((s) => s.name)).toEqual(['hangar']);
  });
});
