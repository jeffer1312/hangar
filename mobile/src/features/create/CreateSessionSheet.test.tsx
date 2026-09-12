// @vitest-environment happy-dom
import { act, createElement, StrictMode, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const calls = vi.hoisted(() => ({
  accounts: vi.fn(),
  models: vi.fn(),
  prepare: vi.fn(),
  preparation: vi.fn(),
  create: vi.fn(),
  archives: vi.fn(),
  resume: vi.fn(),
  replace: vi.fn(),
  alert: vi.fn(),
}));
const server = { id: 'server-b', label: 'Servidor B', baseUrl: 'https://b.local', token: 'token-b' };

vi.mock('expo-router', () => ({ useRouter: () => ({ replace: calls.replace }) }));
vi.mock('react-native', async (original) => ({
  ...await original<typeof import('react-native')>(),
  Alert: { alert: calls.alert },
}));
vi.mock('../../stores/servers', () => ({
  useServers: Object.assign(
    (selector: (state: { active: () => typeof server; servers: typeof server[] }) => unknown) => selector({ active: () => server, servers: [server] }),
    { getState: () => ({ servers: [server], active: () => server }) },
  ),
}));
vi.mock('./CwdPicker', () => ({ CwdPicker: ({ onPick }: { onPick: (path: string) => void }) => createElement('button', { onClick: () => onPick('/repo') }, 'Pasta') }));
vi.mock('./ProviderPicker', () => ({ ProviderPicker: ({ onChange }: { onChange: (provider: string) => void }) => createElement('div', null,
  createElement('button', { onClick: () => onChange('codex') }, 'Codex'),
  createElement('button', { onClick: () => onChange('claude') }, 'Claude'),
) }));
vi.mock('./CodexContextControl', () => ({ CodexContextControl: () => null }));
vi.mock('@react-native-menu/menu', () => ({
  MenuView: ({ actions, onPressAction, children }: { actions: { id: string; title: string }[]; onPressAction: (event: { nativeEvent: { event: string } }) => void; children: ReactNode }) => createElement('div', null,
    children,
    ...actions.map((action) => createElement('button', { key: action.id, onClick: () => onPressAction({ nativeEvent: { event: action.id } }) }, action.title)),
  ),
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  listClaudeConfigs: vi.fn().mockResolvedValue([]),
  listarCotasResumo: vi.fn().mockResolvedValue([]),
  getEngines: vi.fn().mockResolvedValue({ motores: {} }),
  modelOptions: vi.fn().mockResolvedValue({ models: [], reduced: false }),
  getSessions: vi.fn().mockResolvedValue([]),
  getCodexAccountsForServer: calls.accounts,
  modelOptionsForServer: calls.models,
  prepareCodexAccountForServer: calls.prepare,
  getCodexPreparationForServer: calls.preparation,
  createSessionForServer: calls.create,
  getArchivePorCwd: calls.archives,
  resumeArchivedConversation: calls.resume,
}));
vi.mock('../../paraglide/messages', () => ({
  codex_ui_account: () => 'codex_ui_account', codex_ui_login_error: () => 'codex_ui_login_error',
  codex_ui_prepare_error: () => 'codex_ui_prepare_error', codex_ui_unknown: () => 'codex_ui_unknown',
  codex_ui_abrindo_sessao: () => 'codex_ui_abrindo_sessao',
  composer_esforco: () => 'composer_esforco', composer_modelo: () => 'composer_modelo',
  comum_carregando: () => 'comum_carregando', comum_conta_claude: () => 'comum_conta_claude',
  comum_motor: () => 'comum_motor', comum_nome: () => 'comum_nome', comum_provider: () => 'comum_provider',
  contas_nao_conectada: () => 'contas_nao_conectada', cota_conta_parada: () => 'cota_conta_parada',
  cota_precisa_entrar: () => 'cota_precisa_entrar', cota_sem_cota: () => 'cota_sem_cota',
  criar_abre_padrao: ({ erro }: { erro: string }) => `criar_abre_padrao:${erro}`, criar_avancado: () => 'criar_avancado',
  criar_caminho_placeholder: () => 'criar_caminho_placeholder', criar_claude_sua_conta: () => 'criar_claude_sua_conta',
  criar_conta_aria: () => 'criar_conta_aria', criar_criando: () => 'criar_criando', criar_ja_existe: () => 'criar_ja_existe',
  criar_lista_reduzida: () => 'criar_lista_reduzida', criar_modelos_erro: () => 'criar_modelos_erro',
  criar_nome_placeholder: () => 'criar_nome_placeholder', criar_outra_pasta: () => 'criar_outra_pasta',
  criar_padrao: () => 'criar_padrao', criar_permissao: () => 'criar_permissao', criar_permissao_padrao: () => 'criar_permissao_padrao',
  criar_raciocinio: () => 'criar_raciocinio', criar_retomar: () => 'criar_retomar', criar_retomar_acao: () => 'criar_retomar_acao',
  criar_retomar_escolha: () => 'criar_retomar_escolha', criar_sessao_erro: () => 'criar_sessao_erro', criar_usar: () => 'criar_usar',
  criar_verificando: () => 'criar_verificando', criar_sessao: () => 'criar_sessao', sessao_nova: () => 'sessao_nova',
  switcher_atual: () => 'switcher_atual',
}));

import { CreateSessionSheet } from './CreateSessionSheet';
import { codexPreparationMessage } from '@hangar/core';

const connected = { id: 'work', credential_id: 'codex:/work', name: 'Trabalho', home: '/work', is_default: false, auth: { method: 'oauth', status: 'connected', email: 'work@example.com', plan: 'Plus' }, sync: { status: 'ready', trust_pending: false, issues: [] } };
const defaultAccount = { id: 'default', credential_id: 'codex:/default', name: 'Padrão', home: '/default', is_default: true, auth: { method: 'oauth', status: 'connected', email: 'default@example.com', plan: 'Plus' }, sync: { status: 'ready', trust_pending: false, issues: [] } };
const archived = { project: '-repo', cwd: '/repo', session_id: 'sid-1', mtime: 1, preview: 'continuação', ultima: 'última mensagem', live: false, config_dir: null, conta: 'Trabalho', provider: 'codex', codex_account: 'work', codex_home: '/work' };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

describe('CreateSessionSheet Codex', () => {
  afterEach(() => vi.useRealTimers());
  beforeEach(() => {
    calls.accounts.mockReset().mockResolvedValue([connected]);
    calls.models.mockReset().mockResolvedValue({ models: [], reduced: false });
    calls.prepare.mockReset().mockResolvedValue({ status: 'ready', trust_pending: false, issues: [] });
    calls.preparation.mockReset();
    calls.create.mockReset().mockResolvedValue({ name: 'nova', state: 'idle' });
    calls.archives.mockReset().mockResolvedValue([]);
    calls.resume.mockReset().mockResolvedValue({ name: 'retomada', state: 'idle' });
    calls.replace.mockReset();
    calls.alert.mockReset();
    localStorage.clear();
  });

  async function renderSheet(strict = false) {
    const container = document.createElement('div');
    const root = createRoot(container);
    await act(async () => root.render(strict ? createElement(StrictMode, null, createElement(CreateSessionSheet)) : createElement(CreateSessionSheet)));
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'Pasta')!.click());
    await act(async () => Promise.resolve());
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'Codex')!.click());
    await act(async () => Promise.resolve());
    return { container, root };
  }

  it('seleciona conta do servidor e envia a conta no create', async () => {
    const { container, root } = await renderSheet();
    await act(async () => Promise.resolve());
    const create = [...container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('sessao_nova'));
    expect(create).toBeTruthy();
    await act(async () => create!.click());
    expect(calls.prepare).toHaveBeenCalledWith(server, 'work');
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider: 'codex', codex_account: 'work' }));
    root.unmount();
  });

  it.each(['create', 'resume'] as const)('mostra etapas do preparo e abertura durante %s', async (operation) => {
    vi.useFakeTimers();
    calls.archives.mockResolvedValue(operation === 'resume' ? [archived] : []);
    calls.prepare.mockResolvedValue({ status: 'running', etapa: 'principal' });
    calls.preparation
      .mockResolvedValueOnce({ status: 'running', etapa: 'plugins' })
      .mockResolvedValueOnce({ status: 'ready', issues: [], trust_pending: false });
    const pending = deferred<{ name: string; state: 'idle' }>();
    calls[operation].mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    if (operation === 'resume') {
      await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'última mensagem')!.click());
    }
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === (operation === 'create' ? 'sessao_nova' : 'criar_retomar_acao'))!.click());
    expect(container.textContent).toContain(codexPreparationMessage('principal'));
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(container.textContent).toContain(codexPreparationMessage('plugins'));
    expect(calls[operation]).not.toHaveBeenCalled();
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    expect(container.textContent).toContain('codex_ui_abrindo_sessao');
    await act(async () => pending.resolve({ name: 'nova', state: 'idle' }));
    expect(container.textContent).not.toContain('codex_ui_abrindo_sessao');
    root.unmount();
  });

  it.each(['provider', 'unmount'])('descarta atualização do polling após %s', async (change) => {
    vi.useFakeTimers();
    calls.prepare.mockResolvedValue({ status: 'running', etapa: 'principal' });
    const pending = deferred<{ status: 'running'; etapa: string }>();
    calls.preparation.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'sessao_nova')!.click());
    await act(async () => vi.advanceTimersByTimeAsync(1000));
    if (change === 'provider') {
      await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'Claude')!.click());
    } else {
      root.unmount();
    }
    await act(async () => pending.resolve({ status: 'running', etapa: 'plugins' }));
    await act(async () => vi.advanceTimersByTimeAsync(2000));
    expect(container.textContent).not.toContain(codexPreparationMessage('plugins'));
    expect(calls.preparation).toHaveBeenCalledTimes(1);
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.replace).not.toHaveBeenCalled();
    if (change === 'provider') root.unmount();
  });

  it.each(['partial', 'error'])('abre a conta escolhida mesmo com sincronização %s', async (status) => {
    calls.prepare.mockResolvedValue({ status, trust_pending: false, issues: [] });
    const { container, root } = await renderSheet();
    const create = [...container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('sessao_nova'))!;
    await act(async () => create.click());
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ codex_account: 'work' }));
    expect(calls.replace).toHaveBeenCalledWith('/s/server-b/nova');
    expect(calls.alert).toHaveBeenCalledWith('codex_ui_prepare_error');
    root.unmount();
  });

  it('cria normalmente sob StrictMode após o ciclo de efeitos', async () => {
    const { container, root } = await renderSheet(true);
    const create = [...container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('sessao_nova'))!;
    await act(async () => create.click());
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ provider: 'codex', codex_account: 'work' }));
    expect(calls.replace).toHaveBeenCalled();
    root.unmount();
  });

  it('envia esforço Codex do modelo escolhido e limpa esforço incompatível', async () => {
    calls.models.mockResolvedValue({ models: [
      { id: 'sol', name: 'Sol', efforts: ['low', 'high'] },
      { id: 'mini', name: 'Mini', efforts: ['low'] },
    ], reduced: false });
    const { container, root } = await renderSheet();
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'Sol')!.click());
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'high')!.click());
    const firstCreate = [...container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('sessao_nova'))!;
    await act(async () => firstCreate.click());
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ model: 'sol', effort: 'high', codex_account: 'work' }));
    calls.create.mockClear();
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'Mini')!.click());
    const create = [...container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('sessao_nova'))!;
    await act(async () => create.click());
    expect(calls.create).toHaveBeenCalledWith(server, expect.objectContaining({ model: 'mini', effort: null, codex_account: 'work' }));
    root.unmount();
  });

  it('retoma conversa Codex com a identidade do ArchiveEntry', async () => {
    calls.archives.mockResolvedValue([archived]);
    const { container, root } = await renderSheet();
    await act(async () => Promise.resolve());
    expect(container.textContent).toContain('última mensagem');
    const option = [...container.querySelectorAll('button')].find((button) => button.textContent === 'última mensagem');
    expect(option).toBeTruthy();
    await act(async () => option!.click());
    const resume = [...container.querySelectorAll('button')].find((button) => button.textContent === 'criar_retomar_acao');
    expect(resume).toBeTruthy();
    await act(async () => resume!.click());
    expect(calls.resume).toHaveBeenCalledWith('-repo', 'sid-1', null, null, 'codex', 'work', server);
    expect(calls.create).not.toHaveBeenCalled();
    root.unmount();
  });

  it('não cria nem navega se desmontar enquanto prepara uma sessão', async () => {
    const pending = deferred<{ status: 'ready'; issues: never[]; trust_pending: false }>();
    calls.prepare.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    const create = [...container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('sessao_nova'))!;
    await act(async () => create.click());
    root.unmount();
    await act(async () => pending.resolve({ status: 'ready', issues: [], trust_pending: false }));
    expect(calls.create).not.toHaveBeenCalled();
    expect(calls.replace).not.toHaveBeenCalled();
  });

  it('não navega se desmontar depois que o create fica pendente', async () => {
    const pending = deferred<{ name: string; state: 'idle' }>();
    calls.create.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    const create = [...container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('sessao_nova'))!;
    await act(async () => create.click());
    await act(async () => Promise.resolve());
    root.unmount();
    await act(async () => pending.resolve({ name: 'nova', state: 'idle' }));
    expect(calls.replace).not.toHaveBeenCalled();
  });

  it('libera a retomada ao trocar de conta e descarta a operação antiga', async () => {
    calls.accounts.mockResolvedValue([connected, defaultAccount]);
    calls.archives.mockResolvedValue([archived]);
    const pending = deferred<{ status: 'ready'; issues: never[]; trust_pending: false }>();
    calls.prepare.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'Trabalho')!.click());
    await act(async () => Promise.resolve());
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'última mensagem')!.click());
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'criar_retomar_acao')!.click());
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'Padrão')!.click());
    const create = [...container.querySelectorAll('button')].find((button) => button.textContent?.startsWith('sessao_nova'))!;
    expect(create.disabled).toBe(false);
    await act(async () => pending.resolve({ status: 'ready', issues: [], trust_pending: false }));
    expect(calls.resume).not.toHaveBeenCalled();
    root.unmount();
  });

  it('não retoma se desmontar enquanto prepara uma conversa', async () => {
    calls.archives.mockResolvedValue([archived]);
    const pending = deferred<{ status: 'ready'; issues: never[]; trust_pending: false }>();
    calls.prepare.mockReturnValue(pending.promise);
    const { container, root } = await renderSheet();
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'última mensagem')!.click());
    await act(async () => [...container.querySelectorAll('button')].find((button) => button.textContent === 'criar_retomar_acao')!.click());
    root.unmount();
    await act(async () => pending.resolve({ status: 'ready', issues: [], trust_pending: false }));
    expect(calls.resume).not.toHaveBeenCalled();
  });
});
