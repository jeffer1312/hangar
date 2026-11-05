const RESEND_DELAY_MS = 10_000;
// A falha de uma máquina desligada tem destino nela mesma; insistir pesa na VPN do iPhone.
const UNREACHABLE_DELAYS_MS = [10_000, 30_000, 60_000, 120_000, 300_000, 600_000, 1_800_000];

/** Fila limitada em memória. O destino pertence ao evento; credenciais são consultadas no envio. */
export function criarTransporteDiag(tokenPara: (destino: string) => string | null | undefined) {
  const fila: { destino: string; evento: Record<string, unknown> }[] = [];
  const unreachable = new Map<string, { failures: number; retryAt: number }>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let timerAt = 0;
  let enviando: Promise<void> | undefined;

  function agendar(ms: number) {
    const at = Date.now() + ms;
    if (timer !== undefined && timerAt <= at) return;
    clearTimeout(timer);
    timerAt = at;
    timer = setTimeout(() => { timer = undefined; void enviar(); }, ms);
  }

  function markUnreachable(destino: string) {
    const failures = Math.min((unreachable.get(destino)?.failures ?? 0) + 1, UNREACHABLE_DELAYS_MS.length);
    unreachable.set(destino, { failures, retryAt: Date.now() + UNREACHABLE_DELAYS_MS[failures - 1] });
  }

  function nextSendDelay(): number {
    const now = Date.now();
    return Math.min(...fila.map(({ destino }) => {
      const wait = unreachable.get(destino);
      return wait ? Math.max(0, wait.retryAt - now) : RESEND_DELAY_MS;
    }));
  }

  function enviar(): Promise<void> {
    if (enviando) return enviando;
    clearTimeout(timer); timer = undefined;
    enviando = (async () => {
      const now = Date.now();
      const destinos = [...new Set(fila.map((item) => item.destino))]
        .filter((destino) => (unreachable.get(destino)?.retryAt ?? 0) <= now);
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
            unreachable.delete(destino);
            if (!resposta.ok) return;
            const confirmacao = await resposta.json();
            if (!confirmacao || typeof confirmacao !== 'object'
              || !('gravadas' in confirmacao) || confirmacao.gravadas !== lote.length) return;
            const enviados = new Set(lote);
            for (let i = fila.length - 1; i >= 0; i--) {
              if (enviados.has(fila[i])) fila.splice(i, 1);
            }
          }
        } catch {
          // A próxima tentativa preserva o destino original.
          markUnreachable(destino);
        }
      }));
    })().finally(() => {
      enviando = undefined;
      if (fila.length) agendar(nextSendDelay());
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
