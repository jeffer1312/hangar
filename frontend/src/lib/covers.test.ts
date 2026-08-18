// Regra de casamento eco<->fila do dedup cruzado (usada pelo Chat.svelte). Se qualquer metade
// voltar a quebrar — o sufixo do eco, a legenda do anexo, o piso de prefixo — estes testes caem.
import { describe, it, expect } from 'vitest';
import { covers, donoDaLinha } from './covers';

describe('covers — dedup eco<->fila', () => {
  it('casa texto igual', () => {
    expect(covers('Vamos fazer', 'Vamos fazer')).toBe(true);
  });

  it('casa quando a fila e uma linha do real (multi-linha)', () => {
    expect(covers('linha um\nlinha dois', 'linha dois')).toBe(true);
  });

  it('casa pela legenda canonica (msg com anexo)', () => {
    expect(covers('legenda', 'legenda — 📎 imagem: /x.png')).toBe(true);
    expect(covers('legenda — 📎 imagem: /x.png', 'legenda')).toBe(true);
  });

  it('casa quando o real tem sufixo apos o texto da fila (defeito B)', () => {
    // Medido 18/08/2026: a fila digitou "Vamos fazer ate as 23 com o Deepseek...", o transcript
    // gravou a mesma linha com "… eu tinha mandado isso" no fim. Sem o prefixo a bolha ficava
    // marcada "não chegou — reenvie" para sempre sobre uma mensagem que CHEGOU.
    // `a` e SEMPRE o real e `b` a bolha da fila (as duas chamadas do Chat.svelte passam assim):
    // "cobre" e direcional por design, e a fila ser menor que o real e o proprio caso.
    const fila = 'Vamos fazer ate as 23 com o Deepseek dps agnt para e volta amanha';
    const real = fila + '… eu tinha mandado isso';
    expect(covers(real, fila)).toBe(true);   // real chegou, fila sai da tela
  });

  it('nao casa prefixo curto (piso: ok/sim nao confirmam frase alheia)', () => {
    expect(covers('ok, vamos fazer', 'ok')).toBe(false);
    expect(covers('Sim, mas so ate amanha', 'sim')).toBe(false);
  });

  it('nao casa textos diferentes', () => {
    expect(covers('uma coisa', 'outra coisa')).toBe(false);
    expect(covers('Vamos ver amanha', 'Vamos ver hoje')).toBe(false);
  });
});

describe('donoDaLinha — quem a linha do transcript confirma', () => {
  // Medido em 18/08/2026 (parecer G2 rev2, do lado do backend): com X curta e Y longa, a linha
  // do transcript e de Y. Antes, o front removia a bolha de X — a que carrega o aviso "não
  // chegou" — e deixava a de Y pendente: as duas marcas invertidas.
  it('entre dois prefixos, a linha e do MAIS LONGO', () => {
    const x = 'Vamos fazer';
    const y = 'Vamos fazer ate as 23 com o Deepseek';
    const real = y + '… eu tinha mandado isso';
    expect(donoDaLinha(real, [x, y])).toBe(1);
  });

  it('casamento EXATO ganha de prefixo, em qualquer ordem na fila', () => {
    const real = 'pode seguir com a Task 4 agora';
    expect(donoDaLinha(real, ['pode seguir', real])).toBe(1);
    expect(donoDaLinha(real, [real, 'pode seguir'])).toBe(0);
  });

  it('empate total fica com a primeira (a 2a "ok" continua pendente)', () => {
    expect(donoDaLinha('mesma coisa', ['mesma coisa', 'mesma coisa'])).toBe(0);
  });

  it('sem ninguem cobrindo, ninguem sai da tela', () => {
    expect(donoDaLinha('outra coisa', ['Vamos fazer', 'ok'])).toBe(-1);
  });

  it('o piso de prefixo continua valendo dentro da escolha do dono', () => {
    // "ok" nao reivindica linha alheia; a longa, sim.
    expect(donoDaLinha('ok, vamos fazer isso agora', ['ok'])).toBe(-1);
  });
});
