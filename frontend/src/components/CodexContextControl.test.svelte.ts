// @vitest-environment happy-dom
import { afterEach, expect, it, vi } from 'vitest';
import { mount, tick, unmount } from 'svelte';
import Control from './CodexContextControl.svelte';
import * as m from '../paraglide/messages';

import type { ApplyResult } from '@hangar/core';

// Outras máquinas próprias (só os testes do "Em todas" põem alguma) e as rodadas presas à mão.
const fake = vi.hoisted(() => ({
  servers: [] as { id: string; label: string; baseUrl: string; token: string }[],
  rounds: [] as ((r: ApplyResult[]) => void)[],
  values: [] as boolean[],
}));
vi.mock('../lib/auth', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../lib/auth')>()),
  listServers: () => fake.servers,
  listOwnServers: () => fake.servers,
}));
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  applyCodexContext: (_s: unknown, value: boolean) => { fake.values.push(value); return Promise.resolve(); },
  applyToOwnServers: (servers: unknown[], fn: (s: unknown) => Promise<unknown>) => {
    void fn(servers[0]);
    return new Promise<ApplyResult[]>((resolve) => fake.rounds.push(resolve));
  },
}));

const server = { id: 'a', label: 'A', baseUrl: 'http://a.local', token: 'teste' };
const other = { id: 'b', label: 'B', baseUrl: 'http://b.local', token: 'teste' };
const response = (enabled: boolean) => new Response(JSON.stringify({ contexto_estendido: enabled }));
let component: ReturnType<typeof mount>;
async function flush() { for (let i = 0; i < 20; i++) await tick(); }
const toggle = () => document.querySelector<HTMLInputElement>('[role="switch"]')!;
const applyAll = () => document.querySelector<HTMLInputElement>('.all input');
async function open(desktop = false) {
  const props = $state({ server, busy: false, desktop });
  component = mount(Control, { target: document.body, props });
  await flush();
  return props;
}
afterEach(async () => {
  await unmount(component); vi.restoreAllMocks(); document.body.innerHTML = '';
  fake.servers = []; fake.rounds = []; fake.values = [];
});

it('lê o padrão, muda NA HORA ao tocar e só libera criação depois da confirmação', async () => {
  // O switch desfazia a marca e esperava o servidor: parecia que o clique não tinha pegado
  // (relato de 13/09/2026). Agora mostra o valor escolhido e diz que está salvando.
  let finish: (r: Response) => void = () => {};
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => init?.method === 'POST'
    ? new Promise<Response>(resolve => { finish = resolve; }) : response(true));
  const props = await open();
  expect(toggle().checked).toBe(true);
  toggle().click(); await flush();
  expect(props.busy).toBe(true);
  expect(toggle().disabled).toBe(true);
  expect(toggle().checked).toBe(false);
  expect(document.querySelector('[role="status"]')?.textContent).toContain(m.codex_contexto_salvando());
  expect(fetch).toHaveBeenLastCalledWith('http://a.local/api/harness/codex/opcoes', expect.objectContaining({
    method: 'POST', body: JSON.stringify({ contexto_estendido: false }),
  }));
  finish(response(false)); await flush();
  expect(props.busy).toBe(false);
  expect(toggle().checked).toBe(false);
});

it('falha de gravação mantém o valor confirmado e mostra o erro', async () => {
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => init?.method === 'POST'
    ? new Response(JSON.stringify({ detail: 'Gravação recusada' }), { status: 409 }) : response(false));
  const props = await open();
  toggle().click(); await flush();
  expect(toggle().checked).toBe(false);
  expect(props.busy).toBe(false);
  expect(document.querySelector('[role="alert"]')?.textContent).toContain('Gravação recusada');
});

it('resposta atrasada não troca o padrão de outro servidor', async () => {
  let finish: (r: Response) => void = () => {};
  vi.spyOn(globalThis, 'fetch').mockImplementation(async url => String(url).includes('a.local')
    ? new Promise<Response>(resolve => { finish = resolve; }) : response(false));
  const props = await open();
  props.server = { ...server, id: 'b', baseUrl: 'http://b.local' }; await flush();
  finish(response(true)); await flush();
  expect(toggle().checked).toBe(false);
  expect(props.busy).toBe(false);
});

it('"Em todas as minhas máquinas" só no celular: a vista desktop web não ganha o recurso', async () => {
  fake.servers = [server, other];
  vi.spyOn(globalThis, 'fetch').mockImplementation(async () => response(false));
  await open(true);
  expect(applyAll()).toBeNull();
  expect(document.body.textContent).not.toContain(m.tags_apply_all());
  await unmount(component);
  await open(false);
  expect(applyAll()).not.toBeNull();
});

it('toques rápidos: rodadas em fila, a superada não roda e só a última escreve o resultado', async () => {
  fake.servers = [server, other];
  vi.spyOn(globalThis, 'fetch').mockImplementation(async (_url, init) => init?.method === 'POST'
    ? response(JSON.parse(String(init.body)).contexto_estendido) : response(false));
  await open();
  applyAll()!.click(); await flush();          // rodada 1: false
  toggle().click(); await flush();             // rodada 2: true, espera a 1
  toggle().click(); await flush();             // rodada 3: false, supera a 2
  expect(fake.rounds).toHaveLength(1);
  fake.rounds[0]([{ serverId: 'b', status: 'failed', error: 'velha' }]); await flush();
  // A 2 foi superada antes de começar; a 3 é a única que vai às outras máquinas depois da 1.
  expect(fake.rounds).toHaveLength(2);
  expect(fake.values).toEqual([false, false]);
  expect(document.querySelector('.results')).toBeNull();
  fake.rounds[1]([{ serverId: 'b', status: 'ok' }]); await flush();
  expect(document.querySelector('.results')?.textContent).toContain(m.tags_apply_ok());
  expect(document.body.textContent).not.toContain('velha');
});
