// Agenda no maximo 1 quadro por vez: com um ja pendente, chamadas novas de agendar() so retornam
// (o quadro que ja vai rodar sempre le o estado ATUAL quando dispara, entao nao ha disparo perdido
// de verdade — quem chama de novo so evita empilhar N quadros pro mesmo callback).
export function rafThrottle(callback: () => void): { agendar: () => void; cancelar: () => void } {
  let rafId: number | null = null;
  return {
    agendar() {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(() => {
        rafId = null;
        callback();
      });
    },
    cancelar() {
      if (rafId !== null) {
        cancelAnimationFrame(rafId);
        rafId = null;
      }
    },
  };
}
