// @vitest-environment happy-dom
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import HarnessOpcoes from './HarnessOpcoes.svelte';
import type { Server } from '../../lib/auth';
import * as m from '../../paraglide/messages';

const A: Server = { id: 'a', label: 'A', baseUrl: 'http://a.local', token: 'token-a' };
const B: Server = { id: 'b', label: 'B', baseUrl: 'http://b.local', token: 'token-b' };
const config = (valor: boolean) => ({ campos: { claude_statusline_update: { valor, definido: true, origem: 'app' } }, somente_leitura: {} });
const resposta = (valor: boolean) => ({ ok: true, status: 200, json: async () => config(valor) }) as Response;
let componentes: ReturnType<typeof mount>[];
async function estabilizar() { for (let i = 0; i < 12; i++) await tick(); }
async function montar(alvo: Server = B) {
  const props = $state({ apiTarget: alvo, nome: 'Claude Code', onClose: vi.fn() });
  const el = document.createElement('div'); document.body.appendChild(el);
  componentes.push(mount(HarnessOpcoes, { target: el, props }));
  await estabilizar();
  return props;
}
const switchBarra = () => document.querySelector<HTMLInputElement>('#cfg-claude_statusline_update')!;
const salvar = () => [...document.querySelectorAll('button')].find((b) => b.textContent === m.ctx_salvar())!;
beforeEach(() => {
  componentes = [];
  localStorage.setItem('cp_servers', JSON.stringify([A, B]));
  localStorage.setItem('cp_active', A.id);
});
afterEach(async () => {
  for (const c of componentes) await unmount(c);
  vi.restoreAllMocks(); localStorage.clear(); document.body.innerHTML = '';
});

it('salva só a preferência no servidor escolhido e a recupera ao reabrir', async () => {
  let valor = true;
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
    if (init?.method === 'POST') valor = JSON.parse(String(init.body)).claude_statusline_update;
    return resposta(valor);
  });
  await montar();
  expect(switchBarra().checked).toBe(true);
  switchBarra().click(); await estabilizar();
  expect(valor).toBe(true);
  salvar().click(); await estabilizar();
  expect(valor).toBe(false);
  expect(fetch).toHaveBeenCalledWith(`${B.baseUrl}/api/config`, expect.objectContaining({
    method: 'POST', body: JSON.stringify({ claude_statusline_update: false }),
  }));
  await unmount(componentes.pop()!);
  await montar();
  expect(switchBarra().checked).toBe(false);
});

it('erro ao salvar preserva o rascunho e permite tentar novamente', async () => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => {
    if (init?.method === 'POST') throw new Error('Falha de conexão');
    return resposta(true);
  });
  await montar(); switchBarra().click(); await estabilizar();
  salvar().click(); await estabilizar();
  expect(document.body.textContent).toContain('Falha de conexão');
  expect(switchBarra().checked).toBe(false);
  expect(salvar().disabled).toBe(false);
});

it('resposta atrasada do servidor anterior não sobrescreve o novo alvo', async () => {
  let terminar!: (r: Response) => void;
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (url) =>
    String(url).startsWith(B.baseUrl) ? new Promise((resolve) => { terminar = resolve; }) : resposta(false));
  const props = await montar();
  props.apiTarget = A; await estabilizar();
  expect(switchBarra().checked).toBe(false);
  terminar(resposta(true)); await estabilizar();
  expect(switchBarra().checked).toBe(false);
});

it('servidor antigo informa indisponibilidade sem oferecer um valor fictício para salvar', async () => {
  vi.spyOn(globalThis, 'fetch').mockResolvedValue({ ok: true, status: 200,
    json: async () => ({ campos: {}, somente_leitura: {} }),
  } as Response);
  await montar();
  expect(document.body.textContent).toContain(m.harness_opcoes_indisponiveis());
  expect(switchBarra()).toBeNull();
  expect(salvar()).toBeUndefined();
});
