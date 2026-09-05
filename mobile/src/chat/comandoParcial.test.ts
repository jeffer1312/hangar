import { describe, it, expect } from 'vitest';
import { comandoParcial } from './comandoParcial';

describe('comandoParcial', () => {
  it('barra sozinha e nome de comando abrem a lista', () => {
    expect(comandoParcial('/')).toBe('');
    expect(comandoParcial('/cle')).toBe('cle');
    expect(comandoParcial('/release-notes')).toBe('release-notes');
  });
  it('caminho colado, argumento e texto comum não abrem', () => {
    expect(comandoParcial('/home/jefferson/x.py')).toBeNull();
    expect(comandoParcial('/model opus')).toBeNull();
    expect(comandoParcial('/clear\n')).toBeNull();
    expect(comandoParcial('oi')).toBeNull();
    expect(comandoParcial('')).toBeNull();
  });
});
