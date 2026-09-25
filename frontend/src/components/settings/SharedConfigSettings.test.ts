// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import SharedConfigSettings from './SharedConfigSettings.svelte';
import { applyConfigSyncForServer, getConfigSyncBundleForServer, getConfigSyncManifestForServer } from '@hangar/core';
import * as m from '../../paraglide/messages';

const state = vi.hoisted(() => {
  const all = [
    { id: 'casa', label: 'Casa', baseUrl: 'http://casa.test', token: 'a' },
    { id: 'vps', label: 'VPS', baseUrl: 'http://vps.test', token: 'b' },
    { id: 'note', label: 'Note', baseUrl: 'http://note.test', token: 'c' },
  ];
  return { all, servers: all };
});

vi.mock('../../lib/auth', () => ({
  listServers: () => state.servers,
  onServersChanged: () => () => {},
}));
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  getConfigSyncManifestForServer: vi.fn(),
  getConfigSyncBundleForServer: vi.fn(),
  applyConfigSyncForServer: vi.fn(),
}));

let component: ReturnType<typeof mount> | null = null;

async function flush() { for (let i = 0; i < 12; i++) await tick(); }

async function render() {
  component = mount(SharedConfigSettings, { target: document.body, props: { server: state.servers[0] } });
  await flush();
}

function button(text: string) {
  return [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === text);
}

async function toggle(label: string) {
  const input = [...document.querySelectorAll('label')]
    .find((l) => l.textContent?.trim() === label)!.querySelector('input')!;
  input.checked = !input.checked;
  input.dispatchEvent(new Event('change', { bubbles: true }));
  await flush();
}

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
  state.servers = state.all;
  vi.clearAllMocks();
});

it('com uma máquina só mostra o estado vazio', async () => {
  state.servers = [state.all[0]];
  await render();
  expect(document.body.textContent).toContain(m.shared_config_need_two());
  expect(button(m.shared_config_send())).toBeUndefined();
});

it('sem destino escolhido os botões ficam desligados', async () => {
  await render();
  expect(button(m.shared_config_send())!.disabled).toBe(true);
  expect(document.body.textContent).toContain(m.shared_config_pick());
});

it('enviar baixa o pacote uma vez na origem e aplica em cada destino', async () => {
  vi.mocked(getConfigSyncBundleForServer).mockResolvedValue(new Blob(['x']));
  vi.mocked(applyConfigSyncForServer).mockResolvedValue({
    items: { claude_env: { status: 'applied', changed: ['JIRA_TOKEN'], warnings: [] } }, backup: '/b',
  });
  await render();
  await toggle(m.shared_config_all_targets());
  button(m.shared_config_send())!.click();
  await flush();
  document.querySelector<HTMLButtonElement>('.btn-confirm')!.click();
  await flush();
  expect(getConfigSyncBundleForServer).toHaveBeenCalledTimes(1);
  expect(vi.mocked(getConfigSyncBundleForServer).mock.calls[0][0].id).toBe('casa');
  expect(vi.mocked(applyConfigSyncForServer).mock.calls.map((c) => c[0].id)).toEqual(['vps', 'note']);
  expect(document.body.textContent).toContain(m.shared_config_status_applied());
  expect(document.body.textContent).toContain('JIRA_TOKEN');
  expect(document.body.textContent).toContain(m.shared_config_backup({ path: '/b' }));
});

it('destino com Hangar antigo pede atualização na comparação', async () => {
  const manifest = { version: 1, machine: '', items: { claude_env: { ok: true, hashes: { A: '1' }, bytes: 0, warnings: [] } } };
  vi.mocked(getConfigSyncManifestForServer).mockImplementation(async (s) => {
    if (s.id === 'vps') throw Object.assign(new Error('404: Not Found'), { status: 404 });
    return manifest;
  });
  await render();
  await toggle('VPS');
  await toggle('Note');
  button(m.shared_config_compare())!.click();
  await flush();
  expect(document.body.textContent).toContain(m.shared_config_outdated({ machine: 'VPS' }));
  expect(document.body.textContent).toContain(
    m.shared_config_diff_line({ added: 0, changed: 0, same: 1, onlyTarget: 0 }));
});

it('trocar a origem apaga a comparação feita com a origem anterior', async () => {
  const manifest = { version: 1, machine: '', items: { claude_env: { ok: true, hashes: { A: '1' }, bytes: 0, warnings: [] } } };
  vi.mocked(getConfigSyncManifestForServer).mockResolvedValue(manifest);
  const line = m.shared_config_diff_line({ added: 0, changed: 0, same: 1, onlyTarget: 0 });
  await render();
  await toggle('VPS');
  button(m.shared_config_compare())!.click();
  await flush();
  expect(document.body.textContent).toContain(line);
  const select = document.querySelector('select')!;
  select.value = 'note';
  select.dispatchEvent(new Event('change', { bubbles: true }));
  await flush();
  expect(document.body.textContent).not.toContain(line);
});
