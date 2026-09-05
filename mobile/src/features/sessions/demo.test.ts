import { describe, it, expect } from 'vitest';
import { agruparSessoes } from '@hangar/core';
import { sessoesDemo } from './fixtures';

// A lista mostra o que o servidor mandou, e só. Antes havia uma linha "demo-aguardando" fabricada
// em __DEV__ pra completar os três estados na tela — o que aparecia era uma sessão inexistente.
describe('lista de sessões', () => {
  it('não inventa linha quando nenhuma sessão está aguardando', () => {
    const rows = sessoesDemo().filter((s) => s.state !== 'awaiting_input');
    const grupos = agruparSessoes(rows, 'server');
    const saida = grupos.flatMap((g) => g.sessions);
    expect(saida).toHaveLength(rows.length);
    expect(saida.every((s) => rows.includes(s))).toBe(true);
    expect(saida.some((s) => s.state === 'awaiting_input')).toBe(false);
  });
});
