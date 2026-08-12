// @vitest-environment happy-dom
// Bloqueadores 2 e 3 da revisão final (parecer-final-kimi-2f863d0): navegação por teclado.
//
// B2 — o foco nunca entrava na caixa na PRIMEIRA abertura de três dos quatro popovers. A primitiva
// focava `[data-foco]` um frame depois de abrir, mas o conteúdo deles vem de `await`: a busca do
// modelo do Claude e os níveis do Pi só existem depois da resposta. O foco ficava na pill e o Tab
// seguinte ia pra página ATRÁS do portal.
//
// B3 — fechar não devolvia o foco à pill. O elemento focado morre junto com o portal, o foco cai no
// <body> e o Tab recomeça do topo da página. A régua da casa é o Select.svelte, que já faz isso.
//
// Testar aqui, na primitiva, e não em cada popover: o conserto é único e cobre os quatro.
import { describe, it, expect, vi } from 'vitest';
import { mount, unmount, tick, createRawSnippet } from 'svelte';
import Popover from './Popover.svelte';

// happy-dom não roda rAF/MutationObserver na mesma volta: espera algumas de verdade.
async function frames(n = 6): Promise<void> {
  for (let i = 0; i < n; i++) {
    await tick();
    await new Promise((r) => requestAnimationFrame(() => r(null)));
  }
}

function montar(conteudo: string) {
  document.body.innerHTML = '';
  const pill = document.createElement('button');
  pill.textContent = 'pill';
  const alvo = document.createElement('div');
  document.body.append(pill, alvo);
  pill.focus();
  // Props num $state: sem isso `open = false` não chega no componente, e B3 (fechar devolve o
  // foco) é justamente sobre a TRANSIÇÃO da prop.
  const props = $state({
  // O `children` de verdade é um snippet compilado; `createRawSnippet` é a forma suportada de
  // fabricar um em teste. A primitiva só o renderiza — o que ela faz é medir e mandar o foco.
    open: true,
    anchor: pill as HTMLElement | null,
    onClose: vi.fn(),
    ariaLabel: 'teste',
    children: createRawSnippet(() => ({ render: () => `<div>${conteudo}</div>` })),
  });
  const comp = mount(Popover, { target: alvo, props });
  return { comp, pill, props };
}

describe('Popover — foco', () => {
  it('foca o alvo que já existe no primeiro frame', async () => {
    const { comp } = montar('<input data-foco />');
    await frames();
    expect(document.activeElement?.tagName).toBe('INPUT');
    unmount(comp);
  });

  it('B2: foca o alvo que só nasce depois (conteúdo vindo de await)', async () => {
    const { comp, pill } = montar('<p>Carregando…</p>');
    await frames(2);
    expect(document.activeElement).toBe(pill);            // ainda não há o que focar
    const caixa = document.querySelector('.pop')!;
    caixa.insertAdjacentHTML('beforeend', '<input data-foco />');   // a lista chegou
    await frames();
    expect(document.activeElement?.tagName).toBe('INPUT');
    unmount(comp);
  });

  it('B2: não rouba o foco de quem já foi mexer noutra coisa', async () => {
    const { comp } = montar('<p>Carregando…</p>');
    const fora = document.createElement('input');
    document.body.appendChild(fora);
    fora.focus();
    document.querySelector('.pop')!.insertAdjacentHTML('beforeend', '<input data-foco />');
    await frames();
    expect(document.activeElement).toBe(fora);
    unmount(comp);
  });

  it('B3: fechar devolve o foco à pill', async () => {
    const { comp, pill, props } = montar('<input data-foco />');
    await frames();
    expect(document.activeElement).not.toBe(pill);
    props.open = false;
    await frames();
    expect(document.activeElement).toBe(pill);
    unmount(comp);
  });

  it('B3: não devolve o foco se o usuário clicou noutro lugar', async () => {
    const { comp, pill, props } = montar('<input data-foco />');
    await frames();
    const fora = document.createElement('input');
    document.body.appendChild(fora);
    fora.focus();
    props.open = false;
    await frames();
    expect(document.activeElement).toBe(fora);
    expect(document.activeElement).not.toBe(pill);
    unmount(comp);
  });

  it('âncora que sumiu com a caixa aberta fecha a caixa em vez de deixá-la solta', async () => {
    document.body.innerHTML = '';
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const onClose = vi.fn();
    const comp = mount(Popover, {
      target: alvo,
      props: {
        open: true, anchor: null, onClose, ariaLabel: 'teste',
        children: createRawSnippet(() => ({ render: () => '<div></div>' })),
      },
    });
    await frames(2);
    expect(onClose).toHaveBeenCalled();
    unmount(comp);
  });
});
