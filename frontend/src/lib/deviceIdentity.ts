const KEY = 'cp_device_id';
const VALID = /^[A-Za-z0-9-]{1,64}$/;

function newId(): string {
  // randomUUID só existe em contexto seguro; o PWA também abre por http na rede local.
  if (typeof crypto.randomUUID === 'function' && isSecureContext) return crypto.randomUUID();
  return Array.from(crypto.getRandomValues(new Uint8Array(16)), (b) => b.toString(16).padStart(2, '0')).join('');
}

export function deviceIdentity(): { kind: 'pwa'; id: string } | null {
  // No PC o navegador não é celular: o aviso do nativo é de celular conectado.
  if (!matchMedia('(pointer: coarse)').matches) return null;
  try {
    const saved = localStorage.getItem(KEY);
    if (saved && VALID.test(saved)) return { kind: 'pwa', id: saved };
    const id = newId();
    localStorage.setItem(KEY, id);
    return { kind: 'pwa', id };
  } catch {
    // Sem armazenamento, cada abertura seria um aparelho novo: melhor não mandar.
    return null;
  }
}
