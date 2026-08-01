import { describe, expect, it } from 'vitest';
import envelopes from './__fixtures__/ditado-envelopes.json';
import { novoEstadoVad, passoVad, PISO_ABSOLUTO, SILENCIO_MS } from './vad';

const JANELA_MS = 55;
const arquivos = Object.keys(envelopes as Record<string, number[]>);
const env = (nome: string) => (envelopes as Record<string, number[]>)[nome];

/** Roda a regra e devolve o instante (ms) em que encerrou, ou null. */
function quandoEncerra(niveis: number[]): number | null {
  const estado = novoEstadoVad();
  for (let i = 0; i < niveis.length; i++) {
    if (passoVad(estado, niveis[i], i * JANELA_MS) === 'encerra') return i * JANELA_MS;
  }
  return null;
}

describe('regra de silêncio sobre os ditados reais', () => {
  it.each(arquivos)('encerra no silêncio emendado, e não antes: %s', (nome) => {
    const fala = env(nome);
    // 3,3s de silencio emendados no fim: e o unico jeito de afirmar que a regra DISPARA sobre
    // dinamica de fala real. Sem esta emenda, uma regra que nunca encerra passaria no teste.
    const emenda = fala.length * JANELA_MS;
    const t = quandoEncerra([...fala, ...Array(60).fill(0.002)]);
    expect(t).not.toBeNull();
    // Nao pode ter encerrado durante a fala (as maiores pausas medidas sao de 1,6s)...
    expect(t!).toBeGreaterThanOrEqual(emenda);
    // ...e tem que encerrar logo depois da emenda, nao la na frente.
    expect(t! - emenda).toBeLessThan(SILENCIO_MS + 400);
  });

  it.each(arquivos)('arma acima do piso com fala de verdade: %s', (nome) => {
    const estado = novoEstadoVad();
    const fala = env(nome);
    for (let i = 0; i < 200; i++) passoVad(estado, fala[i], i * JANELA_MS);
    // > PISO_ABSOLUTO, nao "> 0": qualquer amostra nao-nula passa em "> 0" e o teste seria vazio.
    expect(estado.pico).toBeGreaterThan(PISO_ABSOLUTO);
  });
});

describe('regra de silêncio, casos sintéticos', () => {
  const fala = (n: number) => Array(n).fill(0.15);
  const quieto = (n: number) => Array(n).fill(0.002);

  it('encerra após 2s contínuos de silêncio depois da fala', () => {
    const t = quandoEncerra([...fala(60), ...quieto(60)]);
    expect(t).not.toBeNull();
    expect(t! - 60 * JANELA_MS).toBeGreaterThanOrEqual(SILENCIO_MS);
    expect(t! - 60 * JANELA_MS).toBeLessThan(SILENCIO_MS + 400);
  });

  it('pausa de 1,6s no meio da fala NÃO encerra', () => {
    // 1,6s foi a maior pausa real medida nos ditados do usuario.
    expect(quandoEncerra([...fala(40), ...quieto(29), ...fala(40)])).toBeNull();
  });

  it('estouro alto no meio da fala NÃO encerra', () => {
    // Buzina, porta batendo, tosse. Com ataque instantaneo o pico saltaria pra 0,8 e a fala em 0,15
    // ficaria abaixo de 25% dele por ~3,2s — mais que os 2s de corte. A gravacao terminaria no meio
    // da frase E, por ter motivo 'silencio', mandaria meia frase sozinha. E o cenario do carro.
    expect(quandoEncerra([...fala(40), 0.8, ...fala(60)])).toBeNull();
  });

  it('silêncio antes de qualquer fala não encerra', () => {
    // Sem o piso absoluto isto vira ruido contra ruido: a razao oscila e a regra encerraria sozinha
    // antes de a pessoa comecar a falar.
    expect(quandoEncerra(quieto(200))).toBeNull();
  });

  it('ruído alto constante não encerra por silêncio', () => {
    // Carro andando: nivel alto o tempo todo, nunca cai pra 25% do pico. Quem encerra este caso e o
    // teto de tempo (Task 4), nao esta regra.
    expect(quandoEncerra(Array(400).fill(0.2))).toBeNull();
  });
});
