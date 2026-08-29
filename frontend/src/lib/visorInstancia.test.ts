// @vitest-environment happy-dom
import { describe, it, expect, vi } from 'vitest';

// Arquivo separado do visor.test.ts de proposito: aqui a lib e MOCKADA, e la nao — o teste do
// arrasto exercita o modulo de verdade.
let construidos = 0;
vi.mock('bigger-picture/vanilla', () => ({
  default: () => {
    construidos++;
    return { open: () => {}, close: () => {} };
  },
}));
vi.mock('bigger-picture/css', () => ({}));

import { abrirVisor } from './visor';

function midia() {
  // A miniatura precisa ter tamanho: sem ela o `medir` devolve null e o plano B carrega a url num
  // `new Image()`, que no happy-dom nao dispara load nem error — o teste ficava pendurado ate o
  // timeout, sem chegar no que interessa.
  const el = document.createElement('div');
  const img = document.createElement('img');
  Object.defineProperty(img, 'naturalWidth', { value: 800 });
  Object.defineProperty(img, 'naturalHeight', { value: 600 });
  el.appendChild(img);
  document.body.appendChild(el);
  return { url: 'foto.png', nome: 'foto.png', tipo: 'image' as const, element: el };
}

// UM teste so, e sem reset entre os casos de proposito: a instancia e memoizada no MODULO, entao
// dois `it` separados dependeriam da ordem em que rodam — o segundo veria a instancia do primeiro.
describe('visor — a instancia da lib e uma so', () => {
  it('nem dois toques ao mesmo tempo nem aberturas em sequencia montam um segundo visor', async () => {
    // Achado da revisao de 28/08/2026, e defeito introduzido no mesmo commit que tornou o carregar
    // da lib assincrono: com `if (!instancia)` e a atribuicao separados por um `await`, as duas
    // chamadas leem `null` e cada uma monta o seu BiggerPicture no body. O segundo rouba o Esc e o
    // arrasto do primeiro, que fica orfao na tela. Guardar a PROMESSA e o que fecha essa janela.
    await Promise.all([abrirVisor([midia()], 0), abrirVisor([midia()], 0)]);
    expect(construidos).toBe(1);

    // E o memo vale depois: reabrir nao remonta nada.
    await abrirVisor([midia()], 0);
    await abrirVisor([midia()], 0);
    expect(construidos).toBe(1);
  });
});
