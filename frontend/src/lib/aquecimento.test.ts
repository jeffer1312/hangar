import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { aoAquecer, segurarAquecimento, soltarAquecimento } from './aquecimento';

const A = 'sessao-a';
const B = 'sessao-b';

/** Resolveu já, ou ainda está esperando?
 *
 * Bandeira + descarga de microtarefas, NÃO `Promise.race` contra um valor pronto: uma promessa já
 * resolvida ainda gasta uma microtarefa pro `.then` rodar, então o valor pronto ganhava a corrida
 * sempre e o helper respondia "ainda esperando" com o portão aberto. */
async function liberado(sessao: string): Promise<boolean> {
  let ok = false;
  void aoAquecer(sessao).then(() => { ok = true; });
  await Promise.resolve();
  await Promise.resolve();
  return ok;
}

describe('aquecimento', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    soltarAquecimento(A);
    soltarAquecimento(B);
    vi.useRealTimers();
  });

  it('sem Chat montado, nada segura — o aquecimento corre na hora', async () => {
    // Quadro/Canvas não têm histórico pra esperar; ali a espera tem que ser um no-op.
    expect(await liberado(A)).toBe(true);
  });

  it('segura enquanto o histórico não chega e solta quando ele chega', async () => {
    segurarAquecimento(A);
    expect(await liberado(A)).toBe(false);
    // Escalonado: acorda depois da folga inicial, um por vez — não no mesmo instante do histórico.
    let ok = false;
    void aoAquecer(A).then(() => { ok = true; });
    soltarAquecimento(A);
    // O `liberado` acima já era o 1º da fila; este é o 2º: folga inicial + um espaço.
    await vi.advanceTimersByTimeAsync(700 + 300 - 1);
    expect(ok).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(ok).toBe(true);
  });

  it('soltar duas vezes não quebra, e quem esperava só é acordado uma vez', async () => {
    segurarAquecimento(A);
    let acordou = 0;
    void aoAquecer(A).then(() => { acordou += 1; });
    soltarAquecimento(A);
    soltarAquecimento(A);
    await vi.advanceTimersByTimeAsync(2000);
    expect(acordou).toBe(1);
  });

  it('o teto solta sozinho quando o histórico nunca resolve', async () => {
    // Rede caída, sessão Kimi antes do 1º prompt: sem isto a pílula de modelo ficaria sem catálogo
    // pra sempre, e uma trava calada é pior que a disputa que este módulo veio resolver.
    segurarAquecimento(A);
    expect(await liberado(A)).toBe(false);
    await vi.advanceTimersByTimeAsync(6000 + 1000);
    expect(await liberado(A)).toBe(true);
  });

  it('os esperadores saem um por vez, com folga, na ordem de chegada', async () => {
    segurarAquecimento(A);
    const ordem: number[] = [];
    void aoAquecer(A).then(() => ordem.push(1));
    void aoAquecer(A).then(() => ordem.push(2));
    void aoAquecer(A).then(() => ordem.push(3));
    soltarAquecimento(A);
    await vi.advanceTimersByTimeAsync(699);
    expect(ordem).toEqual([]);                 // folga pro SSE e o 1º estado passarem antes
    await vi.advanceTimersByTimeAsync(1);
    expect(ordem).toEqual([1]);
    await vi.advanceTimersByTimeAsync(300);
    expect(ordem).toEqual([1, 2]);
    await vi.advanceTimersByTimeAsync(300);
    expect(ordem).toEqual([1, 2, 3]);
    await vi.advanceTimersByTimeAsync(300);    // fila esvaziou: portão sumiu, chat já carregado
    expect(await liberado(A)).toBe(true);
  });

  it('sessão nova segura de novo depois de uma solta', async () => {
    segurarAquecimento(A);
    soltarAquecimento(A);
    await vi.advanceTimersByTimeAsync(2000);
    expect(await liberado(A)).toBe(true);
    segurarAquecimento(A);
    expect(await liberado(A)).toBe(false);
  });

  // ── dois Chat ao mesmo tempo (split do desktop, chat do par) ───────────────────────────────
  // Achado da revisão: com um portão só pro app inteiro, a 2ª sessão trocava a promessa por baixo
  // da 1ª. Estes três casos falham contra aquela versão.

  it('abrir a 2ª sessão não solta nem prende a 1ª', async () => {
    segurarAquecimento(A);
    segurarAquecimento(B);
    expect(await liberado(A)).toBe(false);
    expect(await liberado(B)).toBe(false);
  });

  it('o histórico de uma solta SÓ o portão dela', async () => {
    segurarAquecimento(A);
    segurarAquecimento(B);
    soltarAquecimento(A);
    await vi.advanceTimersByTimeAsync(2000);
    expect(await liberado(A)).toBe(true);
    expect(await liberado(B)).toBe(false);   // B esperava o próprio histórico, não o de A
  });

  it('o teto da 1ª sobrevive à abertura da 2ª', async () => {
    // O bug: `segurarAquecimento` da 2ª cancelava o relógio compartilhado, e a 1ª ficava presa
    // pra sempre — sem lista de `/` e sem catálogo de modelo, sem nada na tela dizendo isso.
    segurarAquecimento(A);
    await vi.advanceTimersByTimeAsync(3000);
    segurarAquecimento(B);
    await vi.advanceTimersByTimeAsync(3100 + 1000);
    expect(await liberado(A)).toBe(true);
    expect(await liberado(B)).toBe(false);   // B abriu depois: o teto dele ainda não venceu
  });
});
