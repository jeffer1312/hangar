import { describe, test, expect } from 'vitest';
import { kindOf, isPermission } from './optionKind';

describe('optionKind', () => {
  test('permission en com always', () => {
    expect(isPermission(['Yes', "Yes, and don't ask again", 'No'])).toBe(true);
    expect(kindOf("Yes, and don't ask again")).toBe('always');
  });
  test('permission pt Sim/Não', () => {
    expect(isPermission(['Sim', 'Não'])).toBe(true);
    expect(kindOf('Sim')).toBe('allow');
    expect(kindOf('Não')).toBe('deny');
  });
  test('não permission genérica', () => {
    expect(isPermission(['opção A', 'opção B'])).toBe(false);
    expect(kindOf('opção A')).toBe('other');
  });
});
