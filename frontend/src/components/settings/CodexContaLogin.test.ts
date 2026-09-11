// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import Login from './CodexContaLogin.svelte';
import Harness from './CodexContaLogin.harness.svelte';
import * as api from '@hangar/core';
import * as m from '../../paraglide/messages';

vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  createCodexAccountForServer: vi.fn(), prepareCodexAccountForServer: vi.fn(),
  getCodexPreparationForServer: vi.fn(), startCodexAccountLoginForServer: vi.fn(),
  getCodexAccountLoginForServer: vi.fn(), cancelCodexAccountLoginForServer: vi.fn(),
  getCodexAccountsForServer: vi.fn(), getCodexIntegrationForServer: vi.fn(),
  startCodexIntegrationForServer: vi.fn(),
}));
const A = { id: 'A', label: 'A', baseUrl: 'https://a.test', token: 'a' };
const B = { id: 'B', label: 'B', baseUrl: 'https://b.test', token: 'b' };
const waiting = { account_id: 'default', attempt_id: 'attempt-A', status: 'waiting' as const, user_code: 'ABC', verification_url: 'https://auth.openai.com/device' };
const ready = { status: 'ready' as const, trust_pending: false, issues: [] };
const idle = { status: 'idle' as const, trust_pending: false, issues: [] };
const conectada = { method: 'oauth' as const, status: 'connected' as const, email: 'a@x', plan: 'pro' };
const padrao = { id: 'default', credential_id: 'codex:d', name: 'default', home: '~/.codex', is_default: true, auth: conectada, sync: ready, has_settings: true } satisfies api.CodexAccount;
const work = { id: 'work', credential_id: 'codex:w', name: 'work', home: '~/.codex-work', is_default: false, auth: conectada, sync: idle } satisfies api.CodexAccount;
let components: ReturnType<typeof mount>[] = [];
const flush = async () => { for (let i = 0; i < 8; i++) await tick(); };
function button(text: string) { return [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === text)!; }
function setup(props: { accountId?: string; herdar?: boolean } = { accountId: 'default' }) {
  const target = document.body.appendChild(document.createElement('div'));
  const done = vi.fn();
  const component = mount(Login, { target, props: { server: B, oncomplete: done, ...props } });
  components.push(component);
  return { done, component };
}
async function digitar(valor: string) {
  const input = document.querySelector('input')!; input.value = valor; input.dispatchEvent(new Event('input')); await flush();
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(api.getCodexAccountLoginForServer).mockResolvedValue(null);
  vi.mocked(api.prepareCodexAccountForServer).mockResolvedValue(ready);
  vi.mocked(api.startCodexAccountLoginForServer).mockResolvedValue(waiting);
  vi.mocked(api.getCodexAccountsForServer).mockResolvedValue([]);
  vi.mocked(api.getCodexIntegrationForServer).mockResolvedValue({ estado: 'ocioso', ultima_execucao: null });
  vi.mocked(api.startCodexIntegrationForServer).mockResolvedValue({ estado: 'ok', ultima_execucao: 'x' });
});
afterEach(async () => { for (const c of components) await unmount(c); components = []; document.body.innerHTML = ''; });

it('inicia o login direto no servidor B, sem preparar nem criar a padrão; fechar não cancela nem confirma', async () => {
  const { component, done } = setup(); await flush();
  button(m.contas_entrar()).click(); await flush();
  expect(api.startCodexAccountLoginForServer).toHaveBeenCalledWith(B, 'default');
  expect(api.prepareCodexAccountForServer).not.toHaveBeenCalled();
  expect(api.createCodexAccountForServer).not.toHaveBeenCalled();
  expect(document.body.textContent).toContain('ABC');
  expect(document.body.textContent).toContain(m.novacred_codex_passo2());
  await unmount(component); components = [];
  expect(api.cancelCodexAccountLoginForServer).not.toHaveBeenCalled();
  expect(done).not.toHaveBeenCalled();
});

it('cancelar A captura ID e não altera a tentativa B após trocar servidor', async () => {
  vi.mocked(api.getCodexAccountLoginForServer).mockImplementation(async (s) => ({ ...waiting, attempt_id: s.id }));
  let resolve!: (a: api.CodexLoginAttempt) => void;
  vi.mocked(api.cancelCodexAccountLoginForServer).mockReturnValue(new Promise((r) => resolve = r));
  const target = document.body.appendChild(document.createElement('div'));
  components.push(mount(Harness, { target, props: { servers: [A, B], oncomplete: vi.fn() } }));
  await flush(); button(m.codex_ui_cancel_login()).click(); await flush();
  button('switch').click(); await flush();
  resolve({ ...waiting, attempt_id: 'A', status: 'cancelled' }); await flush();
  expect(api.cancelCodexAccountLoginForServer).toHaveBeenCalledExactlyOnceWith(A, 'default', 'A');
  expect(document.body.textContent).not.toContain(m.codex_ui_cancelled());
  expect(document.body.textContent).toContain(m.novacred_codex_aguardando());
});

it('resposta atrasada de A não mostra código nem conclusão em B', async () => {
  let resolve!: (a: api.CodexLoginAttempt) => void;
  vi.mocked(api.getCodexAccountLoginForServer).mockImplementation((s) => s.id === 'A'
    ? new Promise((r) => resolve = r) : Promise.resolve(null));
  const done = vi.fn(), target = document.body.appendChild(document.createElement('div'));
  components.push(mount(Harness, { target, props: { servers: [A, B], oncomplete: done } }));
  await flush(); button('switch').click(); await flush();
  resolve({ ...waiting, status: 'completed' }); await flush();
  expect(done).not.toHaveBeenCalled(); expect(document.body.textContent).not.toContain('ABC');
});

it('o nome digitado vira id da conta; cria e pede o login sem preparar antes', async () => {
  vi.mocked(api.createCodexAccountForServer).mockResolvedValue({ id: 'jefferson-felizardo' } as api.CodexAccount);
  setup({}); await flush();
  await digitar('Jefferson Felizardo');
  expect(document.body.textContent).toContain('~/.codex-jefferson-felizardo');
  button(m.contas_entrar()).click(); await flush();
  expect(api.createCodexAccountForServer).toHaveBeenCalledWith(B, 'jefferson-felizardo');
  expect(api.prepareCodexAccountForServer).not.toHaveBeenCalled();
  expect(api.startCodexAccountLoginForServer).toHaveBeenCalledWith(B, 'jefferson-felizardo');
});

it('conta adicional logada pergunta se herda da padrão; Depois fecha sem preparar', async () => {
  vi.mocked(api.getCodexAccountLoginForServer).mockResolvedValue({ ...waiting, account_id: 'work', status: 'completed' });
  vi.mocked(api.getCodexAccountsForServer).mockResolvedValue([padrao, work]);
  const { done } = setup({ accountId: 'work' }); await flush();
  expect(document.body.textContent).toContain(m.codex_ui_herdar_pergunta());
  expect(done).not.toHaveBeenCalled();
  button(m.codex_ui_depois()).click(); await flush();
  expect(api.prepareCodexAccountForServer).not.toHaveBeenCalled();
  expect(done).toHaveBeenCalledOnce();
});

it('Herdar agora prepara; falha fica na tela até Concluir', async () => {
  vi.mocked(api.getCodexAccountLoginForServer).mockResolvedValue({ ...waiting, account_id: 'work', status: 'completed' });
  vi.mocked(api.getCodexAccountsForServer).mockResolvedValue([padrao, work]);
  vi.mocked(api.prepareCodexAccountForServer).mockResolvedValue({ ...ready, status: 'error', issues: [{ code: 'codex_account_prepare_failed', params: { error: 'X' } }] });
  const { done } = setup({ accountId: 'work' }); await flush();
  button(m.codex_ui_herdar_agora()).click(); await flush();
  expect(api.prepareCodexAccountForServer).toHaveBeenCalledWith(B, 'work');
  expect(document.body.textContent).toContain(m.codex_ui_importar_erro());
  expect(done).not.toHaveBeenCalled();
  button(m.codex_ui_concluir()).click(); await flush();
  expect(done).toHaveBeenCalledOnce();
});

it('padrão vazia ou conta já herdada: fecha sem perguntar', async () => {
  vi.mocked(api.getCodexAccountLoginForServer).mockResolvedValue({ ...waiting, account_id: 'work', status: 'completed' });
  vi.mocked(api.getCodexAccountsForServer).mockResolvedValue([{ ...padrao, has_settings: false }, work]);
  const { done } = setup({ accountId: 'work' }); await flush();
  expect(done).toHaveBeenCalledOnce();
  expect(document.body.textContent).not.toContain(m.codex_ui_herdar_pergunta());
});

it('primeira conta (a padrão) oferece importar do Claude; Importar agora roda e fecha quando ok', async () => {
  vi.mocked(api.getCodexAccountLoginForServer).mockResolvedValue({ ...waiting, status: 'completed' });
  vi.mocked(api.getCodexAccountsForServer).mockResolvedValue([{ ...padrao, has_settings: false }]);
  const { done } = setup(); await flush();
  expect(document.body.textContent).toContain(m.codex_ui_importar_claude_pergunta());
  button(m.codex_ui_importar_agora()).click(); await flush();
  expect(api.startCodexIntegrationForServer).toHaveBeenCalledWith(B);
  expect(done).toHaveBeenCalledOnce();
});

it('padrão que já importou do Claude não pergunta de novo', async () => {
  vi.mocked(api.getCodexAccountLoginForServer).mockResolvedValue({ ...waiting, status: 'completed' });
  vi.mocked(api.getCodexAccountsForServer).mockResolvedValue([padrao]);
  vi.mocked(api.getCodexIntegrationForServer).mockResolvedValue({ estado: 'ok', ultima_execucao: '2026-09-11' });
  const { done } = setup(); await flush();
  expect(done).toHaveBeenCalledOnce();
});

it('herdar pelo card vai direto pro prepare, sem login nem pergunta', async () => {
  const { done } = setup({ accountId: 'work', herdar: true }); await flush();
  expect(api.getCodexAccountLoginForServer).not.toHaveBeenCalled();
  expect(api.prepareCodexAccountForServer).toHaveBeenCalledWith(B, 'work');
  expect(done).toHaveBeenCalledOnce();
});

it('sem conta informada e a padrão sem login, entra na padrão em vez de criar outra', async () => {
  vi.mocked(api.getCodexAccountsForServer).mockResolvedValue([
    { id: 'default', is_default: true, auth: { method: 'none', status: 'disconnected' } } as api.CodexAccount]);
  setup({}); await flush();
  expect(document.querySelector('input')).toBeNull();
  button(m.contas_entrar()).click(); await flush();
  expect(api.createCodexAccountForServer).not.toHaveBeenCalled();
  expect(api.startCodexAccountLoginForServer).toHaveBeenCalledWith(B, 'default');
});

it('recusa link não HTTPS e expõe erro de consulta', async () => {
  vi.mocked(api.getCodexAccountLoginForServer).mockResolvedValue({ ...waiting, verification_url: 'javascript:alert(1)' });
  setup(); await flush(); expect(document.querySelector('a')).toBeNull();
  await unmount(components.pop()!);
  vi.mocked(api.getCodexAccountLoginForServer).mockRejectedValue(new Error(m.codex_ui_login_error()));
  setup(); await flush(); expect(document.body.textContent).toContain(m.codex_ui_login_error());
});

it('consulta de novo ao voltar para a aba durante um login', async () => {
  vi.mocked(api.getCodexAccountLoginForServer).mockResolvedValue(waiting);
  setup(); await flush();
  vi.mocked(api.getCodexAccountLoginForServer).mockClear();

  document.dispatchEvent(new Event('visibilitychange'));
  await flush();

  expect(api.getCodexAccountLoginForServer).toHaveBeenCalledExactlyOnceWith(B, 'default', expect.any(AbortSignal));
});
