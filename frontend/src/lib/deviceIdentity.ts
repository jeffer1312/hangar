import { novaChaveIdempotente } from './credenciais';

const KEY = 'cp_device_id';
const VALID = /^[A-Za-z0-9-]{1,64}$/;

export function deviceIdentity(): { kind: 'pwa'; id: string } | null {
  // No PC o navegador não é celular: o aviso do nativo é de celular conectado.
  if (typeof matchMedia !== 'function' || !matchMedia('(pointer: coarse)').matches) return null;
  try {
    const saved = localStorage.getItem(KEY);
    if (saved && VALID.test(saved)) return { kind: 'pwa', id: saved };
    // Fora de contexto seguro (http na rede local) ela cai no getRandomValues.
    const id = novaChaveIdempotente();
    localStorage.setItem(KEY, id);
    return { kind: 'pwa', id };
  } catch {
    // Sem armazenamento, cada abertura seria um aparelho novo: melhor não mandar.
    return null;
  }
}
