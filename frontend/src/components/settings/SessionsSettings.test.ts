// @vitest-environment happy-dom
// Tela Sessões com dublê de servidor: as chamadas de leitura e gravação da base comum são trocadas;
// `autoHideDiverges` e `ownUpdatableServers` continuam os reais.
import { afterEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import { readAutoHideAll, saveAutoHideAll } from '@hangar/core';
import * as m from '../../paraglide/messages';

const state = vi.hoisted(() => {
  const all = [
    { id: 'casa', label: 'Casa', baseUrl: 'http://casa.test', token: 'a' },
    { id: 'vps', label: 'VPS', baseUrl: 'http://vps.test', token: 'b' },
  ];
  return { all, servers: all as unknown[], rows: [] as unknown[] };
});

vi.mock('../../lib/auth', () => ({ listOwnServers: () => state.servers }));
vi.mock('../../lib/sessionsStore.svelte', () => ({ sessionsStore: { get rows() { return state.rows; } } }));
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  readAutoHideAll: vi.fn(),
  saveAutoHideAll: vi.fn(),
}));

import SessionsSettings from './SessionsSettings.svelte';

const on3 = { enabled: true, after_minutes: 180 };
let component: ReturnType<typeof mount> | null = null;
async function flush() { for (let i = 0; i < 12; i++) await tick(); }
async function render() {
  component = mount(SessionsSettings, { target: document.body });
  await flush();
}
const text = () => document.body.textContent ?? '';
const button = (label: string) => [...document.querySelectorAll('button')].find((b) => b.textContent?.trim() === label);
const toggle = () => document.querySelector<HTMLInputElement>('#auto-hide-on')!;

afterEach(() => {
  if (component) unmount(component);
  component = null;
  document.body.innerHTML = '';
  state.servers = state.all;
  state.rows = [];
  vi.clearAllMocks();
});

it('sem máquina própria mostra o vazio e não lê nada', async () => {
  state.servers = [];
  await render();
  expect(text()).toContain(m.lista_nenhum_servidor());
  expect(readAutoHideAll).not.toHaveBeenCalled();
});

it('mostra carregando até as máquinas responderem', async () => {
  vi.mocked(readAutoHideAll).mockReturnValue(new Promise(() => {}));
  await render();
  expect(text()).toContain(m.comum_carregando());
});

it('todas falhando é erro com o motivo de cada uma e tentar de novo relê', async () => {
  vi.mocked(readAutoHideAll).mockResolvedValue([
    { serverId: 'casa', ok: false, error: 'fora do ar' },
    { serverId: 'vps', ok: false, error: m.session_visibility_server_outdated() },
  ]);
  await render();
  expect(text()).toContain('fora do ar');
  expect(text()).toContain(m.session_visibility_server_outdated());
  button(m.busca_tentar_de_novo())!.click();
  await flush();
  expect(readAutoHideAll).toHaveBeenCalledTimes(2);
});

it('iguais em todas: mostra a configuração sem aviso de diferença', async () => {
  vi.mocked(readAutoHideAll).mockResolvedValue([
    { serverId: 'casa', ok: true, config: on3 }, { serverId: 'vps', ok: true, config: on3 },
  ]);
  await render();
  expect(toggle().checked).toBe(true);
  expect(button(m.session_auto_hide_apply_all())).toBeUndefined();
});

it('máquina fora do ar aparece só como erro, não como configuração diferente', async () => {
  vi.mocked(readAutoHideAll).mockResolvedValue([
    { serverId: 'casa', ok: true, config: on3 }, { serverId: 'vps', ok: false, error: 'fora do ar' },
  ]);
  await render();
  expect(text()).toContain('fora do ar');
  expect(text()).not.toContain(m.session_auto_hide_differs({ servers: 'VPS' }));
  expect(button(m.session_auto_hide_apply_all())).toBeUndefined();
});

it('máquina diferente aparece e "Aplicar em todas" grava a da maioria em todas, com resultado por máquina', async () => {
  state.all.push({ id: 'note', label: 'Note', baseUrl: 'http://note.test', token: 'c' });
  try {
    vi.mocked(readAutoHideAll).mockResolvedValue([
      { serverId: 'casa', ok: true, config: on3 },
      { serverId: 'vps', ok: true, config: { enabled: false, after_minutes: 180 } },
      { serverId: 'note', ok: true, config: on3 },
    ]);
    vi.mocked(saveAutoHideAll).mockResolvedValue([
      { serverId: 'casa', ok: true, config: on3 },
      { serverId: 'vps', ok: false, error: 'recusou' },
      { serverId: 'note', ok: true, config: on3 },
    ]);
    await render();
    expect(text()).toContain(m.session_auto_hide_differs({ servers: 'VPS' }));
    button(m.session_auto_hide_apply_all())!.click();
    await flush();
    expect(saveAutoHideAll).toHaveBeenCalledWith(state.all, on3);
    expect(text()).toContain(m.session_auto_hide_saved_on({ server: 'Casa' }));
    expect(text()).toContain(m.session_auto_hide_not_saved_on({ server: 'VPS', error: 'recusou' }));
  } finally {
    state.all.pop();
  }
});

it('ligar grava em todas com o prazo em minutos', async () => {
  vi.mocked(readAutoHideAll).mockResolvedValue([
    { serverId: 'casa', ok: true, config: { enabled: false, after_minutes: 180 } },
    { serverId: 'vps', ok: true, config: { enabled: false, after_minutes: 180 } },
  ]);
  vi.mocked(saveAutoHideAll).mockResolvedValue([
    { serverId: 'casa', ok: true, config: on3 }, { serverId: 'vps', ok: true, config: on3 },
  ]);
  await render();
  expect(toggle().checked).toBe(false);
  toggle().checked = true;
  toggle().dispatchEvent(new Event('change', { bubbles: true }));
  await flush();
  expect(saveAutoHideAll).toHaveBeenCalledWith(state.all, on3);
});

it('"Ver sessões ocultas" abre a seção e vai para a lista', async () => {
  vi.mocked(readAutoHideAll).mockResolvedValue([{ serverId: 'casa', ok: true, config: on3 }, { serverId: 'vps', ok: true, config: on3 }]);
  state.rows = [{ name: 'x', hidden_from_list: true }, { name: 'y' }];
  await render();
  button(m.session_hidden_see({ n: 1 }))!.click();
  expect(localStorage.getItem('cp_hidden_section_open')).toBe('1');
  expect(window.location.hash).toBe('#/');
});
