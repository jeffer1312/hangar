// parsePairing — mesma regra de validarPareamento do front (frontend/src/lib/auth.ts:356).
// Aceita http(s)://host[:porta]/?token=… com opcional &api=, rejeita token vazio/duplicado.
export function parsePairing(texto: string): { base: string; token: string } | null {
  const cru = texto.trim();
  let u: URL;
  try {
    u = new URL(cru);
  } catch {
    return null;
  }
  if (!/^https?:$/.test(u.protocol) || !u.hostname) return null;
  const tokens = u.searchParams.getAll('token');
  const apis = u.searchParams.getAll('api');
  if (
    tokens.length !== 1 ||
    !tokens[0] ||
    /\s/.test(tokens[0]) ||
    apis.length > 1 ||
    (apis.length === 1 && !apis[0])
  )
    return null;
  // api quando presente deve ser URL http/https válida — mesma validação de validarPareamento
  if (apis.length === 1) {
    const api = apis[0];
    if (/\s/.test(api)) return null;
    let apiUrl: URL;
    try {
      apiUrl = new URL(api);
    } catch {
      return null;
    }
    if ((apiUrl.protocol !== 'http:' && apiUrl.protocol !== 'https:') || !apiUrl.hostname) return null;
  }
  const base = (apis[0] ?? u.origin).replace(/\/+$/, '');
  return { base, token: tokens[0] };
}
