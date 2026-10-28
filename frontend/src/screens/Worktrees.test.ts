// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import * as api from '@hangar/core';
import * as m from '../paraglide/messages';

vi.mock('../lib/auth', () => ({
  listOwnServers: () => [{ id: 's', label: 'maq', baseUrl: 'http://maq', token: 't' }],
  onServersChanged: () => () => {},
}));
vi.mock('../lib/sessionsStore.svelte', () => import('./worktreesTestStore.svelte'));

const { default: Worktrees } = await import('./Worktrees.svelte');

const GB = 1024 ** 3;
const pronta = (nome: string, size: number) => ({
  path: `/r/${nome}`, repo: '/r/hangar', exists: true, branch: nome, base: 'main', merged: true, degraded: false,
  ahead: 0, behind: 1, dirty: 0, dirty_files: [], ignored: [], sessions: [], closed: 0, commits: [],
  size, size_pending: false, size_error: false, size_partial: false,
}) as unknown as api.WorktreeStatus;

let comp: Record<string, unknown> | null = null;
afterEach(() => {
  if (comp) unmount(comp);
  comp = null;
  document.body.innerHTML = '';
  vi.restoreAllMocks();
});

const texto = () => document.body.textContent ?? '';
const botao = (rotulo: string) => [...document.querySelectorAll('button')].find((b) => b.textContent?.includes(rotulo))!;

describe('tela de worktrees', () => {
  it('pasta residual nunca aparece como limpa na lista', async () => {
    const residual = { ...pronta('residual', 12), residual: true, merged: false, degraded: true };
    vi.spyOn(api, 'getWorktreesForServer').mockResolvedValue([{ repo: '/r/hangar', worktrees: [residual] }]);
    vi.spyOn(api, 'fetchWorktreesForServer').mockResolvedValue();
    comp = mount(Worktrees, { target: document.body.appendChild(document.createElement('div')), props: {} });
    await vi.waitFor(() => expect(texto()).toContain(m.worktree_estado_residual()));
    expect(document.querySelector('.metricas')?.textContent).not.toContain(m.worktree_limpa());
  });
  it('carregando diz o que está lendo, e não é uma tela vazia', async () => {
    vi.spyOn(api, 'getWorktreesForServer').mockReturnValue(new Promise(() => {}));
    comp = mount(Worktrees, { target: document.body.appendChild(document.createElement('div')), props: {} });
    flushSync();
    expect(texto()).toContain(m.worktrees_lendo());
  });

  it('segurar o stream das sessões não impede a primeira leitura', async () => {
    const ler = vi.spyOn(api, 'getWorktreesForServer').mockReturnValue(new Promise(() => {}));
    comp = mount(Worktrees, { target: document.body.appendChild(document.createElement('div')), props: {} });
    flushSync();
    expect(ler).toHaveBeenCalledTimes(1);
  });

  it('lote mostra o progresso real de cada uma e só oferece fechar no fim', async () => {
    const a = pronta('a', GB), b = pronta('b', 512);
    vi.spyOn(api, 'getWorktreesForServer').mockResolvedValue([{ repo: '/r/hangar', worktrees: [a, b] }]);
    vi.spyOn(api, 'fetchWorktreesForServer').mockResolvedValue();
    let passo!: (p: api.WorktreeBatchProgress) => void;
    let terminar!: () => void;
    vi.spyOn(api, 'deleteWorktreeBatchForServer').mockImplementation((_s, _r, _c, onProgress) => {
      passo = onProgress;
      return new Promise((ok) => { terminar = () => ok(null as never); });
    });
    comp = mount(Worktrees, { target: document.body.appendChild(document.createElement('div')), props: {} });
    await vi.waitFor(() => expect(document.querySelector('button.lote')).not.toBeNull());
    document.querySelector<HTMLButtonElement>('button.lote')!.click();
    flushSync();
    botao(m.worktree_lote_apagar_n({ n: 2 })).click();
    flushSync();

    const base = { total: 2, failed: [], freed: { bytes: 0, partial: false } };
    passo({ ...base, done: 0, current: a, removed: [] });
    flushSync();
    expect(texto()).toContain(m.worktree_lote_progresso({ feitas: 0, total: 2, tamanho: '0 B' }));
    expect(texto()).toContain(m.worktree_lote_apagando({ nome: 'a' }));
    expect(document.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('0');
    expect(botao(m.worktree_lote_fechar())).toBeUndefined();

    passo({ ...base, done: 1, current: b, removed: [a], freed: { bytes: GB, partial: false } });
    flushSync();
    expect(texto()).toContain(m.worktree_lote_progresso({ feitas: 1, total: 2, tamanho: '1 GB' }));
    expect(document.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('50');

    passo({ ...base, done: 2, current: null, removed: [a], freed: { bytes: GB, partial: false },
            failed: [{ worktree: b, error: '409: trancada' }] });
    terminar();
    await vi.waitFor(() => expect(botao(m.worktree_lote_fechar())).toBeDefined());
    expect(texto()).toContain(m.worktree_lote_progresso({ feitas: 1, total: 2, tamanho: '1 GB' }));
    expect(texto()).toContain(m.worktree_lote_erro_item({ nome: 'b', motivo: '409: trancada' }));
    expect(document.querySelector('[role="progressbar"]')?.getAttribute('aria-valuenow')).toBe('100');
  });
});
