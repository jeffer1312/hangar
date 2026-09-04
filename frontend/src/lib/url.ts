// Aceita o usuario digitar "localhost:8765" sem esquema: fetch leria "localhost:" como protocolo e
// quebraria ("Failed to fetch"). Prefixa http:// quando nao ha "scheme://", e tira barra final pra
// casar com a normalizacao de addServer (dedup por baseUrl).
export function normalizeBaseUrl(raw: string): string {
  const s = raw.trim();
  if (!s) return '';
  const withScheme = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(s) ? s : `http://${s}`;
  return withScheme.replace(/\/+$/, '');
}

// Porta em que o backend nasce (backend/app/config.py). Quem digita só o IP quase sempre quer ela.
const PORTA_PADRAO = 8765;
const TEM_ESQUEMA = /^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//;
const IPV4 = /^\d{1,3}(\.\d{1,3}){3}$/;

interface EnderecoNormalizado {
  base: string;                 // origem: http(s)://host[:porta], sem barra final
  token: string | null;         // o ?token= de um link de pareamento colado inteiro
  alternativa: string | null;   // segunda origem a testar quando o esquema foi deduzido
}

// O que a pessoa digita ao ligar uma máquina na outra: IP, nome, com ou sem porta, ou o link de
// pareamento colado inteiro. Nome com ponto é tratado como domínio atrás de https (Tailscale serve
// e proxy não expõem a porta crua), com http na porta padrão de reserva — um FQDN de rede local
// responde lá; IP, `localhost`, nome sem ponto e `.local` são rede local, na porta padrão.
// Esquema ou porta explícitos vencem a dedução, e aí não há reserva.
export function normalizarEndereco(cru: string): EnderecoNormalizado | null {
  const s = cru.trim();
  if (!s) return null;
  const semEsquema = !TEM_ESQUEMA.test(s);
  let url: URL;
  try { url = new URL(semEsquema ? `http://${s}` : s); } catch { return null; }
  if ((url.protocol !== 'http:' && url.protocol !== 'https:') || !url.hostname) return null;

  const tokens = url.searchParams.getAll('token');
  if (tokens.length > 1) return null;
  const token = tokens.length === 1 ? tokens[0] : null;
  if (token !== null && (!token || /\s/.test(token))) return null;

  let alternativa: string | null = null;
  if (semEsquema && !url.port) {
    const host = url.hostname;
    const redeLocal = IPV4.test(host) || host === 'localhost' || !host.includes('.') || host.endsWith('.local');
    if (redeLocal) {
      url.port = String(PORTA_PADRAO);
    } else {
      alternativa = `http://${host}:${PORTA_PADRAO}`;
      url.protocol = 'https:';
    }
  }
  return { base: url.origin, token, alternativa };
}
