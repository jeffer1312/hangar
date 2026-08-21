import { describe, it, expect } from 'vitest';
import { chavesUnicas } from './messageKeys';

describe('chaves do each da lista de mensagens', () => {
  it('sem colisão, devolve os ids crus', () => {
    expect(chavesUnicas(['a', 'b', 'c'])).toEqual(['a', 'b', 'c']);
  });

  // O caso real que derrubou a tela: duas entradas de fila consumidas no mesmo milissegundo com o
  // mesmo texto -> o backend gerou `queued:<ts>:<md5>` idêntico pras duas.
  it('id repetido vira chave distinta, mantendo o primeiro cru', () => {
    const q = 'queued:2026-07-30T23:26:49.370Z:a3242f3c';
    const r = chavesUnicas([q, 'outro', q]);
    expect(r).toEqual([q, 'outro', `${q}#2`]);
    expect(new Set(r).size).toBe(3);
  });

  it('três ou mais repetições continuam distintas', () => {
    expect(chavesUnicas(['x', 'x', 'x'])).toEqual(['x', 'x#2', 'x#3']);
  });

  // Estabilidade: a mesma entrada tem que sair igual sempre, senão o Svelte recria os nós a cada
  // render e o usuário perde scroll e foco.
  it('é determinística — mesma lista, mesmas chaves', () => {
    const entrada = ['a', 'a', 'b', 'a'];
    expect(chavesUnicas(entrada)).toEqual(chavesUnicas(entrada));
  });

  // Um id que JÁ venha com o sufixo não pode colidir com o sufixo que geramos.
  it('não colide com um id que já termina em #2', () => {
    const r = chavesUnicas(['a', 'a', 'a#2']);
    expect(new Set(r).size).toBe(3);
  });
});
