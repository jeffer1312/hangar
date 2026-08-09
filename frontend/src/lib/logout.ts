// Logout local CENTRALIZADO — dono único é o App.onLogout (round 2 da 4b). Contrato:
// - clearCredentials roda EXATAMENTE uma vez, com ou sem sync (o hub pode estar inacessível e o
//   logout local não pode depender dele);
// - syncLogout é best-effort com política bounded (timeout): hub fora do ar ou pendurado não
//   segura nem trava o logout local — o resto segue e a resposta tardia do sync é descartada;
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

export async function logoutLocal(d: LogoutDeps): Promise<void> {
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
