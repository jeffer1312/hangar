// @vitest-environment happy-dom
import './TaskEditor.nativeSetup';
import { act, createElement } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, expect, it, vi, type MockInstance } from 'vitest';
import { configureApi, updateScheduledTask, _limparEsfriamentoParaTestes, type ScheduledTask, type ScheduledTaskInput } from '@hangar/core';
import { _resetApiEnvForTests } from '../../../../packages/core/src/apiEnv';
import { useServers } from '../../stores/servers';
import { _resetDiagForTests } from '../../../../packages/core/src/diag';
import { TaskEditor } from './TaskEditor';
import * as m from '../../paraglide/messages';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const server = { id: 's', label: 'server', baseUrl: 'https://example.test', token: 'owner' };
const task: ScheduledTask = { id: 't', revision: 3, name: 'nome-original', prompt: 'pedido-original', enabled: true,
  schedule: { kind: 'weekly', weekdays: [1, 2], time: '09:00' }, target: { kind: 'session', name: 'destino', session_key: 'vida-original' },
  next_run_at: null, last_run_at: null, last_status: 'never', last_error: null, last_session: null };
let root: Root;
let host: HTMLDivElement;
let fetchMock: MockInstance<typeof globalThis.fetch>;
let writes: ScheduledTaskInput[];
let write: (input: ScheduledTaskInput) => Promise<Response>;
let providers: Record<string, { disponivel: boolean; default?: boolean }>;
const originalServers = useServers.getState();
const close = vi.fn();
const reload = vi.fn();
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });

beforeEach(() => {
  _resetApiEnvForTests(); _limparEsfriamentoParaTestes(); _resetDiagForTests();
  useServers.setState({ servers: [server], activeId: server.id });
  configureApi({ getBaseUrl: () => server.baseUrl, getToken: () => server.token, onUnauthorized() {}, origin: null, createEventSource: vi.fn() });
  close.mockReset(); reload.mockReset(); writes = [];
  providers = { codex: { default: true, disponivel: true }, claude: { disponivel: true } };
  write = async (input) => json({ ...task, ...input, revision: 4 });
  fetchMock = vi.spyOn(globalThis, 'fetch').mockImplementation(async (url, init) => {
    const path = new URL(String(url)).pathname;
    expect(new Headers(init?.headers).get('Authorization')).toBe('Bearer owner');
    if (path === '/api/sessions') return json([{ name: 'destino', state: 'idle' }]);
    if (path === '/api/providers') return json(providers);
    if (path === '/api/model-options') return json({ models: [{ id: 'model-default', name: 'modelo-catalogo' }], default: 'model-default' });
    if (path === '/api/session-defaults') return json({ defaults: null });
    if (path === '/api/codex-contas') return json([]);
    if (path === '/api/scheduled-tasks/t' && init?.method === 'PUT') { const input = JSON.parse(String(init.body)); writes.push(input); return write(input); }
    throw new Error(`Rota inesperada: ${String(url)}`);
  });
  host = document.createElement('div'); root = createRoot(host);
});
afterEach(async () => { await act(async () => root.unmount()); vi.restoreAllMocks(); _resetApiEnvForTests(); _limparEsfriamentoParaTestes(); _resetDiagForTests(); useServers.setState(originalServers, true); });
const mount = async (editing = task, clockOffset = 0) => act(async () => root.render(createElement(TaskEditor, { server, timezone: 'America/Sao_Paulo', task: editing, clockOffset,
  onSave: (input) => updateScheduledTask(server, editing.id, editing.revision, input).then(() => undefined), onReload: reload, onClose: close })));
const save = () => [...host.querySelectorAll<HTMLButtonElement>('button')].find((b) => b.textContent === m.scheduled_save())!;

it('mantém o pedido e a identidade do destino após erro de rede e permite tentar de novo', async () => {
  write = async (input) => { if (writes.length === 1) throw new Error('rede indisponível'); return json({ ...task, ...input, revision: 4 }); };
  await mount();
  await act(async () => save().click());
  expect(host.textContent).toContain('rede indisponível');
  expect(close).not.toHaveBeenCalled();
  expect(host.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${m.scheduled_name()}"]`)?.value).toBe('nome-original');
  await act(async () => save().click());
  expect(writes).toHaveLength(2);
  expect(writes[1]).toMatchObject({ name: 'nome-original', prompt: 'pedido-original', revision: 3, target: { name: 'destino', session_key: 'vida-original' } });
  expect(fetchMock.mock.calls.map(([url]) => new URL(String(url)).pathname)).toContain('/api/sessions');
});

it('bloqueia cliques duplicados e mostra a revisão concorrente sem apagar o rascunho', async () => {
  let finish!: (response: Response) => void;
  write = () => new Promise((resolve) => { finish = resolve; });
  await mount();
  await act(async () => { save().click(); save().click(); });
  expect(writes).toHaveLength(1);
  await act(async () => finish(json({ detail: { code: 'scheduled_revision_conflict', msg: 'changed' } }, 409)));
  expect(host.textContent).toContain(m.scheduled_revision_conflict());
  expect(save().disabled).toBe(true);
  expect(host.textContent).toContain(m.scheduled_reload_draft());
  expect(close).not.toHaveBeenCalled();
  expect(writes[0]).toMatchObject({ name: 'nome-original', prompt: 'pedido-original', target: { session_key: 'vida-original' } });
});

it('resolve o provedor padrão da máquina e salva o agente que o formulário mostra', async () => {
  await mount({ ...task, target: { kind: 'new', launch: { cwd: '/projeto' }, worktree: false } });
  expect(host.textContent).toContain('Codex');
  await act(async () => save().click());
  const urls = fetchMock.mock.calls.map(([url]) => String(url));
  expect(urls).toContain('https://example.test/api/model-options?provider=codex');
  expect(urls).toContain('https://example.test/api/session-defaults?provider=codex');
  expect(urls).toContain('https://example.test/api/codex-contas');
  expect(writes).toHaveLength(1);
  expect(writes[0]).toMatchObject({ target: { kind: 'new', launch: { cwd: '/projeto', provider: 'codex' }, worktree: false } });
});

it('sem padrão marcado e com o Claude indisponível escolhe o primeiro agente disponível', async () => {
  providers = { claude: { disponivel: false }, pi: { disponivel: false }, codex: { disponivel: true }, kimi: { disponivel: true } };
  await mount({ ...task, target: { kind: 'new', launch: { cwd: '/projeto' }, worktree: false } });
  expect(host.textContent).toContain('Codex');
  await act(async () => save().click());
  expect(writes[0]).toMatchObject({ target: { kind: 'new', launch: { cwd: '/projeto', provider: 'codex' } } });
});

it('edita o nome de Once encerrado preservando o offset e os segundos originais', async () => {
  const at = '2027-11-01T09:00:37-03:00';
  const original: ScheduledTask = { ...task, schedule: { kind: 'once', at }, last_status: 'sent' };
  await mount(original, Date.parse('2027-11-12T12:00:00Z') - Date.now());
  const name = host.querySelector<HTMLTextAreaElement>(`textarea[aria-label="${m.scheduled_name()}"]`)!;
  await act(async () => { name.value = 'nome-editado'; name.dispatchEvent(new Event('input', { bubbles: true })); });
  await act(async () => save().click());
  expect(writes).toHaveLength(1);
  expect(writes[0]).toMatchObject({ name: 'nome-editado', schedule: { kind: 'once', at }, revision: 3, target: { session_key: 'vida-original' } });
});
