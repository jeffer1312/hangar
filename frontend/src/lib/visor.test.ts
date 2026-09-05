// @vitest-environment happy-dom
import { describe, it, expect, beforeEach } from 'vitest';
import { ligarArrastoPraBaixo } from './visor';

// Arrastar pra baixo pra fechar a foto. A biblioteca só fecha no arrastar pra CIMA (`y < -90` no
// pointermove dela) e ninguém tenta isso — o gesto que o WhatsApp e a Fotos do iPhone ensinaram é
// puxar pra baixo. Aqui estão os dois guardas que impedem o gesto de atrapalhar o resto.

let wrap: HTMLElement;
let fechou: number;
let soltar: () => void;

function arrasta(dy: number, passo = 25) {
  wrap.dispatchEvent(new PointerEvent('pointerdown', { clientY: 0, bubbles: true }));
  for (let d = passo; d <= dy; d += passo) {
    wrap.dispatchEvent(new PointerEvent('pointermove', { clientY: d, bubbles: true }));
  }
  wrap.dispatchEvent(new PointerEvent('pointerup', { clientY: dy, bubbles: true }));
}

beforeEach(() => {
  wrap = document.createElement('div');
  document.body.appendChild(wrap);
  fechou = 0;
  soltar = ligarArrastoPraBaixo(wrap, () => { fechou += 1; });
});

describe('visor — arrastar pra baixo fecha', () => {
  it('passou do limiar: fecha', () => {
    arrasta(140);
    expect(fechou).toBe(1);
  });

  it('arrasto curto NAO fecha', () => {
    // Um deslize de 60px é rolagem/hesitação, não intenção de fechar. Fechar aí tiraria a foto da
    // tela de quem só encostou nela.
    arrasta(60);
    expect(fechou).toBe(0);
  });

  it('com a imagem AMPLIADA nao fecha: ali arrastar e panoramicar', () => {
    wrap.classList.add('bp-zoomed');
    arrasta(200);
    expect(fechou).toBe(0);
  });

  it('fecha UMA vez so, mesmo continuando a arrastar', () => {
    arrasta(400);
    expect(fechou).toBe(1);
  });

  it('arrastar pra CIMA nao usa este caminho (e o da propria lib)', () => {
    wrap.dispatchEvent(new PointerEvent('pointerdown', { clientY: 300, bubbles: true }));
    wrap.dispatchEvent(new PointerEvent('pointermove', { clientY: 100, bubbles: true }));
    expect(fechou).toBe(0);
  });

  it('soltar tira os listeners', () => {
    // O wrap e REUSADO pela instancia unica do visor: listener que fica vira listener duplicado na
    // proxima foto, e o arrasto passaria a fechar com metade do caminho.
    soltar();
    arrasta(200);
    expect(fechou).toBe(0);
  });

  it('sem pointerdown antes, mover nao fecha', () => {
    wrap.dispatchEvent(new PointerEvent('pointermove', { clientY: 500, bubbles: true }));
    expect(fechou).toBe(0);
  });

  it('dois registros no MESMO wrap nao somam: o gesto nao pode ficar mais sensivel', () => {
    // Achado da revisão: `abrirVisor` é assíncrona, e dois toques rápidos em miniaturas diferentes
    // chegam a `bp.open` duas vezes. Com listener duplicado no wrap reusado, um arrasto contaria
    // dois fechamentos — e na prática o visor fecharia com metade do caminho.
    const solta2 = ligarArrastoPraBaixo(wrap, () => { fechou += 1; });
    arrasta(140);
    expect(fechou).toBe(2);   // é ISTO que o trocarArrasto do visor.ts impede lá em cima
    solta2();
  });
});
