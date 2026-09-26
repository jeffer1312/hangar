import { describe, expect, it } from 'vitest';
import { effortLevels, permissionModes } from './sessionOptions';

describe('opções de abertura', () => {
  it('esforço: lista fixa por provider, a do Codex vem do modelo, o Kimi não tem', () => {
    expect(effortLevels('omp', [], '')).toEqual(effortLevels('pi', [], ''));
    expect(effortLevels('kimi', [], '')).toEqual([]);
    const codex = [{ id: 'gpt-x', efforts: ['low', 'ultra'] }];
    expect(effortLevels('codex', codex, 'gpt-x')).toEqual(['low', 'ultra']);
    expect(effortLevels('codex', codex, '')).toEqual([]);
  });

  it('permissão: Claude sempre, Codex só sem terminal, o resto nunca', () => {
    expect(permissionModes('claude', false)[0]).toBe('plan');
    expect(permissionModes('codex', false)).toEqual([]);
    expect(permissionModes('codex', true)).toContain('Full Access');
    expect(permissionModes('pi', true)).toEqual([]);
  });
});
