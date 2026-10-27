import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import fixtures from './updateAll.fixtures.json';
import { afterAccept, classify, follow, hasRealUpdate, ownUpdatableServers, prepareUpdateAll, updateAll } from './updateAll';
import * as m from './paraglide/messages';
import type { UpdateAllOptions, UpdateAllState } from './updateAll';
import { getAtualizacaoEm, getIdentificadorEm } from './api';
import { configureApi, _resetApiEnvForTests } from './apiEnv';
import type { Server } from './servers';
import type { AtualizacaoEstado } from './types';

const server = (id: string, extra: Partial<Server> = {}): Server => ({
  id, label: id, baseUrl: `https://${id}.test`, token: `token-${id}`, ...extra,
});
const baseline = '2026-10-21T10:00:00Z';
const ready = (state: AtualizacaoEstado = {}) => ({
  atualizacao_disponivel: true, versoes: { repo: 'abc', backend: 'abc' },
  pre_voo: { pode: true, faltando: [] }, estado: { ts: baseline, ...state }, mudancas: [], passos: [],
});
const done = (state: AtualizacaoEstado = {}) => ready({
  fase: 'pronto', ok: true, ts: '2026-10-21T10:00:01Z', pid: 999, ...state,
});
const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), {
  status, headers: { 'Content-Type': 'application/json' },
});

describe('contrato compartilhado com o Rust', () => {
  it.each(fixtures.classify)('$nome', ({ estado_get, esperado }) => {
    expect(classify(estado_get)).toBe(esperado);
  });
  it.each(fixtures.follow)('$nome', ({ estado, base_ts, esperado }) => {
    expect(follow(estado as AtualizacaoEstado, base_ts)).toBe(esperado);
  });
});

describe('aviso pendente e aceite', () => {
  const pending = { atualizacao_disponivel: false, pending_warning: true, versoes: { repo: 'a', backend: 'a' } };
  it('aviso pendente pede a folha, mas não é versão nova', () => {
    expect(classify(pending)).toBe('iniciar');
    expect(classify({ ...pending, pending_warning: false })).toBe('em_dia');
    // Modo desenvolvedor com edição: o aceite não faz git pull, então a edição não segura o aviso.
    const dirty = { pre_voo: { sujo: 1, branch_de_trabalho: true } };
    expect(classify({ ...pending, ...dirty })).toBe('iniciar');
    expect(classify({ ...pending, ...dirty, pending_warning: false, atualizacao_disponivel: true })).toBe('travado:branch');
    expect(hasRealUpdate(pending)).toBe(false);
    expect(hasRealUpdate({ ...pending, atualizacao_disponivel: true })).toBe(true);
    expect(hasRealUpdate({ ...pending, versoes: { repo: 'b', backend: 'a' } })).toBe(true);
  });
  it.each([
    ['servidor antigo inicia', null, pending, 'start'],
    ['lançado pelo aceite', { ok: true, started: true, pid: 5 }, pending, 'started'],
    ['gravado sem versão nova fecha', { ok: true, started: false }, pending, 'done'],
    ['gravado com versão nova inicia', { ok: true, started: false }, { ...pending, atualizacao_disponivel: true }, 'start'],
    ['nada a aceitar segue a versão', undefined, { atualizacao_disponivel: true }, 'start'],
  ] as const)('%s', (_name, accepted, snapshot, expected) => {
    expect(afterAccept(accepted, snapshot)).toBe(expected);
  });
});

it('filtra convites, desligadas, efêmeras e removidas, conservando a ordem', () => {
  const own = server('own');
  expect(ownUpdatableServers([
    own, server('invite', { invite: true }), server('disabled', { disabled: true }),
    server('ephemeral', { ephemeral: true }), server('removed', { removed: true }), server('other'),
  ])).toEqual([own, server('other')]);
});

describe('API e execução por máquina', () => {
  const fetchMock = vi.fn<typeof fetch>();
  let calls: string[];
  let reads: Map<string, number>;
  let updates: Map<string, UpdateAllState[]>;
  let clock: number;
  let options: UpdateAllOptions;
  let read: (id: string, count: number) => Response | Promise<Response>;
  let identity: (id: string) => Response | Promise<Response>;
  let start: (id: string) => Response | Promise<Response>;
  let accept: (id: string, body: unknown) => Response | Promise<Response>;
  let notes: (id: string) => Response | Promise<Response>;
  const onChange = (id: string, state: UpdateAllState) => {
    updates.set(id, [...(updates.get(id) ?? []), state]);
  };
  const final = (id: string) => updates.get(id)?.at(-1);

  beforeEach(() => {
    calls = []; reads = new Map(); updates = new Map(); clock = 0;
    options = { now: () => clock, sleep: async (ms) => { clock += ms; }, fetch: fetchMock };
    read = (_id, count) => json(count === 1 ? ready() : done());
    identity = (id) => json({ identificador: id, lan_url: '' });
    start = () => json({ ok: true, pid: 1 });
    accept = () => json({ ok: true, started: false });
    notes = () => json({ target: 'v2', warnings: [], news: [] });
    fetchMock.mockReset();
    fetchMock.mockImplementation(async (input, init) => {
      const url = new URL(String(input));
      const id = url.hostname.split('.')[0];
      calls.push(`${id}${url.pathname}${url.search}`);
      if (url.pathname === '/api/peers/identificador') return identity(id);
      if (url.pathname === '/api/atualizacao/iniciar') return start(id);
      if (url.pathname === '/api/update/accept') return accept(id, JSON.parse(String(init?.body)));
      if (url.pathname === '/api/update/notes') return notes(id);
      if (url.pathname === '/api/atualizacao') {
        const count = (reads.get(id) ?? 0) + 1;
        reads.set(id, count);
        return read(id, count);
      }
      throw new Error(`unexpected-route: ${url.pathname}`);
    });
    vi.stubGlobal('fetch', fetchMock);
    configureApi({ getBaseUrl: () => 'https://active.test', getToken: () => 'active-token',
      origin: null, onUnauthorized: () => { throw new Error('unexpected-logout'); }, createEventSource: vi.fn() });
  });
  afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); _resetApiEnvForTests(); });

  it('GET explícito usa rota, token e prazo; procurar recebe 120 s e polling 20 s', async () => {
    const timeout = vi.spyOn(AbortSignal, 'timeout');
    await getAtualizacaoEm(server('api'), true);
    expect(timeout).toHaveBeenLastCalledWith(120000);
    await getAtualizacaoEm(server('api'));
    expect(timeout).toHaveBeenLastCalledWith(20000);
    await getAtualizacaoEm(null, true);
    expect(timeout).toHaveBeenLastCalledWith(120000);
    expect(await getIdentificadorEm(server('api'))).toEqual({ identificador: 'api', lan_url: '' });
    expect(calls).toEqual(['api/api/atualizacao?procurar=1', 'api/api/atualizacao',
      'active/api/atualizacao?procurar=1', 'api/api/peers/identificador']);
    expect(new Headers(fetchMock.mock.calls[0][1]?.headers).get('Authorization')).toBe('Bearer token-api');
  });

  it('retira repetidas por identidade, preservando last como representante', async () => {
    identity = () => json({ identificador: 'same' });
    const result = await updateAll([server('alias'), server('last')], onChange, { ...options, last: 'last' });
    expect(result.map((item) => item.server.id)).toEqual(['last']);
    expect(calls.filter((path) => path.endsWith('/iniciar'))).toEqual(['last/api/atualizacao/iniciar']);
    expect(updates.has('alias')).toBe(false);
    expect(final('last')?.status).toBe('atualizado');
  });

  it('aceite por máquina antes de iniciar: lançado acompanha, gravado sem versão fecha, recusa não inicia', async () => {
    const bodies: Record<string, unknown> = {};
    accept = (id, body) => {
      bodies[id] = body;
      if (id === 'launched') return json({ ok: true, started: true, pid: 7 });
      if (id === 'refused') return json({ detail: 'fixture-accept' }, 500);
      if (id === 'old') return json({}, 404);
      return json({ ok: true, started: false });
    };
    read = (id, count) => {
      if (id === 'stored') return json({ ...ready(), atualizacao_disponivel: false, pending_warning: true });
      return json(count === 1 ? ready() : done());
    };
    const ids = ['launched', 'stored', 'refused', 'old', 'plain'];
    const acceptance = { ids: ['2026-10-24-pacote-por-versao'], mode: 'developer' as const };
    await updateAll(ids.map((id) => server(id)), onChange,
      { ...options, accept: { launched: acceptance, stored: acceptance, refused: acceptance, old: { ids: ['w'] } } });
    expect(bodies).toEqual({ launched: acceptance, stored: acceptance, refused: acceptance, old: { ids: ['w'] } });
    expect(calls.filter((path) => path.endsWith('/iniciar')).sort()).toEqual(['old/api/atualizacao/iniciar', 'plain/api/atualizacao/iniciar']);
    expect(final('launched')?.status).toBe('atualizado');
    expect(final('stored')?.status).toBe('em_dia');
    expect(final('refused')).toMatchObject({ status: 'falhou', error: m.update_accept_failed({ reason: '500: fixture-accept' }) });
    expect(final('old')?.status).toBe('atualizado');
  });

  it('prepara os avisos de cada máquina; quem falha fica de fora e 404 é sem avisos', async () => {
    notes = (id) => id === 'down' ? json({ detail: 'fixture-down' }, 503)
      : id === 'old' ? json({}, 404) : json({ target: 'v2', warnings: [{ id: 'w', title: 't', text: 'x', choice: 'mode', mode: null }], news: [] });
    const result = await prepareUpdateAll(['ok', 'down', 'old'].map((id) => server(id)), options);
    expect(result.run.map((entry) => [entry.server.id, entry.notes?.warnings.length ?? null])).toEqual([['ok', 1], ['old', null]]);
    expect(result.failed.map((entry) => entry.id)).toEqual(['down']);
  });

  it('quem não leu os avisos fica fora da rodada; o aceite de cada máquina chega ao POST dela', async () => {
    const bodies: Record<string, unknown> = {};
    accept = (id, body) => { bodies[id] = body; return json({ ok: true, started: false }); };
    notes = (id) => id === 'down' ? Promise.reject(new TypeError('fixture-network'))
      : json({ target: 'v2', warnings: [{ id: 'mig', title: 't', text: 'x', choice: 'mode', mode: null }], news: [] });
    const prepared = await prepareUpdateAll(['p', 'd', 'down'].map((id) => server(id)), options);
    await updateAll(prepared.run.map((entry) => entry.server), onChange, { ...options,
      accept: { p: { ids: ['mig'], mode: 'package' }, d: { ids: ['mig'], mode: 'developer' } } });
    expect(prepared.failed.map((entry) => entry.id)).toEqual(['down']);
    expect(bodies).toEqual({ p: { ids: ['mig'], mode: 'package' }, d: { ids: ['mig'], mode: 'developer' } });
    expect(calls.filter((path) => path.startsWith('down/') && !path.endsWith('/notes') && !path.endsWith('/identificador'))).toEqual([]);
    expect(updates.has('down')).toBe(false);
    expect(final('p')?.status).toBe('atualizado');
  });

  it('identidade vazia ou falha não junta máquinas', async () => {
    identity = (id) => id === 'badidentity' ? Promise.reject(new TypeError('fixture-network')) : json({ identificador: '' });
    const result = await updateAll([server('empty1'), server('empty2'), server('badidentity')], onChange, options);
    expect(result.map((item) => item.state.status)).toEqual(['atualizado', 'atualizado', 'atualizado']);
  });

  it('usa o fetch injetado sem depender do fetch global', async () => {
    vi.stubGlobal('fetch', () => { throw new Error('unexpected-global-fetch'); });
    await updateAll([server('injected')], onChange, options);
    expect(final('injected')?.status).toBe('atualizado');
  });

  it('em dia, travada e fora do ar não impedem sucesso, falha e manual das outras', async () => {
    read = (id, count) => {
      if (id === 'offline') throw new TypeError('fixture-offline');
      if (id === 'current') return json({ ...ready(), atualizacao_disponivel: false });
      if (id === 'branch') return json({ ...ready(), pre_voo: { branch_de_trabalho: true } });
      if (count === 1) return json(ready());
      return json(done({ ok: id !== 'failed', reiniciar_manual: id === 'manual',
        erro: 'fixture-error.', avisos: ['fixture-warning'] }));
    };
    await updateAll(['offline', 'current', 'branch', 'success', 'failed', 'manual'].map((id) => server(id)), onChange, options);
    expect(final('offline')).toMatchObject({ status: 'fora_do_ar', error: 'fixture-offline' });
    expect(final('current')?.status).toBe('em_dia');
    expect(final('branch')).toMatchObject({ status: 'travado', reason: 'branch' });
    expect(final('success')).toMatchObject({ status: 'atualizado', warnings: ['fixture-warning'] });
    expect(final('failed')).toMatchObject({ status: 'falhou', error: 'fixture-error' });
    expect(final('manual')?.status).toBe('atualizado_manual');
  });

  it('preserva erro HTTP da consulta e recusa do POST, sem derrubar outra máquina', async () => {
    read = (id, count) => id === 'http' ? json({ detail: 'fixture-http' }, 503) : json(count === 1 ? ready() : done());
    start = (id) => id === 'refused' ? json({ detail: 'fixture-refused' }, 409) : json({ ok: true, pid: 1 });
    await updateAll(['http', 'refused', 'ok'].map((id) => server(id)), onChange, options);
    expect(final('http')).toMatchObject({ status: 'falhou', error: '503: fixture-http' });
    expect(final('refused')).toMatchObject({ status: 'falhou', error: '409: fixture-refused' });
    expect(final('ok')?.status).toBe('atualizado');
  });

  it.each([401, 403])('encerra imediatamente com erro de autenticação no acompanhamento (%s)', async (status) => {
    read = (_id, count) => count === 1 ? json(ready()) : json({ detail: 'fixture-auth-error' }, status);
    await updateAll([server('auth')], onChange, options);
    expect(reads.get('auth')).toBe(2);
    expect(clock).toBe(2000);
    expect(final('auth')).toMatchObject({ status: 'falhou', error: `${status}: fixture-auth-error`, warnings: [] });
    expect(final('auth')?.errorCode).toBeUndefined();
  });

  it('acompanha por ts, tolera rede no reinício, publica passo e avisos, sem usar pid', async () => {
    read = (_id, count) => {
      if (count === 1 || count === 2) return json(ready({ fase: 'pronto', ok: true }));
      if (count === 3) throw new TypeError('fixture-restart');
      if (count === 4 || count === 5) return json(ready({ fase: 'rodando', passo: 3, total: 8, texto: 'fixture-step' }));
      return json(done({ avisos: ['fixture-native-warning'] }));
    };
    const sleep = vi.fn(options.sleep);
    await updateAll([server('restart')], onChange, { ...options, sleep });
    expect(updates.get('restart')?.filter((state) => state.status === 'rodando')).toHaveLength(1);
    expect(updates.get('restart')).toContainEqual(expect.objectContaining({ status: 'rodando',
      state: expect.objectContaining({ passo: 3, total: 8, texto: 'fixture-step' }) }));
    expect(final('restart')).toMatchObject({ status: 'atualizado', warnings: ['fixture-native-warning'] });
    expect(sleep.mock.calls.every(([ms]) => ms === 2000)).toBe(true);
  });

  it('encerra após 300 leituras seguidas sem resposta', async () => {
    read = (_id, count) => { if (count === 1) return json(ready()); throw new TypeError('fixture-silent'); };
    await updateAll([server('silent')], onChange, options);
    expect(reads.get('silent')).toBe(301);
    expect(final('silent')).toMatchObject({ status: 'falhou', errorCode: 'silent' });
    expect(final('silent')?.error).toBeUndefined();
  });

  it('uma resposta zera o silêncio e rodando não desiste por número de leituras', async () => {
    read = (_id, count) => {
      if (count === 1) return json(ready());
      if (count === 300) return json(ready({ fase: 'rodando' }));
      if (count === 599) return json(done());
      throw new TypeError('fixture-silent');
    };
    await updateAll([server('reset')], onChange, options);
    expect(reads.get('reset')).toBe(599);
    expect(final('reset')?.status).toBe('atualizado');
  });

  it('rodando chega ao teto de 30 min; desfecho nessa mesma leitura vence o teto', async () => {
    read = (_id, count) => json(count === 1 ? ready() : ready({ fase: 'rodando' }));
    await updateAll([server('ceiling')], onChange, options);
    expect(clock).toBe(30 * 60 * 1000);
    expect(final('ceiling')).toMatchObject({ status: 'falhou', errorCode: 'timeout' });
    expect(final('ceiling')?.error).toBeUndefined();
    clock = 0;
    read = (_id, count) => { if (count > 1) clock = 30 * 60 * 1000; return json(count === 1 ? ready() : done()); };
    await updateAll([server('terminal')], onChange, options);
    expect(final('terminal')?.status).toBe('atualizado');
  });

  it('inicia outras em paralelo e last espera todos os desfechos', async () => {
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    let announce!: () => void;
    const bothStarted = new Promise<void>((resolve) => { announce = resolve; });
    let started = 0;
    start = (id) => {
      if (id !== 'last' && ++started === 2) announce();
      return json({ ok: true, pid: 1 });
    };
    read = async (id, count) => {
      if (count === 1) return json(ready());
      if (id === 'slow') await gate;
      return json(done());
    };
    const run = updateAll(['last', 'slow', 'fast'].map((id) => server(id)), onChange, { ...options, last: 'last' });
    await bothStarted;
    expect(calls).not.toContain('last/api/atualizacao/iniciar');
    release();
    await run;
    expect(calls.filter((path) => path.endsWith('/iniciar'))).toEqual([
      'slow/api/atualizacao/iniciar', 'fast/api/atualizacao/iniciar', 'last/api/atualizacao/iniciar',
    ]);
  });
});
