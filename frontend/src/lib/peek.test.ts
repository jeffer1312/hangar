import { describe, it, expect } from 'vitest';
import { peekStep, initialPeek, type PeekMemo } from './peek';

// Roda uma sequência de rotas pela máquina, devolvendo o ativo final e os restores pedidos.
// `activeId` é simulado como o chamador faz: peekStep não mexe no ativo, quem aplica é o App.
function walk(routes: Array<{ name: string; peek?: string | null }>, active: string | null) {
  let memo: PeekMemo = initialPeek;
  const restores: Array<string | null> = [];
  for (const r of routes) {
    const peek = r.peek ?? null;
    const { memo: next, restore } = peekStep(memo, r.name, peek, active);
    memo = next;
    if (restore) { restores.push(restore); active = restore; }   // App: selectServer(restore)
    else if (peek) active = peek;                                 // App: selectServer(routed)
  }
  return { active, restores };
}

describe('espiada do quadro: abrir um card não muda onde você está', () => {
  it('repro do review: A ativo -> card de B -> fecha overlay -> volta pra A', () => {
    // Sem o restore, o ativo ficava em B e "+ nova sessão" nascia na máquina errada.
    const { active, restores } = walk(
      [{ name: 'sessions' }, { name: 'board', peek: 'B' }, { name: 'board' }],
      'A',
    );
    expect(restores).toEqual(['A']);
    expect(active).toBe('A');
  });

  it('sair pra #/, #/costs ou #/archive também restaura (nenhuma carrega serverId)', () => {
    for (const exit of ['sessions', 'costs', 'archive']) {
      const { active } = walk([{ name: 'sessions' }, { name: 'board', peek: 'B' }, { name: exit }], 'A');
      expect(active).toBe('A');
    }
  });

  it('(a) deep-link FRIO não restaura depois — não havia "A" pra voltar', () => {
    // #/board/B/x colado numa aba nova: a 1ª rota real já é a espiada.
    const { active, restores } = walk([{ name: 'board', peek: 'B' }, { name: 'board' }], 'B');
    expect(restores).toEqual([]);
    expect(active).toBe('B');
  });

  it('(a2) boot com loading/login antes não conta como navegação', () => {
    const { restores } = walk(
      [{ name: 'loading' }, { name: 'login' }, { name: 'board', peek: 'B' }, { name: 'board' }],
      'B',
    );
    expect(restores).toEqual([]);
  });

  it('(b) ir do overlay pro chat PROMOVE, não restaura', () => {
    const { active, restores } = walk(
      [{ name: 'sessions' }, { name: 'board', peek: 'B' }, { name: 'chat' }],
      'A',
    );
    expect(restores).toEqual([]);
    expect(active).toBe('B');
  });

  it('(c) idempotente: re-run da mesma rota não recaptura o "de onde vim"', () => {
    const { active, restores } = walk(
      [{ name: 'sessions' }, { name: 'board', peek: 'B' }, { name: 'board', peek: 'B' }, { name: 'board' }],
      'A',
    );
    expect(restores).toEqual(['A']);
    expect(active).toBe('A');
  });

  it('trocar de card B->C direto ainda volta pra A (não pro card anterior)', () => {
    const { active } = walk(
      [{ name: 'sessions' }, { name: 'board', peek: 'B' }, { name: 'board', peek: 'C' }, { name: 'board' }],
      'A',
    );
    expect(active).toBe('A');
  });

  it('espiar o card do servidor JÁ ativo não agenda restore', () => {
    const { active, restores } = walk(
      [{ name: 'sessions' }, { name: 'board', peek: 'A' }, { name: 'board' }],
      'A',
    );
    expect(restores).toEqual([]);
    expect(active).toBe('A');
  });

  it('promovido pro chat e voltando pro card: o card agora É a casa, nada a restaurar', () => {
    const { active, restores } = walk(
      [{ name: 'sessions' }, { name: 'board', peek: 'B' }, { name: 'chat' }, { name: 'board', peek: 'B' }, { name: 'board' }],
      'A',
    );
    expect(restores).toEqual([]);
    expect(active).toBe('B');
  });
});
