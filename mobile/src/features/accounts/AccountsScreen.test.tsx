// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Alert } from 'react-native';
import type { Credencial, EnginesResponse, Motor, Server } from '@hangar/core';
import { configureLocale } from '@hangar/core';
import { overwriteGetLocale } from '../../paraglide/runtime';
import { useServers } from '../../stores/servers';
import Contas from '../../../app/config/contas';

const calls = vi.hoisted(() => ({ accounts: vi.fn(), credentials: vi.fn(), engines: vi.fn(), deleteAccount: vi.fn(), deleteEngine: vi.fn(), consume: vi.fn(),
  claudeStart: vi.fn(), claudeRead: vi.fn(), claudeCancel: vi.fn(), createClaude: vi.fn(), logoutClaude: vi.fn() }));
vi.mock('@hangar/core', async original => ({ ...await original<typeof import('@hangar/core')>(),
  getCodexAccountsForServer: calls.accounts, getCredentialsForServer: calls.credentials, getEnginesForServer: calls.engines,
  deleteCodexAccountForServer: calls.deleteAccount, deleteEngineForServer: calls.deleteEngine, consumeCodexRateLimitResetForServer: calls.consume,
  startClaudeAccountLoginForServer: calls.claudeStart, getClaudeAccountLoginStepForServer: calls.claudeRead,
  cancelClaudeAccountLoginForServer: calls.claudeCancel, criarConta: calls.createClaude, sairConta: calls.logoutClaude,
}));
// A casca da tela depende do fundo nativo e da navegação; as linhas, cabeçalho e ações são reais.
vi.mock('../../ui/Screen', () => ({ Screen: ({ children }: { children: ReactNode }) => createElement('main', null, children) }));
vi.mock('expo-router/react-navigation', () => ({ useHeaderHeight: () => 0 }));
vi.mock('expo-secure-store', () => ({ getItemAsync: async () => null, setItemAsync: async () => {}, deleteItemAsync: async () => {} }));
vi.mock('expo-linking', () => ({ openURL: vi.fn() }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn() }));
// A folha da etiqueta puxa o teclado e a área segura nativos, que não carregam no teste.
vi.mock('../machines/parts', () => ({ MachineSheet: () => null, Notice: () => null }));
vi.mock('../../stores/prefs', () => ({ prefs: { getString: () => undefined, set: () => {}, remove: () => {} } }));
vi.mock('../../ui/Sheet', () => ({ Sheet: ({ children, open }: { children: ReactNode; open?: boolean }) => open === false ? null : createElement('div', { role: 'dialog' }, children) }));
vi.mock('react-native', async original => ({ ...await original<typeof import('react-native')>(),
  TextInput: (props: { accessibilityLabel: string; value: string; editable?: boolean; onChangeText: (value: string) => void }) => createElement('textarea', {
    'aria-label': props.accessibilityLabel, value: props.value, disabled: props.editable === false,
    onInput: (event: { currentTarget: { value: string } }) => props.onChangeText(event.currentTarget.value),
  }),
  Switch: () => null,
}));
Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
overwriteGetLocale(() => 'pt'); configureLocale({ getLocale: () => 'pt' });
const a: Server = { id: 'a', label: 'Máquina A', baseUrl: 'https://a.local', token: 'token-a' };
const b: Server = { id: 'b', label: 'Máquina B', baseUrl: 'https://b.local', token: 'token-b' };
const engine: Motor = { label: 'Provedor', base_url: 'https://provider.example', model: 'test', api_key: '••••1234', api_key_definida: true };
const codex: Credencial = { id: 'codex:/work', tipo: 'codex', codex_account: 'work', auth_method: 'oauth', nome: 'Codex', nome_natural: 'Codex', ativa: false, usos: [],
  cota: { estado: 'lida', janelas: [{ rotulo: '7d', pct: 100 }], reset_credits: { available_count: 1, credits: null } } };
const provider: Credencial = { id: 'chave:custom', tipo: 'chave', auth_method: 'api_key', nome: 'Provedor', nome_natural: 'Provedor', ativa: true, usos: ['claude_code'] };
const roots: Array<ReturnType<typeof createRoot>> = [];
beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset();
  calls.accounts.mockResolvedValue([{ id: 'work', credential_id: 'codex:/work', name: 'Trabalho', home: '/work', is_default: false,
    auth: { method: 'oauth', status: 'connected', email: 'a@example.com', plan: 'Plus' }, sync: { status: 'ready', trust_pending: false, issues: [] } }]);
  calls.credentials.mockResolvedValue([codex, provider]);
  calls.engines.mockResolvedValue({ motores: { custom: engine }, arquivo_corrompido: false, arquivo_caminho: '/home/test/engines.json' });
  calls.deleteAccount.mockResolvedValue({ merged: 0 }); calls.deleteEngine.mockResolvedValue({ ok: true });
  calls.claudeStart.mockResolvedValue({ ok: true }); calls.claudeCancel.mockResolvedValue({ ok: true });
  calls.claudeRead.mockResolvedValue({ etapa: 'aguardando', url: 'https://auth.example/authorize', email: null, plano: null });
  calls.createClaude.mockResolvedValue({ path: '/home/user/.claude-new', label: 'Returned', active: false });
  calls.logoutClaude.mockResolvedValue(undefined);
  useServers.setState({ servers: [a, b], activeId: a.id });
});
afterEach(async () => { for (const root of roots.splice(0)) await act(async () => root.unmount()); vi.restoreAllMocks(); });
const buttons = (container: HTMLElement, text: string) => [...container.querySelectorAll('button')].filter(x => x.getAttribute('aria-label') === text);
async function click(container: HTMLElement, label: string) {
  const target = buttons(container, label)[0]; expect(target, label).toBeTruthy(); await act(async () => target.click());
}
async function show() {
  const container = document.createElement('div'); const root = createRoot(container); roots.push(root);
  await act(async () => root.render(createElement(Contas)));
  return { container, root };
}
async function switchTo(server: Server) { await act(async () => useServers.setState({ servers: [server], activeId: server.id })); }
function lastConfirmation() {
  const dialogs = vi.mocked(Alert.alert).mock.calls;
  return dialogs[dialogs.length - 1][2]!;
}

const claude: Credencial = { id: 'claude:/home/user/.claude-work', tipo: 'claude', auth_method: 'oauth', nome: 'Claude trabalho',
  nome_natural: 'Natural', ativa: false, usos: [], gerenciada: true, login: { estado: 'ok', loggedIn: true } };

async function fill(container: HTMLElement, label: string, value: string) {
  const target = container.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${label}"]`)!;
  expect(target, label).toBeTruthy();
  await act(async () => { target.value = value; target.dispatchEvent(new Event('input', { bubbles: true })); });
}

describe('metadados do plano da conta', () => {
  it.each([
    ['claude', 'max', 'Max'], ['claude', 'pro', 'Pro'], ['claude', 'prolite', 'Prolite'], ['codex', 'Plus', 'Plus'],
  ] as const)('mostra o nome do plano %s/%s sem multiplicador inventado', async (kind, plan, label) => {
    const email = 'plan@example.com';
    calls.credentials.mockResolvedValue(kind === 'claude'
      ? [{ ...claude, login: { estado: 'ok', loggedIn: true, email, plano: plan } }] : [codex]);
    calls.accounts.mockResolvedValue(kind === 'claude' ? [] : [{
      id: 'work', credential_id: 'codex:/work', name: 'Trabalho', home: '/work', is_default: false,
      auth: { method: 'oauth', status: 'connected', email, plan }, sync: { status: 'ready', trust_pending: false, issues: [] },
    }]);
    const view = await show();
    // Email numa linha própria, plano no selo ao lado do nome.
    const spans = [...view.container.querySelectorAll('span')].map(node => node.textContent);
    expect(spans).toContain(email);
    expect(spans).toContain(label);
    expect(spans.join('|')).not.toMatch(/×|maxx|prox/);
  });
});

describe('integração das contas Claude', () => {
  it('abre criação pelo cabeçalho e autentica o nome retornado na mesma máquina, mantendo Codex', async () => {
    calls.credentials.mockResolvedValue([claude, codex, provider]);
    const view = await show();
    await click(view.container, 'Nova conta Claude');
    await fill(view.container, 'Nome da conta nova', 'new-account');
    await click(view.container, 'criar');
    expect(calls.createClaude).toHaveBeenCalledWith(a, 'new-account');
    expect(calls.claudeStart).toHaveBeenCalledWith(a, 'Returned');
    expect(view.container.querySelector('[role="dialog"]')?.textContent).toContain('auth.example');
    expect(buttons(view.container, '+ Nova conta')).toHaveLength(1);
    await switchTo(b);
    expect(view.container.querySelector('[role="dialog"]')).toBeNull();
    expect(calls.claudeStart).not.toHaveBeenCalledWith(b, 'Returned');
    await click(view.container, '+ Nova conta');
    expect(view.container.textContent).toContain('Conta do ChatGPT (Codex)');
  });

  it('a ação Sair da linha usa o nome natural e relê force=true sem duplicar a conta', async () => {
    vi.spyOn(Alert, 'alert');
    calls.credentials.mockResolvedValue([claude, codex]);
    const view = await show();
    await click(view.container, 'Sair');
    expect(calls.logoutClaude).not.toHaveBeenCalled();
    const confirm = lastConfirmation().find(x => x.style === 'destructive')!.onPress!;
    await act(async () => confirm());
    expect(calls.logoutClaude).toHaveBeenCalledWith(a, 'Natural');
    expect(calls.credentials).toHaveBeenLastCalledWith(a, true, expect.any(AbortSignal));
    expect([...view.container.querySelectorAll('span')].filter(x => x.textContent === 'Claude trabalho')).toHaveLength(1);
  });

  it('não desmonta login ao atualizar e mostra falha da releitura dentro da folha', async () => {
    const disconnected = { ...claude, login: { estado: 'ok' as const, loggedIn: false } };
    calls.credentials.mockResolvedValueOnce([disconnected, codex]);
    calls.claudeRead.mockResolvedValue({ etapa: 'concluido', url: null, email: 'user@example.com', plano: 'Max' });
    calls.credentials.mockRejectedValueOnce(new Error('refresh credentials failed'));
    const view = await show();
    await click(view.container, 'Entrar');
    expect(calls.claudeStart).toHaveBeenCalledWith(a, 'Natural');
    expect(calls.credentials).toHaveBeenLastCalledWith(a, true, expect.any(AbortSignal));
    const dialog = view.container.querySelector('[role="dialog"]');
    expect(dialog).toBeTruthy();
    expect(dialog!.textContent).toContain('refresh credentials failed');
    expect(view.container.textContent).toContain('Claude trabalho');
  });
});

describe('contas com motores e cota integrados', () => {
  it.each([['Claude', claude], ['Codex', codex]] as const)('mostra cota ausente para %s com cota null, sem aplicar às outras chaves', async (_kind, account) => {
    calls.credentials.mockResolvedValue([{ ...account, cota: null }, provider]);
    const view = await show();
    expect(view.container.textContent).toContain('Cota ausente');
    expect((view.container.textContent?.match(/Cota ausente/g) ?? []).length).toBe(1);
    expect(view.container.textContent).not.toContain('%');
  });

  it('mantém as identidades existentes e acrescenta cota e edição na mesma tela', async () => {
    const view = await show();
    expect((view.container.textContent?.match(/Trabalho/g) ?? []).length).toBe(1);
    expect([...view.container.querySelectorAll('span')].filter(node => node.textContent === 'Provedor')).toHaveLength(1);
    expect(view.container.textContent).toContain('100%');
    expect(buttons(view.container, 'Novo motor')).toHaveLength(1);
    expect(buttons(view.container, '+ Nova conta')).toHaveLength(1);
    await click(view.container, 'Editar motor');
    expect(view.container.querySelector('textarea[aria-label="Identificador"]')?.getAttribute('disabled')).not.toBeNull();
  });

  it('recarrega credenciais com forcar=true depois do reset definitivo', async () => {
    calls.consume.mockResolvedValue({ outcome: 'reset' });
    const view = await show();
    await click(view.container, 'Usar redefinição guardada'); await click(view.container, 'Confirmar redefinição');
    expect(calls.consume).toHaveBeenCalledWith(a, 'work', null, expect.stringMatching(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/));
    expect(calls.credentials).toHaveBeenLastCalledWith(a, true, expect.any(AbortSignal));
    expect(view.container.textContent).toContain('Cota redefinida');
  });

  it('uma leitura tardia não entra na máquina nova nem com o mesmo id e outro token', async () => {
    let done!: (value: Credencial[]) => void;
    calls.credentials.mockImplementationOnce(() => new Promise(resolve => { done = resolve; }));
    const view = await show();
    const moved = { ...a, label: 'Outra origem', baseUrl: 'https://new.local', token: 'new-token' };
    calls.credentials.mockResolvedValue([]); calls.accounts.mockResolvedValue([]);
    await switchTo(moved);
    await act(async () => done([{ ...codex, nome: 'Leitura antiga' }]));
    expect(view.container.textContent).toContain('Outra origem');
    expect(view.container.textContent).not.toContain('Leitura antiga');
    expect(calls.credentials).toHaveBeenLastCalledWith(moved, false, expect.any(AbortSignal));
  });

  it('confirma apagar conta no alvo capturado e descarta conclusão de outra máquina', async () => {
    vi.spyOn(Alert, 'alert');
    let done!: (value: {}) => void;
    calls.deleteAccount.mockImplementation(() => new Promise(resolve => { done = resolve; }));
    const view = await show(); await click(view.container, 'Remover');
    const confirm = lastConfirmation()[2].onPress!;
    await act(async () => { confirm(); confirm(); });
    expect(calls.deleteAccount).toHaveBeenCalledTimes(1);
    expect(calls.deleteAccount).toHaveBeenCalledWith(a, 'work', true);
    await switchTo(b);
    const reads = calls.credentials.mock.calls.length;
    await act(async () => done({ merged: 0 }));
    expect(calls.credentials).toHaveBeenCalledTimes(reads);
    expect(view.container.textContent).toContain('Máquina B');
    expect(vi.mocked(Alert.alert)).toHaveBeenCalledTimes(1);
  });

  it('um diálogo antigo não começa o DELETE depois da troca de máquina', async () => {
    vi.spyOn(Alert, 'alert');
    const view = await show(); await click(view.container, 'Remover');
    const confirm = lastConfirmation()[2].onPress!;
    await switchTo(b); await act(async () => confirm());
    expect(calls.deleteAccount).not.toHaveBeenCalled();
  });

  it('mostra arquivo corrompido e sua origem sem oferecer criação sobre ele', async () => {
    calls.engines.mockResolvedValue({ motores: {}, arquivo_corrompido: true, arquivo_caminho: '/broken/engines.json' });
    const view = await show();
    expect(view.container.querySelector('[role="alert"]')?.textContent).toContain('/broken/engines.json');
    expect(buttons(view.container, 'Novo motor')).toHaveLength(0);
    expect(view.container.textContent).not.toContain('Nenhum motor configurado');
  });

  it('mantém motor sem credencial após falha inicial da lista', async () => {
    calls.credentials.mockRejectedValueOnce(new Error('credential read failed'));
    const view = await show();
    expect(view.container.textContent).toContain('credential read failed');
    expect([...view.container.querySelectorAll('span')].filter(node => node.textContent === 'Provedor')).toHaveLength(1);
    expect(buttons(view.container, 'Editar motor')).toHaveLength(1);
  });

  it('mantém motor sem credencial durante recarga e deduplica quando a credencial chega', async () => {
    calls.credentials.mockRejectedValueOnce(new Error('credential read failed'));
    const view = await show();
    expect([...view.container.querySelectorAll('span')].filter(node => node.textContent === 'Provedor')).toHaveLength(1);
    let credentialsDone!: (value: Credencial[]) => void;
    let enginesDone!: (value: EnginesResponse) => void;
    calls.credentials.mockImplementationOnce(() => new Promise(resolve => { credentialsDone = resolve; }));
    calls.engines.mockImplementationOnce(() => new Promise(resolve => { enginesDone = resolve; }));
    await click(view.container, 'Atualizar lista e cotas');
    await act(async () => enginesDone({ motores: { custom: engine }, arquivo_corrompido: false, arquivo_caminho: '/home/test/engines.json' }));
    expect([...view.container.querySelectorAll('span')].filter(node => node.textContent === 'Provedor')).toHaveLength(1);
    expect(buttons(view.container, 'Editar motor')).toHaveLength(1);
    await act(async () => credentialsDone([provider]));
    expect([...view.container.querySelectorAll('span')].filter(node => node.textContent === 'Provedor')).toHaveLength(1);
    expect(buttons(view.container, 'Editar motor')).toHaveLength(1);
  });

  it('uma falha de leitura mostra o motivo e a atualização permite recuperar vazio', async () => {
    calls.engines.mockRejectedValueOnce(new Error('engine read refused'));
    const view = await show();
    expect(view.container.textContent).toContain('engine read refused');
    calls.engines.mockResolvedValue({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '/ok/engines.json' });
    await click(view.container, 'Atualizar lista e cotas');
    expect(view.container.textContent).toContain('Nenhum motor configurado');
    expect(buttons(view.container, 'Novo motor')).toHaveLength(1);
  });

  it('apagar motor confirma, trava duplo toque e não publica resposta de outro servidor', async () => {
    vi.spyOn(Alert, 'alert');
    let done!: (value: { ok: boolean }) => void;
    calls.deleteEngine.mockImplementation(() => new Promise(resolve => { done = resolve; }));
    const view = await show(); await click(view.container, 'Apagar motor');
    expect(calls.deleteEngine).not.toHaveBeenCalled();
    const confirm = lastConfirmation().find(x => x.style === 'destructive')!.onPress!;
    await act(async () => { confirm(); confirm(); });
    expect(calls.deleteEngine).toHaveBeenCalledTimes(1);
    expect(calls.deleteEngine).toHaveBeenCalledWith(a, 'custom');
    await switchTo(b); const reads = calls.engines.mock.calls.length;
    await act(async () => done({ ok: true }));
    expect(calls.engines).toHaveBeenCalledTimes(reads);
    expect(view.container.textContent).toContain('Máquina B');
  });

  it.each(['success', 'failure'])('DELETE tardio com %s não altera motor de mesmo nome na máquina B', async outcome => {
    vi.spyOn(Alert, 'alert');
    let done!: (value: { ok: boolean }) => void;
    let fail!: (cause: Error) => void;
    calls.deleteEngine.mockImplementationOnce(() => new Promise((resolve, reject) => { done = resolve; fail = reject; }));
    calls.credentials.mockImplementation((target: Server) => Promise.resolve([{ ...provider, nome: target.id === a.id ? 'motor-a' : 'motor-b' }]));
    const view = await show();
    await click(view.container, 'Apagar motor');
    const confirm = lastConfirmation().find(x => x.style === 'destructive')!.onPress!;
    await act(async () => confirm());
    expect(calls.deleteEngine).toHaveBeenCalledWith(a, 'custom');
    expect(buttons(view.container, 'Apagar motor')[0].disabled).toBe(true);
    await switchTo(b);
    expect(view.container.textContent).toContain('motor-b');
    const before = view.container.textContent;
    const engineReads = calls.engines.mock.calls.length;
    const credentialReads = calls.credentials.mock.calls.length;
    await act(async () => { if (outcome === 'success') done({ ok: true }); else fail(new Error('delete A failed')); });
    expect(view.container.textContent).toBe(before);
    expect(calls.engines).toHaveBeenCalledTimes(engineReads);
    expect(calls.credentials).toHaveBeenCalledTimes(credentialReads);
    expect(buttons(view.container, 'Apagar motor')[0].disabled).toBe(false);
    await click(view.container, 'Apagar motor');
    const confirmB = lastConfirmation().find(x => x.style === 'destructive')!.onPress!;
    await act(async () => confirmB());
    expect(calls.deleteEngine).toHaveBeenLastCalledWith(b, 'custom');
  });

  it('diálogo antigo de motor não envia DELETE após a troca de máquina', async () => {
    vi.spyOn(Alert, 'alert');
    const view = await show(); await click(view.container, 'Apagar motor');
    const confirm = lastConfirmation().find(x => x.style === 'destructive')!.onPress!;
    await switchTo(b);
    const before = view.container.textContent;
    const engineReads = calls.engines.mock.calls.length;
    const credentialReads = calls.credentials.mock.calls.length;
    await act(async () => confirm());
    expect(calls.deleteEngine).not.toHaveBeenCalled();
    expect(view.container.textContent).toBe(before);
    expect(calls.engines).toHaveBeenCalledTimes(engineReads);
    expect(calls.credentials).toHaveBeenCalledTimes(credentialReads);
    expect(buttons(view.container, 'Apagar motor')[0].disabled).toBe(false);
  });

  it('descarta motores recebidos de uma máquina antiga', async () => {
    let done!: (value: EnginesResponse) => void;
    calls.engines.mockImplementationOnce(() => new Promise(resolve => { done = resolve; }));
    const view = await show();
    calls.engines.mockResolvedValue({ motores: {}, arquivo_corrompido: false, arquivo_caminho: '/b/engines.json' });
    await switchTo(b);
    expect(calls.engines).toHaveBeenCalledWith(a);
    await act(async () => done({ motores: { custom: engine }, arquivo_corrompido: false, arquivo_caminho: '/a/engines.json' }));
    expect(buttons(view.container, 'Editar motor')).toHaveLength(0);
    expect(view.container.textContent).toContain('Nenhum motor configurado');
  });
});
