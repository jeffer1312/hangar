import { describe, test, expect } from 'vitest';
import { parseStatusLine } from '@hangar/core';
import { statusChips } from './statusChips';

// Linha real desta máquina (sessão Claude da conta principal, 22/08/2026)
const LINHA =
  '🤖 Opus5 (high✦) │ 📁 hangar [mobile-expo*] │ 💬 782k/241 780k/1M │ 💵 $179.59 │ ⚡5h:23% ↺1h55m │ 📅7d:74% ↺ter 19h·3d2h │ 🕐 16:14 ⏱ 19h53m';

describe('statusChips', () => {
  test('linha real: 6 chips com os textos esperados', () => {
    const chips = statusChips(parseStatusLine(LINHA));
    expect(chips.map((c) => c.key)).toEqual(['ctx', 'cost', '5h', '7d', 'repo', 'time']);
    expect(chips.find((c) => c.key === 'ctx')).toMatchObject({ text: '78%', warn: false });
    expect(chips.find((c) => c.key === 'cost')).toMatchObject({ text: '$179.59', warn: false });
    expect(chips.find((c) => c.key === '5h')).toMatchObject({ text: '5h 23%', warn: false });
    // 74 < 80: sem aviso
    expect(chips.find((c) => c.key === '7d')).toMatchObject({ text: '7d 74%', warn: false });
    expect(chips.find((c) => c.key === 'repo')).toMatchObject({
      text: 'hangar [mobile-expo*]',
      warn: false,
    });
    expect(chips.find((c) => c.key === 'time')).toMatchObject({ text: '19h53m', warn: false });
  });

  test('nulo devolve lista vazia', () => {
    expect(statusChips(null)).toEqual([]);
  });

  test('>= 80% vira aviso (ctx, 5h, 7d e 30d)', () => {
    // formato Pi: par do turno + "ctx" rotulado (um par só sem rótulo não é contexto)
    const chips = statusChips(
      parseStatusLine('🤖 m │ 💬 251kin/10kout ctx 900k/1M │ ⚡5h:85% │ 📅7d:90% │ 🗓30d:95%'),
    );
    expect(chips.find((c) => c.key === 'ctx')?.warn).toBe(true);
    expect(chips.find((c) => c.key === '5h')?.warn).toBe(true);
    expect(chips.find((c) => c.key === '7d')?.warn).toBe(true);
    expect(chips.find((c) => c.key === '30d')).toMatchObject({ text: '30d 95%', warn: true });
  });

  test('só desenha o que veio: campo ausente não vira chip', () => {
    const chips = statusChips({ raw: '', branch: 'main' });
    expect(chips.map((c) => c.key)).toEqual(['repo']);
    expect(chips[0].text).toBe('[main]');
  });

  test('branch sem dirty sai sem asterisco', () => {
    const chips = statusChips(parseStatusLine('📁 proj [main]'));
    expect(chips.find((c) => c.key === 'repo')?.text).toBe('proj [main]');
  });
});
