// Logout local CENTRALIZADO — dono único é o App.onLogout (round 2/3 da 4b). Contrato:
// - clearCredentials roda EXATAMENTE uma vez por logout, com ou sem sync (o hub pode estar
//   inacessível e o logout local não pode depender dele);
// - syncLogout é best-effort com política bounded (timeout): hub fora do ar ou pendurado não
//   segura nem trava o logout local — o resto segue e a resposta tardia do sync é descartada;
// - LOCK in-flight compartilhado: duas origens (Sair no drawer + remover-último, Sidebar +
//   SessionList) disparando juntas esperam a MESMA promise — uma sincronização e uma limpeza;
// - nada aqui rejeita: quem chama pode await sem unhandled/hang.
//
// DI pura/testável: o App injeta as dependências reais (syncLogout, clearKey, clearCredentials,
// aoSair com o estado da sessão).

export interface LogoutDeps {
  temEncKey: boolean;
  syncLogout: () => Promise<unknown>;
  clearKey: () => void;
  clearCredentials: () => void;
  /** App: zera estado da sessão (encKey, syncReady, listener do sync) e navega. */
  aoSair: () => void;
  timeoutMs?: number;
}

let emVoo: Promise<void> | null = null;

export function logoutLocal(d: LogoutDeps): Promise<void> {
  // Lock in-flight: a segunda origem (mesmo processo, mesmo dono) não re-executa — espera a
  // promise da primeira. O lock vive no MÓDULO, não na instância: todas as origens convergem
  // no App.onLogout, que é quem chama este módulo.
  if (emVoo) return emVoo;
  emVoo = exec(d).finally(() => { emVoo = null; });
  return emVoo;
}

async function exec(d: LogoutDeps): Promise<void> {
  d.clearCredentials();
  if (!d.temEncKey) {
    d.aoSair();
    return;
  }
  try {
    await Promise.race([
      d.syncLogout(),
      new Promise<never>((_, rej) =>
        setTimeout(() => rej(new Error('syncLogout timeout')), d.timeoutMs ?? 3000)),
    ]);
  } catch {
    // Hub inacessível/pendurado: logout local segue de qualquer jeito. A rejeição tardia do sync
    // já tem handler no race — não vira unhandled rejection.
  }
  d.clearKey();
  d.aoSair();
}
