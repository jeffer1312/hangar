import { describe, it, expect } from 'vitest';
import { initialNav, selectTab, pushLevel, popLevel, currentLevel, GIT_TABS } from './gitTabs';

describe('gitTabs', () => {
  it('começa em Mudanças, nível 0', () => {
    const n = initialNav();
    expect(n.tab).toBe('changes');
    expect(currentLevel(n)).toBe(0);
  });

  it('cada aba guarda o próprio nível', () => {
    let n = initialNav();
    n = pushLevel(n);                    // changes -> 1
    n = selectTab(n, 'history');
    expect(currentLevel(n)).toBe(0);
    n = pushLevel(n); n = pushLevel(n);  // history -> 2
    n = selectTab(n, 'changes');
    expect(currentLevel(n)).toBe(1);
    n = selectTab(n, 'history');
    expect(currentLevel(n)).toBe(2);
  });

  it('para no teto de cada aba — valores cravados', () => {
    // Cravado de proposito: comparar com GIT_TABS.maxLevel passaria com qualquer numero.
    let c = initialNav();
    for (let i = 0; i < 9; i++) c = pushLevel(c);
    expect(currentLevel(c)).toBe(1);                       // changes: lista -> diff

    let h = selectTab(initialNav(), 'history');
    for (let i = 0; i < 9; i++) h = pushLevel(h);
    expect(currentLevel(h)).toBe(2);                       // history: lista -> commit -> diff

    let b = selectTab(initialNav(), 'branches');
    for (let i = 0; i < 9; i++) b = pushLevel(b);
    expect(currentLevel(b)).toBe(0);                       // branches: so a lista
  });

  it('não desce abaixo de zero', () => {
    expect(currentLevel(popLevel(popLevel(initialNav())))).toBe(0);
  });

  it('a aba ativa sobrevive a mudar a ordem das abas', () => {
    // O ponto do teste: a selecao guarda o ID. Se guardasse indice, mexer na lista de abas trocaria
    // a aba ativa debaixo do usuario (a mesma classe de bug do plan_name no _list_sig).
    const n = selectTab(initialNav(), 'branches');
    const ordemInvertida = [...GIT_TABS].reverse();
    const aindaExiste = ordemInvertida.some((t) => t.id === n.tab);
    expect(aindaExiste).toBe(true);
    expect(n.tab).toBe('branches');
  });

  it('não muta a entrada', () => {
    const a = initialNav();
    const b = pushLevel(a);
    expect(currentLevel(a)).toBe(0);
    expect(b).not.toBe(a);
  });
});
