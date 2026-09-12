/** Fila limitada em memória. O destino pertence ao evento; credenciais são consultadas no envio. */
export function criarTransporteDiag(tokenPara: (destino: string) => string | null | undefined) {
  const fila: { destino: string; evento: Record<string, unknown> }[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;
  let enviando: Promise<void> | undefined;

  function agendar(ms: number) {
    if (timer === undefined) timer = setTimeout(() => { timer = undefined; void enviar(); }, ms);
  }

  function enviar(): Promise<void> {
    if (enviando) return enviando;
    clearTimeout(timer); timer = undefined;
    enviando = (async () => {
      const destinos = [...new Set(fila.map((item) => item.destino))];
      await Promise.allSettled(destinos.map(async (destino) => {
        const token = tokenPara(destino);
        if (!token) return;
        const pendentes = fila.filter((item) => item.destino === destino);
        try {
          // O servidor aceita no máximo 50 linhas por POST.
          for (let offset = 0; offset < pendentes.length; offset += 50) {
            const lote = pendentes.slice(offset, offset + 50);
            const body = JSON.stringify({ eventos: lote.map((item) => item.evento) });
            const resposta = await fetch(`${destino}/api/diag`, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
              body,
              signal: AbortSignal.timeout(8000),
              // UTF-8 pode ocupar três bytes por caractere; keepalive tem orçamento pequeno.
              keepalive: body.length <= 16000,
            });
            if (!resposta.ok) return;
            const confirmacao = await resposta.json();
            if (!confirmacao || typeof confirmacao !== 'object'
              || !('gravadas' in confirmacao) || confirmacao.gravadas !== lote.length) return;
            const enviados = new Set(lote);
            for (let i = fila.length - 1; i >= 0; i--) {
              if (enviados.has(fila[i])) fila.splice(i, 1);
            }
          }
        } catch { /* A próxima tentativa preserva o destino original. */ }
      }));
    })().finally(() => {
      enviando = undefined;
      if (fila.length) agendar(10000);
    });
    return enviando;
  }

  return {
    registrar(destino: string, evento: Record<string, unknown>) {
      // ponytail: no máximo 200 eventos nesta execução; persistência só se precisar sobreviver ao app fechado.
      if (fila.length >= 200) fila.shift();
      fila.push({ destino, evento });
      agendar(4000);
    },
    enviar,
  };
}
