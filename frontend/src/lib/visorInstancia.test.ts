// @vitest-environment happy-dom
import { describe, it, expect, vi } from 'vitest';

// Arquivo separado do visor.test.ts de proposito: aqui a lib e MOCKADA, e la nao — o teste do
// arrasto exercita o modulo de verdade.
//
// O alvo e `obterInstancia` DIRETO, e nao o `abrirVisor`. Passar por fora nao prova nada: a
// medicao das midias gasta mais ticks que o import, entao a primeira chamada termina tudo antes
// de a segunda chegar no ponto da corrida, e o teste passa ate com o codigo bugado. Isso foi
// medido em 28/08/2026, restaurando o codigo com a corrida e vendo o teste passar assim mesmo.
let construidos = 0;
vi.mock('bigger-picture/vanilla', () => ({
  default: () => {
    construidos++;
    return { open: () => {}, close: () => {} };
  },
}));
vi.mock('bigger-picture/css', () => ({}));

import { obterInstancia } from './visor';

describe('visor — a instancia da lib e uma so', () => {
  it('duas chamadas ao mesmo tempo montam UM visor, e o memo vale depois', async () => {
    // Defeito introduzido no commit que tornou o carregar da lib assincrono: com `if (!instancia)`
    // e a atribuicao separados por um `await`, as duas chamadas leem `null` e cada uma monta o seu
    // BiggerPicture no body. O segundo rouba o Esc e o arrasto do primeiro, que fica orfao na tela.
    // Guardar a PROMESSA fecha essa janela, porque a atribuicao passa a ser sincrona.
    // A trava e a IDENTIDADE, nao o contador: verificado em 28/08/2026 restaurando a corrida —
    // `a === b` falha na hora, enquanto o contador de construcoes ainda voltava 1 (com o codigo
    // bugado as duas chamadas nao passam as duas pelo mock, e contar viraria falso "passou").
    const [a, b] = await Promise.all([obterInstancia(), obterInstancia()]);
    expect(a).toBe(b);

    // E o memo vale depois: pedir de novo devolve a MESMA instancia, sem remontar.
    expect(await obterInstancia()).toBe(a);
    expect(construidos).toBe(1);
  });
});
