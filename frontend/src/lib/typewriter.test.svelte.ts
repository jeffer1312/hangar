import { describe, expect, it } from 'vitest';
import { Typewriter } from './typewriter.svelte';

// Sem rAF no node: os passos vêm de tick() com timestamps de mentira — é a seam pública da classe.

function avanca(tw: Typewriter, ms: number, passo = 40) {
  let t = 1000;
  const fim = 1000 + ms;
  while (t < fim) {
    t += passo;
    tw.tick(t);
  }
}

describe('Typewriter', () => {
  it('revela texto que estende aos poucos, não de uma vez', () => {
    const tw = new Typewriter(false);
    tw.set('Primeiro parágrafo da resposta, com umas palavras a mais pra dar corpo.');
    expect(tw.texto).toBe(''); // nada revelado antes do primeiro passo
    avanca(tw, 120);
    expect(tw.texto.length).toBeGreaterThan(0);
    expect(tw.texto.length).toBeLessThan(tw.alvo.length);
    avanca(tw, 3000);
    expect(tw.texto).toBe(tw.alvo); // alcança o fim
  });

  it('acelera com backlog grande: nunca fica mais de ~1.2s atrás', () => {
    const tw = new Typewriter(false);
    tw.set('x'.repeat(5000)); // chegou num tranco (ex: bolha montada no meio da mensagem)
    avanca(tw, 1400);
    expect(tw.texto.length).toBe(5000); // 5000/1.2 ≈ 4166 chars/s -> 1.4s sobra
  });

  it('texto que NÃO estende faz snap imediato (pane oscilando, mensagem nova)', () => {
    const tw = new Typewriter(false);
    tw.set('mensagem antiga inteira');
    avanca(tw, 2000);
    tw.set('conteúdo novo sem relação');
    expect(tw.texto).toBe('conteúdo novo sem relação'); // sem redigitar
  });

  it('prefers-reduced-motion = snap sempre', () => {
    const tw = new Typewriter(true);
    tw.set('qualquer texto');
    expect(tw.texto).toBe('qualquer texto');
  });

  it('encolher dentro do mesmo prefixo não estoura o índice', () => {
    const tw = new Typewriter(false);
    tw.set('abcdef');
    avanca(tw, 5000);
    tw.set('abc'); // prefixo do revelado -> não estende -> snap pro novo tamanho
    expect(tw.texto).toBe('abc');
  });
});
