import { describe, expect, it } from 'vitest';
import { runWorktreeBatch, worktreeLabel, worktreesSizeTotal, worktreeState, worktreeReady, worktreeStaleDays, type WorktreeStatus } from './api';

describe('worktreeStaleDays', () => {
  const now = 30 * 86400;
  const w = (extra: Partial<WorktreeStatus>) => ({
    path: '/r/wt', repo: '/r', exists: true, branch: 'topic', base: 'main', main_branch: 'main',
    merged: false, ahead: 1, dirty: 0, ignored: [], sessions: [], closed: 0,
    created_at: 20 * 86400, ...extra,
  });

  it('usa o prazo de cada servidor e inclui o dia em que ele vence', () => {
    expect(worktreeStaleDays(w({ inactive_days: 8 }), now)).toBe(10);
    expect(worktreeStaleDays(w({ inactive_days: 30 }), now)).toBeNull();
    expect(worktreeStaleDays(w({ inactive_days: 10 }), now)).toBe(10);
  });

  it('não inventa prazo quando a opção está desligada ou o servidor não a informa', () => {
    expect(worktreeStaleDays(w({ inactive_days: null }), now)).toBeNull();
    expect(worktreeStaleDays(w({}), now)).toBeNull();
  });

  it('usa o commit quando disponível e não marca atividade desconhecida', () => {
    expect(worktreeStaleDays(w({ inactive_days: 8, last_commit: { sha: 'abc', subject: '', at: 29 * 86400 } }), now)).toBeNull();
    expect(worktreeStaleDays(w({ inactive_days: 8, created_at: null }), now)).toBeNull();
  });
});

it('resíduo registrado nunca parece limpo ou mesclado', () => {
  const w = { exists: true, residual: true, sessions: [], dirty: 0, merged: true, branch: 'topic' } as never;
  expect(worktreeState(w)).toBe('residual');
  expect(worktreeReady(w)).toBe(false);
});

describe('worktreeLabel', () => {
  it('usa a pasta da worktree onde o agente está', () => {
    expect(worktreeLabel({ name: 's', worktree: true, worktree_path: '/r/hangar-rust-parte3', cwd: '/r/hangar' } as never))
      .toBe('hangar-rust-parte3');
  });
  it('nada fora de worktree', () => {
    expect(worktreeLabel({ name: 's', cwd: '/r/hangar' } as never)).toBeNull();
  });
});

describe('worktreesSizeTotal', () => {
  const w = (o: object) => ({ size: null, size_pending: false, size_error: false, size_partial: false, ...o }) as never;
  it('soma o medido e é exato quando tudo foi medido', () => {
    expect(worktreesSizeTotal([w({ size: 10 }), w({ size: 5 })])).toEqual({ bytes: 15, partial: false });
  });
  it('pendente, falha ou parcial fazem do total um mínimo', () => {
    for (const o of [{ size_pending: true }, { size_error: true }, { size: 3, size_partial: true }]) {
      expect(worktreesSizeTotal([w({ size: 10 }), w(o)]).partial).toBe(true);
    }
  });
  it('nada medido soma zero', () => {
    expect(worktreesSizeTotal([w({ size_pending: true })]).bytes).toBe(0);
  });
});

describe('runWorktreeBatch', () => {
  const w = (path: string, size: number | null, o: object = {}) =>
    ({ path, size, size_pending: false, size_error: false, size_partial: false, ...o }) as never;

  it('avança uma por vez, soma o espaço das que saíram e não conta falha como apagada', async () => {
    const vistos: string[] = [];
    const fim = await runWorktreeBatch(
      [w('/r/a', 1024 ** 3), w('/r/b', 512), w('/r/c', 2048)],
      async (x: { path: string }) => {
        if (x.path === '/r/b') throw new Error('409: trancada');
        return x.path === '/r/a';
      },
      (p) => vistos.push(`${p.done}/${p.total} ${p.current?.path ?? '-'} ${p.freed.bytes} ${p.removed.length}`),
    );
    expect(vistos).toEqual([
      '0/3 /r/a 0 0',
      `1/3 /r/b ${1024 ** 3} 1`,
      `2/3 /r/c ${1024 ** 3} 1`,
      `3/3 - ${1024 ** 3} 1`,
    ]);
    expect(fim.removed.map((x) => x.path)).toEqual(['/r/a']);
    expect(fim.failed.map((f) => [f.worktree.path, f.error])).toEqual([['/r/b', '409: trancada'], ['/r/c', null]]);
  });

  it('tamanho sem medida deixa o liberado como mínimo', async () => {
    const fim = await runWorktreeBatch([w('/r/a', null, { size_pending: true }), w('/r/b', 10)], async () => true, () => {});
    expect(fim.freed).toEqual({ bytes: 10, partial: true });
  });
});
