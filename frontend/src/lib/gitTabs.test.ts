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

  it('a aba ativa é identificada por ID, não por índice', () => {
    // O ponto do teste: a seleção guarda o ID. Se guardasse índice, mexer na ordem de GIT_TABS
    // trocaria a aba ativa debaixo do usuário (mesma classe de bug do plan_name no _list_sig).
    // Exercita de verdade resolvendo a aba contra a lista invertida.

    // Guarda um nav com 'branches' selecionado
    const n = selectTab(initialNav(), 'branches');
    const branchesIndexInOriginal = GIT_TABS.findIndex(t => t.id === n.tab);

    // Simula uma mudança de ordem das abas (local, não muta GIT_TABS)
    const inverseOrder = [...GIT_TABS].reverse();

    // A resolução por ID continua achando a mesma aba em qualquer ordem
    const resolvedById = inverseOrder.find(t => t.id === n.tab);
    expect(resolvedById?.id).toBe('branches');
    expect(resolvedById?.label).toBe('Branches');

    // Contraste: resolver por índice quebraria
    // branches estava no índice 2 em GIT_TABS; na ordem invertida, índice 2 aponta para 'changes'
    const wrongResolution = inverseOrder[branchesIndexInOriginal];
    expect(wrongResolution?.id).not.toBe('branches');
    expect(wrongResolution?.id).toBe('changes');
  });

  it('não muta a entrada', () => {
    const a = initialNav();
    const b = pushLevel(a);
    expect(currentLevel(a)).toBe(0);
    expect(b).not.toBe(a);
  });
});
