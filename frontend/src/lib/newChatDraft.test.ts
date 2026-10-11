import { beforeEach, describe, expect, it, vi } from 'vitest';

const srv = { id: 'pc', label: 'PC', baseUrl: 'http://pc:8765', token: 't' };
const core = vi.hoisted(() => ({
  createSessionForServer: vi.fn(), sendInputForServer: vi.fn(), fetchSessionsForServer: vi.fn(),
  getCodexAccountsForServer: vi.fn(), getFolderBranchesForServer: vi.fn(), getRootsForServer: vi.fn(),
  listClaudeConfigs: vi.fn(), getClaudeAccountSuggestion: vi.fn(), getProviders: vi.fn(), modelOptions: vi.fn(),
  modelOptionsForServer: vi.fn(), getEnginesForServer: vi.fn(), getConfigForServer: vi.fn(), setClaudeDefaultsForServer: vi.fn(),
  uploadFileForServer: vi.fn(),
}));
vi.mock('@hangar/core', async (orig) => ({ ...(await orig<object>()), ...core }));
vi.mock('./auth', () => ({ listOwnServers: () => [srv], selectServer: vi.fn(() => true), getActiveId: () => 'pc' }));

import { createNewChatDraft, isNotRepo, worktreeChoiceOf } from './newChatDraft.svelte';

function never() { return new Promise(() => {}); }
const flush = () => new Promise((r) => setTimeout(r, 0));

// vitest roda em node, sem localStorage.
function fakeStorage(initial: Record<string, string>) {
  const store = new Map(Object.entries(initial));
  return {
    getItem: (k: string) => store.get(k) ?? null,
    setItem: (k: string, v: string) => { store.set(k, v); },
    removeItem: (k: string) => { store.delete(k); },
  };
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.stubGlobal('localStorage', fakeStorage({ 'cp_newchat_cwd:pc': '/home/u/proj' }));
  for (const fn of [core.getRootsForServer, core.getFolderBranchesForServer, core.getClaudeAccountSuggestion])
    fn.mockImplementation(never);
  core.getProviders.mockResolvedValue({});
  core.listClaudeConfigs.mockResolvedValue([{ path: '/c', label: 'c', active: true }]);
  core.modelOptions.mockResolvedValue({ models: [], reduced: false });
  core.modelOptionsForServer.mockResolvedValue({ models: [], reduced: false });
  core.getEnginesForServer.mockResolvedValue({ motores: {}, arquivo_corrompido: false });
  core.getConfigForServer.mockResolvedValue({ campos: { headless_default: { valor: true } } });
});

async function ready() {
  const draft = createNewChatDraft();
  draft.init();
  await flush();
  return draft;
}

describe('NewChatDraft.send', () => {
  it('cria com o nome desempatado e envia no nome devolvido pelo backend', async () => {
    core.fetchSessionsForServer.mockResolvedValue([{ name: 'proj' }]);
    core.createSessionForServer.mockResolvedValue({ name: 'proj-3' });
    core.sendInputForServer.mockResolvedValue(undefined);
    const draft = await ready();

    const r = await draft.send('oi');

    expect(core.createSessionForServer).toHaveBeenCalledWith(srv, expect.objectContaining({
      name: 'proj-2', cwd: '/home/u/proj', provider: 'claude', config_dir: '/c' }));
    expect(core.sendInputForServer).toHaveBeenCalledWith(srv, 'proj-3', 'oi');
    expect(r).toEqual({ serverId: 'pc', name: 'proj-3' });
  });

  it('envio que falha depois de criar: tentar de novo reenvia na mesma sessão', async () => {
    core.fetchSessionsForServer.mockResolvedValue([]);
    core.createSessionForServer.mockResolvedValue({ name: 'proj' });
    core.sendInputForServer.mockRejectedValueOnce(new Error('500: caiu')).mockResolvedValueOnce(undefined);
    const draft = await ready();

    await expect(draft.send('oi')).rejects.toThrow('caiu');
    expect(draft.note?.text).toBe('500: caiu');
    await draft.send('oi');

    expect(core.createSessionForServer).toHaveBeenCalledTimes(1);
    expect(core.sendInputForServer).toHaveBeenLastCalledWith(srv, 'proj', 'oi');
  });

  it('anexo que falha: tentar de novo reaproveita o anexo já enviado à mesma sessão', async () => {
    core.fetchSessionsForServer.mockResolvedValue([]);
    core.createSessionForServer.mockResolvedValue({ name: 'proj' });
    core.sendInputForServer.mockResolvedValue(undefined);
    core.uploadFileForServer
      .mockResolvedValueOnce({ path: '/up/a.txt' })
      .mockRejectedValueOnce(new Error('500: caiu'))
      .mockResolvedValueOnce({ path: '/up/b.txt' });
    const draft = await ready();
    const a = new File(['a'], 'a.txt', { type: 'text/plain' });
    const b = new File(['b'], 'b.txt', { type: 'text/plain' });
    draft.attachments = [a, b];

    await expect(draft.send('oi')).rejects.toThrow('caiu');
    await draft.send('oi');

    expect(core.createSessionForServer).toHaveBeenCalledTimes(1);
    expect(core.uploadFileForServer.mock.calls.map((c) => c[2])).toEqual([a, b, b]);
    expect(core.sendInputForServer).toHaveBeenCalledWith(srv, 'proj', expect.stringMatching(/\/up\/a\.txt[\s\S]*\/up\/b\.txt/));
  });

  it('trocar uma escolha depois da falha de envio cria sessão nova com a escolha nova', async () => {
    core.fetchSessionsForServer.mockResolvedValue([]);
    core.createSessionForServer.mockResolvedValueOnce({ name: 'proj' }).mockResolvedValueOnce({ name: 'proj-2' });
    core.sendInputForServer.mockRejectedValueOnce(new Error('500: caiu')).mockResolvedValueOnce(undefined);
    const draft = await ready();

    await expect(draft.send('oi')).rejects.toThrow('caiu');
    draft.branch = 'feature';
    await draft.send('oi');

    expect(core.createSessionForServer).toHaveBeenCalledTimes(2);
    expect(core.createSessionForServer).toHaveBeenLastCalledWith(srv, expect.objectContaining({ branch: 'feature' }));
    expect(core.sendInputForServer).toHaveBeenLastCalledWith(srv, 'proj-2', 'oi');
  });

  it('branch nova sem nome nasce com o nome da sessão, e tentar de novo não cria outra sessão', async () => {
    core.fetchSessionsForServer.mockResolvedValue([{ name: 'proj' }]);
    core.createSessionForServer.mockResolvedValue({ name: 'proj-2' });
    core.sendInputForServer.mockRejectedValueOnce(new Error('500: caiu')).mockResolvedValueOnce(undefined);
    const draft = await ready();
    draft.newBranch = true;
    draft.base = 'develop';

    await expect(draft.send('oi')).rejects.toThrow('caiu');
    await draft.send('oi');

    expect(core.createSessionForServer).toHaveBeenCalledTimes(1);
    expect(core.createSessionForServer).toHaveBeenCalledWith(srv, expect.objectContaining({
      name: 'proj-2', branch: 'proj-2', new_branch: true, base: 'develop' }));
    expect(draft.branchName).toBe('proj-2');
  });

  it('contas ainda carregando: não envia', async () => {
    core.listClaudeConfigs.mockImplementation(never);
    const draft = createNewChatDraft();
    draft.init();
    expect(draft.loading).toBe(true);
    await expect(draft.send('oi')).rejects.toThrow();
    expect(core.createSessionForServer).not.toHaveBeenCalled();
  });

  it('sem pasta não cria nada', async () => {
    localStorage.removeItem('cp_newchat_cwd:pc');
    const draft = await ready();
    await expect(draft.send('oi')).rejects.toThrow();
    expect(core.createSessionForServer).not.toHaveBeenCalled();
  });
});

describe('NewChatDraft — respostas atrasadas', () => {
  it('espera a seleção inicial e respeita escolha manual feita antes da resposta', async () => {
    let resolve!: (value: unknown) => void;
    core.getProviders.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const draft = await ready();
    expect(draft.loading).toBe(true);
    await expect(draft.send('oi')).rejects.toThrow();
    draft.setProvider('claude');
    resolve({ claude: { disponivel: true }, codex: { disponivel: true, default: true } });
    await flush();
    expect(draft.provider).toBe('claude');
    expect(core.createSessionForServer).not.toHaveBeenCalled();
  });

  it('pré-seleciona GPT com Claude instalado e usa a conta Codex conectada', async () => {
    core.getProviders.mockResolvedValue({ claude: { disponivel: true }, codex: { disponivel: true, default: true } });
    core.getCodexAccountsForServer.mockResolvedValue([
      { id: 'default', is_default: true, auth: { status: 'disconnected' } },
      { id: 'gpt', is_default: false, auth: { status: 'connected' } },
    ]);
    core.fetchSessionsForServer.mockResolvedValue([]);
    core.createSessionForServer.mockResolvedValue({ name: 'proj' });
    core.sendInputForServer.mockResolvedValue(undefined);
    const draft = await ready();
    expect(draft.provider).toBe('codex');
    expect(draft.codexAccount).toBe('gpt');
    await draft.send('oi');
    expect(core.createSessionForServer).toHaveBeenCalledWith(srv, expect.objectContaining({
      provider: 'codex', codex_account: 'gpt', remember_provider: true,
    }));
  });

  it('trocar de provider com o catálogo do Claude em voo: a resposta velha é descartada e o modelo some', async () => {
    localStorage.setItem('cp_last_model:pc:claude:-', 'opus');
    let resolveClaude!: (v: unknown) => void;
    core.modelOptions.mockImplementationOnce(() => new Promise((r) => { resolveClaude = r; }));
    core.getCodexAccountsForServer.mockRejectedValue(new Error('codex fora'));
    const draft = await ready();
    expect(draft.modelsLoading).toBe(true);

    draft.setProvider('codex');
    await flush();
    resolveClaude({ models: [{ id: 'opus' }], reduced: false });
    await flush();

    expect(draft.models).toEqual([]);
    expect(draft.model).toBe('');
    expect(draft.modelsLoading).toBe(false);
    expect(draft.note?.text).toBe('codex fora');
    await expect(draft.send('oi')).rejects.toThrow();
    expect(core.createSessionForServer).not.toHaveBeenCalled();
  });
});

describe('isNotRepo', () => {
  it('404 e 409 "not a git repository" não são falha; outro 409 é', () => {
    expect(isNotRepo(Object.assign(new Error('404: x'), { status: 404 }))).toBe(true);
    expect(isNotRepo(Object.assign(new Error('409: fatal: not a git repository'), { status: 409 }))).toBe(true);
    expect(isNotRepo(Object.assign(new Error('409: outra coisa'), { status: 409 }))).toBe(false);
    expect(isNotRepo(new Error('rede'))).toBe(false);
  });
});

describe('NewChatDraft.switchFromExhausted', () => {
  const conta = (path: string, pct: number, ativa = false) => ({
    id: `claude:${path}`, label: path, provedor: 'claude' as const, ativa, estado: 'lida' as const,
    janelas: [{ rotulo: '5h', pct, nivel: 'normal' as const, resetTs: null, porModelo: false }],
    velha: false, idade_s: 1, motivo: null, loginVenceDias: null,
  });
  beforeEach(() => {
    core.listClaudeConfigs.mockResolvedValue([
      { path: '/c', label: 'c', active: true }, { path: '/d', label: 'd', active: false }]);
  });

  it('conta escolhida esgotada troca pra conta com folga e recarrega os modelos', async () => {
    const draft = await ready();
    const antes = core.modelOptions.mock.calls.length;
    draft.switchFromExhausted([conta('/c', 100, true), conta('/d', 20)]);
    await flush();
    expect(draft.configDir).toBe('/d');
    expect(core.modelOptions.mock.calls.length).toBeGreaterThan(antes);
    const depois = core.modelOptions.mock.calls.length;
    draft.switchFromExhausted([conta('/c', 100, true), conta('/d', 20)]);
    await flush();
    expect(draft.configDir).toBe('/d');
    expect(core.modelOptions.mock.calls.length).toBe(depois);
  });

  it('escolher só o modelo não desliga a troca', async () => {
    const draft = await ready();
    draft.setModel('x');
    draft.switchFromExhausted([conta('/c', 100, true), conta('/d', 20)]);
    expect(draft.configDir).toBe('/d');
  });

  it('com as contas carregando ou fora do claude não troca', async () => {
    const draft = await ready();
    draft.configsLoading = true;
    draft.switchFromExhausted([conta('/c', 100, true), conta('/d', 20)]);
    expect(draft.configDir).toBe('/c');
    draft.configsLoading = false;
    draft.provider = 'codex';
    draft.switchFromExhausted([conta('/c', 100, true), conta('/d', 20)]);
    expect(draft.configDir).toBe('/c');
  });

  it('escolha manual não é desfeita', async () => {
    const draft = await ready();
    draft.setConfig('/c');
    draft.switchFromExhausted([conta('/c', 100, true), conta('/d', 20)]);
    expect(draft.configDir).toBe('/c');
  });

  it('sem conta com folga fica na atual', async () => {
    const draft = await ready();
    draft.switchFromExhausted([conta('/c', 100, true), conta('/d', 100)]);
    expect(draft.configDir).toBe('/c');
  });
});

describe('worktreeChoiceOf', () => {
  it('branch existente', () => {
    expect(worktreeChoiceOf({ branch: 'x', newBranch: false, base: '', branchName: '' })).toEqual({ branch: 'x' });
  });
  it('branch nova a partir da base', () => {
    expect(worktreeChoiceOf({ branch: '', newBranch: true, base: 'develop', branchName: 'rust-parte3' }))
      .toEqual({ branch: 'rust-parte3', new_branch: true, base: 'develop' });
  });
  it('nada escolhido', () => {
    expect(worktreeChoiceOf({ branch: '', newBranch: false, base: '', branchName: '' })).toBeNull();
  });
});

describe('NewChatDraft — abertura igual à do desktop', () => {
  it('Codex como padrão do servidor não herda o Bypass do Claude', async () => {
    core.getProviders.mockResolvedValue({ claude: { disponivel: true }, codex: { disponivel: true, default: true } });
    core.getCodexAccountsForServer.mockResolvedValue([{ id: 'gpt', is_default: true, auth: { status: 'connected' } }]);
    const draft = await ready();
    expect(draft.provider).toBe('codex');
    expect(draft.permission).toBe('Full Access');
  });

  it('sem a config do servidor lida, headless fica nulo e o envio espera', async () => {
    core.getConfigForServer.mockImplementation(never);
    const draft = await ready();
    expect(draft.headless).toBeNull();
    expect(draft.loading).toBe(true);
  });

  it('Claude nasce em Bypass e manda o modo de execução lido do servidor', async () => {
    core.getConfigForServer.mockResolvedValue({ campos: { headless_default: { valor: false } } });
    core.fetchSessionsForServer.mockResolvedValue([]);
    core.createSessionForServer.mockResolvedValue({ name: 'proj' });
    core.sendInputForServer.mockResolvedValue(undefined);
    const draft = await ready();
    await draft.send('oi');
    expect(core.createSessionForServer).toHaveBeenCalledWith(srv, expect.objectContaining({
      provider: 'claude', permission_mode: 'bypassPermissions', headless: false }));
  });
});

describe('NewChatDraft — Usar como padrão', () => {
  it('só fica marcado enquanto a escolha bate com o padrão salvo', async () => {
    core.modelOptions.mockResolvedValue({ models: [{ id: 'opus' }, { id: 'sonnet' }], reduced: false });
    const draft = await ready();
    await flush();
    draft.setModel('opus');
    await draft.saveDefault(true);
    expect(draft.isDefault).toBe(true);
    draft.setModel('sonnet');
    expect(draft.isDefault).toBe(false);
  });
});
