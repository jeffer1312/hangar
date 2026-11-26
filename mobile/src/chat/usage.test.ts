import { describe, it, expect, beforeAll } from 'vitest';
import { overwriteGetLocale } from '../paraglide/runtime';
import { agyUsageWindows, composerStatus, fitComposerItems, linhaStats, settingsLabel, spacedModel, usageWindows } from './usage';

// O baseLocale do projeto é `en`; os rótulos esperados aqui são os de pt-BR.
beforeAll(() => overwriteGetLocale(() => 'pt'));

describe('linhaStats', () => {
  it('monta as partes na ordem da PWA e omite o que não veio', () => {
    expect(linhaStats({ turns: 1, steps: 3, in_tok: 1200, out_tok: 300 })).toEqual(['1 turno', '3 chamadas', '↓ 1.2K · ↑ 300 tok']);
    expect(linhaStats({ turns: 2, steps: 1, in_tok: 0, out_tok: 0, tok_s: 41.6, cache_pct: 80 })).toContain('~42 tok/s');
    expect(linhaStats({ turns: 2, steps: 1, in_tok: 0, out_tok: 0, cache_pct: 80 })).toContain('cache 80%');
  });
});

describe('usageWindows', () => {
  it('só lista as janelas que vieram, na ordem 5h, 7d, 30d', () => {
    expect(usageWindows(null)).toEqual([]);
    expect(usageWindows({ raw: '', weeklyPct: 74, fiveHourPct: 23, fiveHourReset: '1h55m' }).map((w) => [w.key, w.pct, w.reset]))
      .toEqual([['5h', 23, '1h55m'], ['7d', 74, undefined]]);
  });
});

describe('agyUsageWindows', () => {
  const cota = { estado: 'lida', janelas: [
    { rotulo: '7d', pct: 1.1, reset_ts: null, por_modelo: false }, { rotulo: '5h', pct: 4.5, reset_ts: null, por_modelo: false },
    { rotulo: 'Claude/GPT 7d', pct: 50, reset_ts: null, por_modelo: true }, { rotulo: 'Claude/GPT 5h', pct: 60, reset_ts: null, por_modelo: true }] };
  it('pega o grupo do modelo da sessão, na ordem 5h, 7d', () => {
    expect(agyUsageWindows(cota, 'gemini-pro-orchestrator-neon').map((w) => [w.key, w.pct])).toEqual([['5h', 4.5], ['7d', 1.1]]);
    expect(agyUsageWindows(cota, 'claude-sonnet-4-6').map((w) => [w.key, w.pct])).toEqual([['5h', 60], ['7d', 50]]);
  });
  it('sem leitura da cota não inventa janela', () => {
    expect(agyUsageWindows({ estado: 'indisponivel', janelas: [] }, 'gemini')).toEqual([]);
    expect(agyUsageWindows(undefined, 'gemini')).toEqual([]);
  });
});

describe('settingsLabel', () => {
  it('junta modelo e nível; sem modelo usa o nome do campo', () => {
    expect(settingsLabel('Opus5.5·1M', 'high')).toBe('Opus 5.5 · 1M · high');
    expect(settingsLabel('Haiku', null)).toBe('Haiku');
    expect(settingsLabel(null, 'high')).toBe('Modelo');
  });
});

describe('spacedModel', () => {
  it('separa nome, versão e janela sem mexer em nome com hífen', () => {
    expect(spacedModel('Opus5.5·1M')).toBe('Opus 5.5 · 1M');
    expect(spacedModel('Haiku4.5')).toBe('Haiku 4.5');
    expect(spacedModel('gpt-5.1-codex')).toBe('gpt-5.1-codex');
    expect(spacedModel('o3')).toBe('o3');
  });
});

describe('fitComposerItems', () => {
  const measured = [
    { key: 'folder', width: 400, shrink: true }, { key: 'branch', width: 350, shrink: true },
    { key: 'diff', width: 62 }, { key: 'time', width: 48 },
    { key: 'mode', width: 240, shrink: true }, { key: 'account', width: 280, shrink: true },
    { key: 'ctx', width: 44 }, { key: '5h', width: 44 }, { key: '7d', width: 44 },
    { key: '30d', width: 52 }, { key: 'cost', width: 44 },
  ];
  const occupied = (widths: Record<string, number>) => Object.values(widths).reduce((sum, w) => sum + w, 0)
    + Math.max(0, Object.keys(widths).length - 1) * 4;

  it.each([375 - 32, 430 - 32])('preserva modo, conta, contexto, três cotas e custo em %s pt medidos', (available) => {
    const widths = fitComposerItems(measured, available);
    expect(widths).toEqual(available === 343
      ? { mode: 47, account: 44, ctx: 44, '5h': 44, '7d': 44, '30d': 52, cost: 44 }
      : { branch: 44, mode: 54, account: 44, ctx: 44, '5h': 44, '7d': 44, '30d': 52, cost: 44 });
    expect(occupied(widths!)).toBeLessThanOrEqual(available);
    expect(Object.values(widths!).every(w => w >= 44)).toBe(true);
  });

  it('fonte maior retira custo e contexto antes das cotas, depois 30d, 7d, 5h, conta e modo', () => {
    const large = measured.map(item => ({ ...item, width: item.width * 2 }));
    const widths = fitComposerItems(large, 398);
    expect(widths).toEqual({ mode: 58, account: 44, '5h': 88, '7d': 88, '30d': 104 });
    expect(occupied(widths!)).toBeLessThanOrEqual(398);
    for (const [available, keys] of [[343, ['mode', 'account', '5h', '7d']], [200, ['mode', 'account', '5h']],
      [120, ['mode', 'account']], [60, ['mode']], [43, []]] as const) {
      const fitted = fitComposerItems(large, available);
      expect(Object.keys(fitted)).toEqual(keys);
      expect(occupied(fitted)).toBeLessThanOrEqual(available);
    }
  });

  it('distribui sobra por modo, conta, branch e pasta sem ultrapassar medidas preferidas', () => {
    const items = ['folder', 'branch', 'account', 'mode'].map(key => ({ key, width: 60, shrink: true }));
    expect(fitComposerItems(items, 220)).toEqual({ folder: 44, branch: 44, account: 60, mode: 60 });
    expect(fitComposerItems(items, 1000)).toEqual({ folder: 60, branch: 60, account: 60, mode: 60 });
    expect(fitComposerItems([], 100)).toEqual({});
    expect(fitComposerItems(items, 0)).toEqual({});
    expect(items.every(item => item.width === 60)).toBe(true);
  });
});

describe('composerStatus', () => {
  it('junta statusline e lista: pasta e branch caem para a sessão, diff zerado some', () => {
    expect(composerStatus(null, null)).toEqual({});
    const s = composerStatus(
      { raw: '', ctxPct: 31, costUsd: 1.5, sessionTime: '1h00', fiveHourPct: 23, weeklyPct: 74 },
      { cwd: '/home/u/hangar', branch: 'fix/mobile', git_added: 256, git_removed: 0 },
    );
    expect(s).toMatchObject({ folder: 'hangar', branch: 'fix/mobile', added: 256, time: '1h00', ctxPct: 31, cost: '$1.50' });
    expect(s.removed).toBeUndefined();
    expect(s.quotas?.map(({ key, pct }) => [key, pct])).toEqual([['5h', 23], ['7d', 74]]);
  });

  it('conserva cotas simultâneas de 5h, 7d e 30d e valores zero sem criar cotas vazias', () => {
    const status = composerStatus({ raw: '', fiveHourPct: 0, weeklyPct: 0, monthlyPct: 0, ctxPct: 0, costUsd: 0 }, null);
    expect(status.quotas?.map(({ key, pct }) => [key, pct])).toEqual([['5h', 0], ['7d', 0], ['30d', 0]]);
    expect(status).toMatchObject({ ctxPct: 0, cost: '$0.00' });
    expect(composerStatus(null, null)).toEqual({});
    expect(composerStatus({ raw: '' }, null)).not.toHaveProperty('quotas');
  });

  it('a statusline vence a lista para pasta e branch', () => {
    expect(composerStatus({ raw: '', repo: 'outro', branch: 'main' }, { cwd: '/x/hangar', branch: 'dev' }))
      .toMatchObject({ folder: 'outro', branch: 'main' });
  });
});
