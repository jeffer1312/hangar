// Regra de casamento eco<->fila do dedup cruzado (usada pelo Chat.svelte). Se qualquer metade
// voltar a quebrar — o sufixo do eco, a legenda do anexo, o piso de prefixo — estes testes caem.
import { describe, it, expect } from 'vitest';
import { covers } from './covers';

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
