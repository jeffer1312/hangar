import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { SessionInfo } from '@hangar/core';

const { memory, calls, failWrites } = vi.hoisted(() => ({
  memory: new Map<string, string>(),
  failWrites: { on: false, drafts: false, remove: false },
  calls: {
    create: vi.fn(),
    sessions: vi.fn(),
    progress: vi.fn(),
    send: vi.fn(),
    history: vi.fn(),
  },
}));

vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (key: string) => memory.get(key),
    set: (key: string, value: string) => {
      if (failWrites.on || (failWrites.drafts && key.startsWith('draft.v1:'))) throw new Error('disk full');
      memory.set(key, value);
    },
    remove: (key: string) => {
      if (failWrites.remove) throw new Error('disk unavailable');
      memory.delete(key);
    },
  }),
}));
vi.mock('expo-secure-store', () => ({
  getItemAsync: async () => null,
  setItemAsync: async () => {},
  deleteItemAsync: async () => {},
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  createSessionForServer: calls.create,
  fetchSessionsForServer: calls.sessions,
  getCreationProgress: calls.progress,
  sendInputForServer: calls.send,
  getHistory: calls.history,
}));

import { useServers } from './servers';
import { readDraft, writeDraft } from './drafts';
import {
  _resetNewConversationForTests, abandonUnknownAttempt, adoptCandidate, beginAttempt, discardAttempt, recoverAttempt,
  restoreAttempt, useNewConversation, sendFirstInput, readFirstInput, confirmFirstInput, autoSessionName,
} from './newConversation';

const serverA = { id: 'server-a', label: 'A', baseUrl: 'http://a', token: 'secret-a' };
const serverB = { id: 'server-b', label: 'B', baseUrl: 'http://b', token: 'secret-b' };
const input = { body: { cwd: '/repo/mobile', provider: 'codex' as const }, text: 'oi' };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: unknown) => void;
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

async function settle() {
  for (let i = 0; i < 20; i++) await Promise.resolve();
}

const httpError = (status: number, message = `${status}: detalhe`) => Object.assign(new Error(message), { status });
const attempt = (serverId = 'server-a') => useNewConversation.getState().attempts[serverId];
const issue = (serverId = 'server-a') => useNewConversation.getState().issues[serverId];
const stored = (serverId = 'server-a') => JSON.parse(memory.get(`create.attempt.v1:${serverId}`)!);

describe('newConversation', () => {
  beforeEach(() => {
    memory.clear();
    failWrites.on = false;
    failWrites.drafts = false;
    failWrites.remove = false;
    vi.clearAllMocks();
    _resetNewConversationForTests();
    useServers.setState({ servers: [serverA, serverB], activeId: 'server-b' });
    calls.sessions.mockResolvedValue([]);
    calls.progress.mockResolvedValue({ step: null, params: {} });
  });

  it('criação publica sessão somente depois de gravar texto na chave real; recuperação termina handoff interrompido', async () => {
    calls.create.mockImplementation(async (_server, body) => {
      failWrites.drafts = true;
      return { name: body.name, state: 'idle', jsonl: '/transcript/first' };
    });
    await beginAttempt('server-a', input);
    expect(issue()?.kind).toBe('local');
    expect(attempt()?.sessionName).toBeNull();
    expect(stored().phase).toBe('created');
    const name = stored().sessionName;
    failWrites.drafts = false;
    _resetNewConversationForTests();
    expect(restoreAttempt('server-a')?.phase).toBe('created');
    expect(readDraft('server-a', name)).toMatchObject({ text: 'oi', submission: null });
    await beginAttempt('server-a', input);
    expect(calls.create).toHaveBeenCalledOnce();
  });

  it('persiste body, destino, nome e texto antes do POST e nunca grava token', async () => {
    const create = deferred<SessionInfo>();
    calls.create.mockReturnValue(create.promise);
    void beginAttempt('server-a', input);
    await settle();

    expect(calls.create).toHaveBeenCalledOnce();
    const saved = stored();
    expect(saved).toMatchObject({ serverId: 'server-a', text: 'oi', phase: 'creating', sessionName: null });
    expect(saved.body).toMatchObject({ cwd: '/repo/mobile', provider: 'codex' });
    expect(saved.body.name).toBe('mobile');
    expect(calls.create.mock.calls[0][0]).toBe(serverA);
    expect(calls.create.mock.calls[0][1]).toEqual(saved.body);
    expect(JSON.stringify([...memory.values()])).not.toContain('secret');
  });

  it('dois toques e remontagem durante a criação disparam um único POST', async () => {
    const create = deferred<SessionInfo>();
    calls.create.mockReturnValue(create.promise);
    const first = beginAttempt('server-a', input);
    const second = beginAttempt('server-a', { ...input, text: 'outro' });
    await settle();
    expect(restoreAttempt('server-a')?.phase).toBe('creating');
    void beginAttempt('server-a', input);
    await settle();
    expect(calls.create).toHaveBeenCalledOnce();

    create.resolve({ name: stored().body.name, state: 'idle' } as SessionInfo);
    await Promise.all([first, second]);
    expect(attempt()).toMatchObject({ phase: 'created', text: 'oi' });
    await beginAttempt('server-a', input);
    expect(calls.create).toHaveBeenCalledOnce();
  });

  it('evita nome já visível na lista sem depender dela para arbitrar', async () => {
    calls.create.mockImplementation(async (_s, body) => ({ name: body.name, state: 'idle' }));
    calls.sessions.mockResolvedValue([{ name: 'mobile', state: 'idle' }, { name: 'mobile-2', state: 'idle' }]);
    await beginAttempt('server-a', input);
    expect(attempt()?.sessionName).toBe('mobile-3');

    _resetNewConversationForTests(); memory.clear();
    calls.sessions.mockRejectedValue(new Error('offline'));
    await beginAttempt('server-a', input);
    expect(calls.create).toHaveBeenCalledTimes(2);
  });

  it('nome automático segue a regra das outras telas: acento vira letra sem acento', async () => {
    calls.create.mockImplementation(async (_s, body) => ({ name: body.name, state: 'idle' }));
    await beginAttempt('server-a', { ...input, body: { ...input.body, cwd: '/home/x/Área de trabalho' } });
    expect(calls.create.mock.calls[0][1].name).toBe('Area-de-trabalho');
  });

  it('processo reaberto converte creating em create_unknown e não repete o POST', async () => {
    calls.create.mockReturnValue(new Promise(() => {}));
    void beginAttempt('server-a', input);
    await settle();
    const id = stored().id;
    _resetNewConversationForTests();

    await beginAttempt('server-a', input);
    expect(attempt()?.phase).toBe('create_unknown');
    expect(stored()).toMatchObject({ id, phase: 'create_unknown' });
    expect(issue()?.kind).toBe('unknown');
    expect(calls.create).toHaveBeenCalledOnce();
  });

  it('processo reaberto converte sending em send_unknown', () => {
    memory.set('create.attempt.v1:server-a', JSON.stringify({
      id: 'attempt-1', serverId: 'server-a', text: 'oi', phase: 'sending', sessionName: 'repo-mobile',
      body: { name: 'repo-mobile', cwd: '/repo' },
    }));
    expect(restoreAttempt('server-a')).toMatchObject({ phase: 'send_unknown', sessionName: 'repo-mobile', text: 'oi' });
  });

  it.each([
    ['timeout', new Error('A não respondeu em 8s — servidor fora do ar?')],
    ['rede', new TypeError('Network request failed')],
    ['5xx', httpError(502)],
    ['408', httpError(408)],
  ])('create sem resultado conclusivo (%s) fica incerto e não cria outra sessão', async (_label, cause) => {
    calls.create.mockRejectedValue(cause);
    await beginAttempt('server-a', input);
    expect(attempt()?.phase).toBe('create_unknown');
    expect(issue()?.kind).toBe('unknown');
    await beginAttempt('server-a', { ...input, text: 'de novo' });
    expect(calls.create).toHaveBeenCalledOnce();
    expect(attempt()?.text).toBe('oi');
  });

  it('409 mostra a causa e não ganha novo sufixo automaticamente', async () => {
    calls.create.mockRejectedValue(httpError(409, '409: modo de permissao so vale para claude'));
    await beginAttempt('server-a', input);
    const name = attempt()!.body.name;
    expect(attempt()?.phase).toBe('create_unknown');
    expect(issue()).toEqual({ kind: 'conflict', message: '409: modo de permissao so vale para claude' });
    await beginAttempt('server-a', input);
    expect(calls.create).toHaveBeenCalledOnce();
    expect(attempt()?.body.name).toBe(name);
  });

  it('409 erro_nome_em_uso é recusa definitiva: volta ao rascunho e nunca recupera a sessão antiga pelo nome', async () => {
    calls.create.mockRejectedValue(Object.assign(httpError(409, 'já existe uma sessão com esse nome'), { code: 'erro_nome_em_uso' }));
    await beginAttempt('server-a', input);
    expect(attempt()).toMatchObject({ phase: 'draft', text: 'oi' });
    expect(issue()).toEqual({ kind: 'rejected', message: 'já existe uma sessão com esse nome' });

    const name = attempt()!.body.name;
    calls.sessions.mockResolvedValue([{ name, cwd: '/repo/mobile', provider: 'codex', state: 'idle' }]);
    await recoverAttempt('server-a');
    expect(issue()?.kind).toBe('rejected');
    expect(adoptCandidate('server-a', attempt()!.id)).toBe(false);
  });

  it('o nome automático das outras telas pula a lista e os recusados, e diz se a lista respondeu', async () => {
    calls.sessions.mockResolvedValue([{ name: 'mobile', state: 'idle' }]);
    expect(await autoSessionName(serverA, '/repo/mobile', ['mobile-2'])).toEqual({ name: 'mobile-3', known: true });
    calls.sessions.mockRejectedValue(new Error('offline'));
    expect(await autoSessionName(serverA, '/repo/mobile')).toEqual({ name: 'mobile', known: false });
  });

  it('nome automático em uso tenta uma vez com o próximo, mesmo sem a lista', async () => {
    calls.sessions.mockRejectedValue(new Error('503'));
    calls.create.mockImplementation(async (_s, body) => ({ name: body.name, state: 'idle' }));
    calls.create.mockRejectedValueOnce(Object.assign(httpError(409, 'já existe uma sessão com esse nome'), { code: 'erro_nome_em_uso' }));
    await beginAttempt('server-a', input);
    expect(calls.create.mock.calls.map((c) => c[1].name)).toEqual(['mobile', 'mobile-2']);
    expect(attempt()?.sessionName).toBe('mobile-2');
    expect(issue()).toBeNull();
  });

  it('a reserva só vai com o nome dela: a repetição por nome em uso e o nome digitado saem sem ela', async () => {
    calls.sessions.mockRejectedValue(new Error('503'));
    calls.create.mockImplementation(async (_s, body) => ({ name: body.name, state: 'idle' }));
    calls.create.mockRejectedValueOnce(Object.assign(httpError(409, 'já existe uma sessão com esse nome'), { code: 'erro_nome_em_uso' }));
    const reserved = { ...input, body: { ...input.body, prewarm: 'k1' }, reserveName: 'reserva' };
    await beginAttempt('server-a', reserved);
    expect(calls.create.mock.calls.map((c) => [c[1].name, c[1].prewarm])).toEqual([['reserva', 'k1'], ['mobile', undefined]]);

    _resetNewConversationForTests();
    memory.clear();
    calls.create.mockClear();
    await beginAttempt('server-a', { ...reserved, body: { ...reserved.body, name: 'meu-nome' } });
    expect(calls.create.mock.calls[0][1]).toMatchObject({ name: 'meu-nome' });
    expect(calls.create.mock.calls[0][1]).not.toHaveProperty('prewarm');
  });

  it('nome explícito é gravado antes do POST e não ganha sufixo nem troca após conflito', async () => {
    const create = deferred<SessionInfo>();
    calls.create.mockReturnValue(create.promise);
    calls.sessions.mockResolvedValue([{ name: 'meu-nome', state: 'idle' }]);
    void beginAttempt('server-a', { ...input, body: { ...input.body, name: ' meu-nome ' } });
    await settle();
    expect(stored().body.name).toBe('meu-nome');
    expect(calls.create.mock.calls[0][1]).toEqual(stored().body);

    create.reject(httpError(409, '409: nome em uso'));
    await settle();
    expect(attempt()).toMatchObject({ phase: 'create_unknown', body: { name: 'meu-nome' } });
    expect(restoreAttempt('server-a')?.body.name).toBe('meu-nome');
    await beginAttempt('server-a', { ...input, body: { ...input.body, name: 'outro' } });
    expect(calls.create).toHaveBeenCalledOnce();
  });

  it('branch nova sem nome segue o nome final da sessão; com nome digitado, fica o digitado', async () => {
    calls.create.mockImplementation(async (_s, body) => ({ name: body.name, state: 'idle' }));
    const worktree = { new_branch: true, base: 'main' };
    await beginAttempt('server-a', { ...input, body: { ...input.body, ...worktree, branch: '' } });
    const sent = calls.create.mock.calls[0][1];
    expect(sent.name).toBe('mobile');
    expect(sent).toMatchObject({ branch: sent.name, new_branch: true, base: 'main' });

    _resetNewConversationForTests(); memory.clear();
    await beginAttempt('server-a', { ...input, body: { ...input.body, ...worktree, branch: 'feat-x', name: 'meu-nome' } });
    expect(calls.create.mock.calls[1][1]).toMatchObject({ name: 'meu-nome', branch: 'feat-x', new_branch: true });
  });

  it('branch nova sem nome segue o nome digitado da sessão já limpo', async () => {
    calls.create.mockImplementation(async (_s, body) => ({ name: body.name, state: 'idle' }));
    await beginAttempt('server-a', { ...input, body: { ...input.body, new_branch: true, branch: '', name: 'Minha Sessão' } });
    expect(calls.create.mock.calls[0][1]).toMatchObject({ name: 'Minha Sessão', branch: 'Minha-Sessao', new_branch: true });
  });

  it('recusa definitiva (cwd inválido) volta ao rascunho editável com o motivo do servidor', async () => {
    calls.create.mockRejectedValueOnce(httpError(400, '400: diretório não existe'));
    await beginAttempt('server-a', input);
    expect(attempt()).toMatchObject({ phase: 'draft', text: 'oi' });
    expect(issue()).toEqual({ kind: 'rejected', message: '400: diretório não existe' });

    calls.create.mockImplementation(async (_s, body) => ({ name: body.name, state: 'idle' }));
    await beginAttempt('server-a', { ...input, body: { ...input.body, cwd: '/repo/outro' } });
    expect(attempt()).toMatchObject({ phase: 'created' });
    expect(attempt()?.body.cwd).toBe('/repo/outro');
    expect(calls.create).toHaveBeenCalledTimes(2);
  });

  it('GET vazio não prova falha: recuperação só consulta e mantém a incerteza', async () => {
    calls.create.mockRejectedValue(httpError(503));
    await beginAttempt('server-a', input);
    calls.sessions.mockClear();

    await recoverAttempt('server-a');
    expect(calls.sessions).toHaveBeenCalledWith(serverA);
    expect(calls.progress).toHaveBeenCalledWith(attempt()!.body.name, serverA);
    expect(attempt()?.phase).toBe('create_unknown');
    expect(issue()?.kind).toBe('not_found');

    calls.progress.mockResolvedValue({ step: 'spawn', params: {} });
    await recoverAttempt('server-a');
    expect(issue()?.kind).toBe('in_progress');

    calls.sessions.mockRejectedValue(new Error('offline'));
    await recoverAttempt('server-a');
    expect(issue()?.kind).toBe('recover_failed');
    expect(calls.create).toHaveBeenCalledOnce();
  });

  it('candidata compatível só vira criada por ação explícita; incompatível é conflito', async () => {
    calls.create.mockRejectedValue(httpError(504));
    await beginAttempt('server-a', input);
    const name = attempt()!.body.name;

    calls.sessions.mockResolvedValue([{ name, cwd: '/outro', provider: 'codex', state: 'idle' }]);
    await recoverAttempt('server-a');
    expect(issue()?.kind).toBe('conflict');
    expect(adoptCandidate('server-a', attempt()!.id)).toBe(false);

    calls.sessions.mockResolvedValue([{ name, cwd: '/repo/mobile', provider: 'codex', state: 'idle' }]);
    await recoverAttempt('server-a');
    expect(issue()?.kind).toBe('candidate');
    expect(attempt()?.phase).toBe('create_unknown');
    expect(adoptCandidate('server-a', 'outra-tentativa')).toBe(false);
    expect(adoptCandidate('server-a', attempt()!.id)).toBe(true);
    expect(attempt()).toMatchObject({ phase: 'created', sessionName: name });
    expect(stored().phase).toBe('created');
  });

  it('destino fica congelado no servidor capturado; troca de máquina não recebe o resultado', async () => {
    const create = deferred<SessionInfo>();
    calls.create.mockReturnValue(create.promise);
    void beginAttempt('server-a', input);
    await settle();
    useServers.setState({ activeId: 'server-a' });
    useServers.setState({ activeId: 'server-b' });
    create.resolve({ name: stored().body.name, state: 'idle' } as SessionInfo);
    await settle();

    expect(attempt('server-a')?.phase).toBe('created');
    expect(attempt('server-b')).toBeUndefined();
    expect(memory.has('create.attempt.v1:server-b')).toBe(false);
  });

  it('resultado antigo não pisa na tentativa nova depois de descartar', async () => {
    calls.create.mockRejectedValueOnce(httpError(500));
    await beginAttempt('server-a', input);
    const old = attempt()!;
    discardAttempt('server-a', old.id);
    expect(memory.has('create.attempt.v1:server-a')).toBe(false);

    const create = deferred<SessionInfo>();
    calls.create.mockReturnValue(create.promise);
    void beginAttempt('server-a', { ...input, text: 'nova' });
    await settle();
    expect(adoptCandidate('server-a', old.id)).toBe(false);
    create.resolve({ name: stored().body.name, state: 'idle' } as SessionInfo);
    await settle();
    expect(attempt()).toMatchObject({ phase: 'created', text: 'nova' });
  });

  it('falha ao gravar no aparelho não dispara POST nem publica tentativa', async () => {
    failWrites.on = true;
    await beginAttempt('server-a', input);
    expect(calls.create).not.toHaveBeenCalled();
    expect(attempt()).toBeUndefined();
    expect(issue()?.kind).toBe('local');
  });

  it('tentativa gravada inválida é descartada com aviso', () => {
    memory.set('create.attempt.v1:server-a', '{"token":"secret"');
    expect(restoreAttempt('server-a')).toBeNull();
    expect(issue()?.kind).toBe('local');
    expect(issue()?.message).not.toContain('secret');
    expect(memory.has('create.attempt.v1:server-a')).toBe(false);
  });
});

// Sem ACK não há limpeza; falha de input nunca inicia outra criação.
describe('primeiro input', () => {
  beforeEach(() => {
    memory.clear(); failWrites.on = false; failWrites.drafts = false; failWrites.remove = false; vi.clearAllMocks();
    _resetNewConversationForTests();
    useServers.setState({ servers: [serverA, serverB], activeId: 'server-a' });
    calls.sessions.mockResolvedValue([]);
    calls.create.mockImplementation(async (_server, body) => ({ name: body.name, state: 'idle' }));
    calls.send.mockResolvedValue(undefined);
    calls.history.mockResolvedValue([]);
  });

  it('persiste sessão antes do input; dois toques usam snapshot único e só ACK permite finalizar', async () => {
    const ack = deferred<void>();
    calls.send.mockImplementation(() => {
      expect(stored()).toMatchObject({ phase: 'sending', sessionName: attempt()!.sessionName, text: 'oi' });
      expect(JSON.parse(memory.get(`draft.v1:server-a::${attempt()!.sessionName}`)!).submission)
        .toMatchObject({ text: 'oi', status: 'sending' });
      return ack.promise;
    });
    await beginAttempt('server-a', input);
    const created = attempt()!;
    const first = sendFirstInput('server-a', created.id);
    const second = sendFirstInput('server-a', created.id);
    expect(first).toBe(second);
    confirmFirstInput(created.id);
    expect(stored().phase).toBe('sending');
    expect(calls.send).toHaveBeenCalledOnce();
    expect(calls.send).toHaveBeenCalledWith(serverA, created.sessionName, 'oi');
    useServers.setState({ activeId: 'server-b' });
    ack.resolve(); await first;
    expect(attempt()).toMatchObject({ phase: 'sent', text: 'oi', sessionName: created.sessionName });
    expect(attempt('server-b')).toBeUndefined();
    expect(readFirstInput('server-b', created.sessionName!)).toBeNull();
    expect(readFirstInput('server-a', 'outra')).toBeNull();
    expect(readFirstInput('server-a', created.sessionName!)).toEqual(attempt());
    confirmFirstInput('outro-id');
    expect(attempt()?.id).toBe(created.id);
    confirmFirstInput(created.id);
    expect(readFirstInput('server-a', created.sessionName!)).toBeNull();
    expect(memory.has('create.attempt.v1:server-a')).toBe(false);
    expect(readDraft('server-a', created.sessionName!)?.text).toBe('');
  });

  it('input recusado mantém sessão/texto e nova ação envia nela sem outro create', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    calls.send.mockRejectedValueOnce(httpError(404, 'sessão indisponível'));
    await sendFirstInput('server-a', created.id);
    expect(attempt()).toMatchObject({ id: created.id, phase: 'created', text: 'oi', sessionName: created.sessionName });
    expect(issue()?.kind).toBe('rejected');
    expect(readDraft('server-a', created.sessionName!)).toMatchObject({ text: 'oi', submission: { text: 'oi', status: 'rejected' } });
    confirmFirstInput(created.id);
    expect(attempt()?.id).toBe(created.id);
    await beginAttempt('server-a', { ...input, text: 'nova edição' });
    await sendFirstInput('server-a', created.id);
    expect(calls.create).toHaveBeenCalledOnce();
    expect(calls.send).toHaveBeenCalledTimes(2);
    expect(calls.send.mock.calls[1]).toEqual([serverA, created.sessionName, 'oi']);
    expect(attempt()?.phase).toBe('sent');
  });

  it.each([new TypeError('offline'), new Error('sem status'), httpError(503), httpError(408)])(
    'input incerto conserva snapshot após reabertura e não repete automaticamente (%s)', async (cause) => {
      await beginAttempt('server-a', input);
      const created = attempt()!;
      calls.send.mockRejectedValueOnce(cause);
      await sendFirstInput('server-a', created.id);
      expect(stored()).toMatchObject({ phase: 'send_unknown', text: 'oi' });
      expect(readDraft('server-a', created.sessionName!)?.submission?.status).toBe('unknown');
      _resetNewConversationForTests();
      expect(readFirstInput('server-a', created.sessionName!)?.phase).toBe('send_unknown');
      expect(issue()?.kind).toBe('unknown');
      await beginAttempt('server-a', input);
      await sendFirstInput('server-a', created.id);
      confirmFirstInput(created.id);
      expect(attempt()?.phase).toBe('send_unknown');
      expect(calls.send).toHaveBeenCalledOnce();
      expect(calls.create).toHaveBeenCalledOnce();
    },
  );

  it('consulta histórico/fila no destino original sem confirmar pelo texto igual nem reenviar', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    calls.send.mockRejectedValueOnce(httpError(502));
    await sendFirstInput('server-a', created.id);
    useServers.setState({ activeId: 'server-b' });
    const history = [{ kind: 'user_msg', text: 'oi', id: 'outro-envio' }];
    calls.history.mockResolvedValue(history);
    await recoverAttempt('server-a');
    expect(calls.history).toHaveBeenCalledWith(created.sessionName, 120, undefined, 10000, serverA);
    expect(issue()).toMatchObject({ kind: 'unknown', events: history });
    expect(attempt()?.phase).toBe('send_unknown');
    confirmFirstInput(created.id);
    expect(attempt()?.phase).toBe('send_unknown');
    calls.history.mockResolvedValueOnce([]);
    await recoverAttempt('server-a');
    expect(issue()).toMatchObject({ kind: 'unknown', events: [] });
    expect(attempt()?.phase).toBe('send_unknown');
    calls.history.mockRejectedValueOnce(httpError(404));
    await recoverAttempt('server-a');
    expect(issue()?.kind).toBe('recover_failed');
    expect(attempt()?.phase).toBe('send_unknown');
    expect(calls.send).toHaveBeenCalledOnce();
  });

  it('abandono explícito remove só send_unknown e preserva rascunho e outro servidor', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    expect(abandonUnknownAttempt('server-a', created.id)).toBe(false);
    calls.send.mockRejectedValueOnce(new TypeError('offline'));
    await sendFirstInput('server-a', created.id);
    await beginAttempt('server-b', { ...input, text: 'outro destino' });
    const draft = readDraft('server-a', created.sessionName!);
    expect(abandonUnknownAttempt('server-a', 'id-antigo')).toBe(false);
    expect(abandonUnknownAttempt('server-a', created.id)).toBe(true);
    expect(attempt()).toBeUndefined();
    expect(memory.has('create.attempt.v1:server-a')).toBe(false);
    expect(issue()).toBeNull();
    expect(readDraft('server-a', created.sessionName!)).toEqual(draft);
    expect(attempt('server-b')?.text).toBe('outro destino');
    expect(calls.send).toHaveBeenCalledOnce();
  });

  it('abandono não remove tentativa sent nem inexistente', async () => {
    expect(abandonUnknownAttempt('server-a', 'ausente')).toBe(false);
    await beginAttempt('server-a', input);
    const created = attempt()!;
    await sendFirstInput('server-a', created.id);
    expect(attempt()?.phase).toBe('sent');
    expect(abandonUnknownAttempt('server-a', created.id)).toBe(false);
    expect(stored().phase).toBe('sent');
  });

  it('abandono durante consulta em voo não remove nem envia; depois pode ser explícito', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    calls.send.mockRejectedValueOnce(new TypeError('offline'));
    await sendFirstInput('server-a', created.id);
    const history = deferred<[]>();
    calls.history.mockReturnValueOnce(history.promise);
    const recovery = recoverAttempt('server-a');
    expect(abandonUnknownAttempt('server-a', created.id)).toBe(false);
    expect(stored().phase).toBe('send_unknown');
    expect(attempt()?.id).toBe(created.id);
    history.resolve([]);
    await recovery;
    expect(abandonUnknownAttempt('server-a', created.id)).toBe(true);
    expect(calls.send).toHaveBeenCalledOnce();
  });

  it('falha ao remover chave conserva tentativa no disco e na memória e mostra erro', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    calls.send.mockRejectedValueOnce(new TypeError('offline'));
    await sendFirstInput('server-a', created.id);
    const unknown = attempt();
    failWrites.remove = true;
    expect(abandonUnknownAttempt('server-a', created.id)).toBe(false);
    expect(attempt()).toBe(unknown);
    expect(stored()).toMatchObject({ id: created.id, phase: 'send_unknown' });
    expect(issue()).toMatchObject({ kind: 'local' });
    expect(calls.send).toHaveBeenCalledOnce();
  });

  it('falha de persistência antes do input impede POST; depois do ACK preserva incerteza', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    failWrites.on = true;
    await sendFirstInput('server-a', created.id);
    expect(calls.send).not.toHaveBeenCalled();
    expect(attempt()?.phase).toBe('created');
    expect(issue()?.kind).toBe('local');
    failWrites.on = false;
    calls.send.mockImplementation(async () => { failWrites.on = true; });
    await sendFirstInput('server-a', created.id);
    expect(stored().phase).toBe('sending');
    expect(issue()?.kind).toBe('local');
    confirmFirstInput(created.id);
    expect(attempt()?.id).toBe(created.id);
    failWrites.on = false;
    _resetNewConversationForTests();
    expect(readFirstInput('server-a', created.sessionName!)?.phase).toBe('send_unknown');
    await sendFirstInput('server-a', created.id);
    expect(calls.send).toHaveBeenCalledOnce();
  });

  it('falha só no draft antes do input mostra erro local sem declarar envio incerto', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    failWrites.drafts = true;
    await sendFirstInput('server-a', created.id);
    expect(calls.send).not.toHaveBeenCalled();
    expect(attempt()?.phase).toBe('created');
    expect(issue()?.kind).toBe('local');
  });

  it('ACK recebido seguido de falha no draft registra sent com aviso local', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    calls.send.mockImplementation(async () => { failWrites.drafts = true; });
    await sendFirstInput('server-a', created.id);
    expect(calls.send).toHaveBeenCalledOnce();
    expect(attempt()?.phase).toBe('sent');
    expect(issue()?.kind).toBe('local');
  });

  it('ACK tardio não apaga texto de tentativa nova nem altera seu destino', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    await sendFirstInput('server-a', created.id);
    confirmFirstInput(created.id);
    // A sessão criada já está na lista: a tentativa nova ganha outro nome.
    calls.sessions.mockResolvedValue([{ name: created.sessionName!, state: 'idle' }]);
    await beginAttempt('server-a', { ...input, text: 'nova edição' });
    const next = attempt()!;
    confirmFirstInput(created.id);
    await sendFirstInput('server-a', created.id);
    expect(attempt()).toMatchObject({ id: next.id, phase: 'created', text: 'nova edição' });
    expect(calls.send).toHaveBeenCalledOnce();
    expect(readFirstInput('server-a', created.sessionName!)).toBeNull();
  });

  it('ACK do primeiro input conserva edição posterior na sessão real após confirmar a transferência', async () => {
    await beginAttempt('server-a', input);
    const created = attempt()!;
    expect(readDraft('server-a', created.sessionName!)?.text).toBe('oi');
    const ack = deferred<void>();
    calls.send.mockReturnValue(ack.promise);
    const sending = sendFirstInput('server-a', created.id);
    const current = readDraft('server-a', created.sessionName!)!;
    writeDraft('server-a', created.sessionName!, { ...current, text: 'edição posterior', revision: current.revision + 1 });
    ack.resolve(); await sending;
    confirmFirstInput(created.id);
    _resetNewConversationForTests();
    expect(readDraft('server-a', created.sessionName!)).toMatchObject({ text: 'edição posterior', submission: null });
    expect(readFirstInput('server-a', created.sessionName!)).toBeNull();
    expect(calls.send).toHaveBeenCalledOnce();
  });
});
