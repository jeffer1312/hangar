// @vitest-environment happy-dom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, tick, unmount } from 'svelte';
import * as api from '@hangar/core';
import WorktreeSheet from './WorktreeSheet.svelte';
import * as m from '../paraglide/messages';

const st = { path: '/r/hangar-x', repo: '/r/hangar', exists: true, branch: 'x', base: 'main', merged: false, ahead: 1,
  dirty: 0, ignored: [], sessions: [], closed: 0, size: null, size_pending: true } as unknown as api.WorktreeStatus;

let comp: Record<string, unknown> | null = null;
afterEach(() => {
  if (comp) unmount(comp);
  comp = null;
  document.body.innerHTML = '';
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('releitura do tamanho', () => {
  it('resíduo informa os arquivos remanescentes e confirma preservando a branch', async () => {
    vi.spyOn(api, 'getWorktreeForServer').mockResolvedValue({ ...st, size_pending: false, residual: true,
      residual_files: ['source.rs', 'node_modules/cache'], residual_entries: 2 } as api.WorktreeStatus);
    const remove = vi.spyOn(api, 'deleteWorktreeForServer').mockResolvedValue({ removed: st.path, branch_deleted: false, moved: 0 });
    const alvo = document.body.appendChild(document.createElement('div'));
    comp = mount(WorktreeSheet, { target: alvo, props: { open: true, server: { id: 's' } as never, path: st.path, onClose: () => {} } });
    await tick(); await tick(); flushSync();
    [...document.querySelectorAll('button')].find((b) => b.textContent?.includes(m.worktree_apagar_reticencias()))!.click();
    flushSync();
    expect(document.body.textContent).toContain('source.rs');
    expect(document.body.textContent).toContain('node_modules/cache');
    expect(document.querySelector('input[type=checkbox]')).toBeNull();
    document.querySelector<HTMLButtonElement>('.acoes .apagar')!.click();
    await tick();
    expect(remove).toHaveBeenCalledWith(expect.anything(), expect.objectContaining({ confirm: true, keep_branch: true, delete_branch: false }));
  });
  it('falha aparece no detalhe e não vaza para a confirmação', async () => {
    vi.useFakeTimers();
    vi.spyOn(api, 'getWorktreeForServer').mockResolvedValueOnce(st).mockRejectedValueOnce(new Error('caiu'));
    const alvo = document.body.appendChild(document.createElement('div'));
    comp = mount(WorktreeSheet, { target: alvo, props: { open: true, server: { id: 's' } as never, path: st.path, onClose: () => {} } });
    await vi.advanceTimersByTimeAsync(5000);
    await tick();
    expect(document.body.textContent).toContain(m.worktrees_erro({ motivo: 'caiu' }));
    const apagar = [...document.querySelectorAll('button')].find((b) => b.textContent?.includes(m.worktree_apagar_reticencias()))!;
    apagar.click();
    flushSync();
    expect(document.body.textContent).toContain(m.worktree_apagar_titulo({ nome: 'hangar-x' }));
    expect(document.body.textContent).not.toContain('caiu');
  });
});
