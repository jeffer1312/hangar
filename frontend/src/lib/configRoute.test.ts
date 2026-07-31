// frontend/src/lib/configRoute.test.ts
import { describe, it, expect } from 'vitest';
import { parseConfig, comConfig, TELAS_DE_SERVIDOR } from './configRoute';

describe('parseConfig', () => {
  it('devolve null quando nao ha query', () => {
    expect(parseConfig('#/chat/127/x')).toBeNull();
  });
  it('le tela e servidor', () => {
    expect(parseConfig('#/chat/127/x?config=motores&srv=abc')).toEqual({ tela: 'motores', srv: 'abc' });
  });
  it('srv ausente vira null, nao string vazia', () => {
    expect(parseConfig('#/?config=aparencia')).toEqual({ tela: 'aparencia', srv: null });
  });
  it('tela desconhecida e tratada como painel fechado', () => {
    expect(parseConfig('#/?config=xyz')).toBeNull();
  });
  it('config vazio e tratado como painel fechado', () => {
    expect(parseConfig('#/?config=')).toBeNull();
  });
});

describe('comConfig', () => {
  it('preserva o caminho', () => {
    expect(comConfig('#/board', 'aparencia')).toBe('#/board?config=aparencia');
  });
  it('troca a tela sem duplicar a query', () => {
    expect(comConfig('#/board?config=motores&srv=abc', 'anexos', 'abc'))
      .toBe('#/board?config=anexos&srv=abc');
  });
  it('tela null devolve o caminho limpo', () => {
    expect(comConfig('#/chat/127/x?config=motores&srv=abc', null)).toBe('#/chat/127/x');
  });
  it('hash vazio nao vira string vazia', () => {
    expect(comConfig('', null)).toBe('#/');
  });
  it('ida e volta', () => {
    const h = comConfig('#/canvas', 'notificacoes', 'srv-1');
    expect(parseConfig(h)).toEqual({ tela: 'notificacoes', srv: 'srv-1' });
  });
  it('as telas de servidor sao as que exigem alvo', () => {
    expect(TELAS_DE_SERVIDOR).toEqual(['notificacoes', 'anexos', 'avancado', 'motores']);
  });
});
