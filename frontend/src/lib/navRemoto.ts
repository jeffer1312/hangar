// Endereço do acesso remoto ao navegador da sessão. Mesmo molde do `termUrlForServer` (term.ts:27),
// inclusive o fallback pra `location.origin`: o servidor de mesma origem guarda baseUrl vazio e
// `new WebSocket('/api/...')` sozinho levanta SyntaxError.
import { getBaseUrl, getToken } from './auth';

// `w` = largura real em pixels que a tela de quem olha consegue mostrar. Sem isso o backend manda
// quadro de 1400px pro iPhone, e a banda extra é paga no túnel sem nada aparecer a mais.
export function larguraUtil(): number {
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  return Math.round(Math.min(window.innerWidth, 1400) * dpr);
}

export function navUrl(name: string): string {
  const base = (getBaseUrl() || location.origin).replace(/^http/, 'ws');
  const qs = new URLSearchParams({ token: getToken() || '', w: String(larguraUtil()) });
  // `nav-remoto`, não `nav`: o `POST/DELETE /nav` já existe (marcador de página pendente do
  // `hangar-preview open`) e um WebSocket com o mesmo caminho seria só confusão pra quem lê o log.
  return `${base}/api/sessions/${encodeURIComponent(name)}/nav-remoto?${qs}`;
}

export type QuadroNav = { d: string; w: number; h: number; lento?: boolean };
