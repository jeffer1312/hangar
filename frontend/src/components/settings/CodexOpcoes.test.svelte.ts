// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import CodexOpcoes from './CodexOpcoes.svelte';
import * as m from '../../paraglide/messages';

let components: ReturnType<typeof mount>[];
const server = { id: 'b', label: 'B', baseUrl: 'http://b.local', token: 'token-teste' };
const data = (enabled: boolean, voice = false) => ({ contexto_estendido: enabled, codex_voice_beta: voice, contexto_configurado: enabled ? 1000000 : null,
  compactacao: null, modelos: [{ model: 'gpt-6-astra', default: 272000, max: 872000 }] });
async function flush() { for (let i = 0; i < 15; i++) await tick(); }
async function montar() {
  const props = $state({ apiTarget: server, nome: 'Codex', onClose: vi.fn() });
  const el = document.createElement('div'); document.body.appendChild(el);
  components.push(mount(CodexOpcoes, { target: el, props })); await flush(); return props;
}
beforeEach(() => { components = []; });
afterEach(async () => { for (const c of components) await unmount(c); vi.restoreAllMocks(); document.body.innerHTML = ''; });

it('salva no servidor escolhido e recupera a opção ao reabrir', async () => {
  let enabled = false;
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
    if (init?.method === 'POST') enabled = JSON.parse(String(init.body)).contexto_estendido;
    return new Response(JSON.stringify(data(enabled)));
  });
  await montar();
  document.querySelector<HTMLInputElement>('[role="switch"]')!.click(); await flush();
  expect(enabled).toBe(false);
  [...document.querySelectorAll('button')].find(b => b.textContent?.trim() === m.ctx_salvar())!.click(); await flush();
  expect(enabled).toBe(true);
  expect(fetch).toHaveBeenCalledWith('http://b.local/api/harness/codex/opcoes', expect.objectContaining({
    method: 'POST', body: JSON.stringify({ contexto_estendido: true, codex_voice_beta: false }),
  }));
  await unmount(components.pop()!); await montar();
  expect(document.querySelector<HTMLInputElement>('[role="switch"]')!.checked).toBe(true);
});

it('voz beta nasce desligada, salva junto e avisa o chat do mesmo servidor', async () => {
  let voice = false;
  const changed = vi.fn();
  window.addEventListener('hangar:codex-voice-config', changed);
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
    if (init?.method === 'POST') voice = JSON.parse(String(init.body)).codex_voice_beta;
    return new Response(JSON.stringify(data(false, voice)));
  });
  await montar();
  const toggle = document.querySelector<HTMLInputElement>('#codex-voice-beta')!;
  expect(toggle.checked).toBe(false);
  expect(document.body.textContent).toContain(m.comum_beta());
  toggle.click(); await flush();
  [...document.querySelectorAll('button')].find(b => b.textContent?.trim() === m.ctx_salvar())!.click(); await flush();
  expect(voice).toBe(true);
  expect(changed).toHaveBeenCalledOnce();
  expect((changed.mock.calls[0][0] as CustomEvent).detail).toEqual({ serverId: 'b', enabled: true });
  window.removeEventListener('hangar:codex-voice-config', changed);
});

it('resposta antiga não atravessa a troca de servidor', async () => {
  let respond: (r: Response) => void = () => {};
  vi.spyOn(globalThis, 'fetch').mockImplementation(async url => String(url).includes('b.local')
    ? new Promise<Response>(r => { respond = r; }) : new Response(JSON.stringify(data(false))));
  const props = await montar();
  props.apiTarget = { ...server, id: 'c', baseUrl: 'http://c.local' }; await flush();
  respond(new Response(JSON.stringify(data(true)))); await flush();
  expect(document.querySelector<HTMLInputElement>('[role="switch"]')!.checked).toBe(false);
});
