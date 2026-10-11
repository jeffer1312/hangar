import { describe, expect, it } from 'vitest';
import { contextModel, effortLevels, engineModelChoice, hasContext, permissionModes } from './sessionOptions';

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

describe('contextModel', () => {
  it('põe e tira o [1m] sem duplicar', () => {
    expect(contextModel('gpt-5.5', true)).toBe('gpt-5.5[1m]');
    expect(contextModel('gpt-5.5[1m]', true)).toBe('gpt-5.5[1m]');
    expect(contextModel('gpt-5.5[1m]', false)).toBe('gpt-5.5');
    expect(hasContext('gpt-5.5[1m]')).toBe(true);
    expect(hasContext('gpt-5.5')).toBe(false);
  });
});

describe('engineModelChoice', () => {
  const res = (current: string) => ({ kind: 'engine' as const, engine: 'gpt', current: { model: current },
    models: [{ id: 'gpt-5.5', supports_fast: true }, { id: 'gpt-5.6', supports_fast: true }, { id: 'gpt-4o' }] });
  it('troca de modelo mantém o 1M quando o novo aceita', () => {
    expect(engineModelChoice(res('gpt-5.5[1m]'), 'gpt-5.6')).toBe('gpt-5.6[1m]');
    expect(engineModelChoice(res('gpt-5.5[1m]'), 'gpt-4o')).toBe('gpt-4o');
    expect(engineModelChoice(res('gpt-5.5'), 'gpt-5.6')).toBe('gpt-5.6');
  });
});
