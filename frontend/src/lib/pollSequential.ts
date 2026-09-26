/**
 * Chama `ler` agora e de novo `intervaloMs` depois que a chamada anterior TERMINAR.
 *
 * `setInterval` com fetch empilha pedidos quando o backend responde mais devagar que o intervalo,
 * e a pilha é o que mantém o pool de threads dele saturado. Aqui nunca há mais de um no ar.
 * Devolve a função que para o laço.
 */
export function pollSequential(ler: () => Promise<unknown>, intervaloMs: number): () => void {
  let parado = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const rodada = () => {
    ler().catch(() => {}).finally(() => {
      if (!parado) timer = setTimeout(rodada, intervaloMs);
    });
  };
  rodada();
  return () => { parado = true; clearTimeout(timer); };
}
