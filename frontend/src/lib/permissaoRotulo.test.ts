import { describe, it, expect } from 'vitest';
import { glifoPermissao, rotuloPermissao } from './permissaoRotulo';

describe('permissaoRotulo', () => {
  it('modos que param levam ⏸, os que seguem levam ⏵⏵', () => {
    expect(glifoPermissao('plan')).toBe('⏸');
    expect(glifoPermissao('manual')).toBe('⏸');
    for (const m of ['auto', 'acceptEdits', 'bypassPermissions', 'dontAsk']) expect(glifoPermissao(m)).toBe('⏵⏵');
  });

  it('modo desconhecido sai como o id cru, sem glifo', () => {
    expect(rotuloPermissao('novoModo')).toBe('novoModo');
    expect(glifoPermissao('novoModo')).toBe('');
    expect(rotuloPermissao('bypassPermissions')).not.toBe('bypassPermissions');
  });
});
