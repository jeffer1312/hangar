// Aceita o usuario digitar "localhost:8765" sem esquema: fetch leria "localhost:" como protocolo e
// quebraria ("Failed to fetch"). Prefixa http:// quando nao ha "scheme://", e tira barra final pra
// casar com a normalizacao de addServer (dedup por baseUrl).
export function normalizeBaseUrl(raw: string): string {
  const s = raw.trim();
  if (!s) return '';
  const withScheme = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(s) ? s : `http://${s}`;
  return withScheme.replace(/\/+$/, '');
}
