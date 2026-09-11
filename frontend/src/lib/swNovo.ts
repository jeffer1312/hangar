// Depois de uma atualização, o reload só serve o build novo quando o service worker NOVO já está
// ativo. `reg.update()` resolve com o SW novo ainda INSTALANDO (baixando o precache), então
// recarregar logo em seguida volta servido pelo SW velho — a tela diz "atualizado" mostrando o
// bundle de antes, sem nada denunciando (aconteceu em 10/09/2026, depois do Atualizar pelo app).
// O `sw.ts` já faz skipWaiting + clientsClaim, então basta esperar o `statechange` chegar a
// `activated`.

type Reg = Pick<ServiceWorkerRegistration, 'update' | 'installing' | 'waiting' | 'active' | 'addEventListener'>;

/** true = o SW ativo é o novo (ou não há SW); false = deu errado (ficou `redundant`). */
export async function aguardarSwNovo(reg: Reg | null | undefined): Promise<boolean> {
  if (!reg) return true;
  // O `updatefound` é armado ANTES do update(): o installing pode aparecer antes de a promise
  // resolver, e ler `reg.installing` só depois perdia esse instante em alguns navegadores.
  let novo: ServiceWorker | null = null;
  reg.addEventListener('updatefound', () => { novo = reg.installing; });
  await reg.update();
  novo = novo ?? reg.installing ?? reg.waiting;
  // Sem worker novo: o sw.js do servidor é o mesmo que já está ativo — o build não mudou o SW,
  // e o reload já vem certo.
  if (!novo) return true;
  const w: ServiceWorker = novo;
  if (w.state === 'activated') return true;
  if (w.state === 'redundant') return false;
  return new Promise((res) => {
    w.addEventListener('statechange', () => {
      if (w.state === 'activated') res(true);
      if (w.state === 'redundant') res(false);
    });
  });
}

/** Plano B do teto: esvazia os caches sem desregistrar o SW (desregistrar mataria a inscrição de
 *  push do celular). O SW velho, sem cache, busca da rede — e o index novo puxa os pedaços novos. */
export async function esvaziarCaches(): Promise<number> {
  if (typeof caches === 'undefined') return 0;
  const ks = await caches.keys();
  await Promise.all(ks.map((k) => caches.delete(k)));
  return ks.length;
}
