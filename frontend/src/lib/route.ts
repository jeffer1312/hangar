import { parseCompareIds, type CompareId } from './format';

// ── Hash-based Router ────────────────────────────────────────────────
export type Route =
  | { name: 'loading' }
  | { name: 'login' }
  | { name: 'sessions' }
  | { name: 'costs' }
  | { name: 'archive'; deepLink?: { serverId: string; project: string; sessionId: string } }
  | { name: 'chat'; sessionName: string; serverId: string | null }
  // Quadro: sessionName/serverId preenchidos = overlay do card aberto por cima dele (#/board puro
  // = quadro sem overlay). Mesmo par de campos do 'chat' de propósito — é o que deixa o $effect
  // do servidor ativo servir às duas rotas sem duplicar a regra.
  | { name: 'board'; sessionName: string | null; serverId: string | null }
  // Canvas livre: visualização irmã do quadro (#/canvas), mesmos campos overlay do board de
  // propósito — o servidor ativo e a espiada seguem a mesma regra pras duas rotas.
  | { name: 'canvas'; sessionName: string | null; serverId: string | null }
  | { name: 'compare'; ids: CompareId[] };

export function parseHash(hash: string): Route {
  // `.split('?')[0]`: o painel de Configuracoes mora num segundo eixo do endereco
  // (?config=&srv=, ver lib/configRoute.ts). O corte fica AQUI, na primeira linha, e nao nos
  // chamadores no App.svelte — sao tres (o boot que le `window.location.hash`, o `$derived` de
  // `route` e `applyRouteServer`), e um esquecido devolve rota errada calada. Seguro: nome de
  // sessao com `?` real nunca chega cru, navigateToChat faz encodeURIComponent.
  const path = hash.replace(/^#/, '').split('?')[0];
  // Rota de chat COM servidor: #/chat/<serverId>/<nome>. Sessões homônimas em servidores
  // diferentes precisam de hashes distintos — só o nome fazia o clique "não trocar" (hash igual
  // não dispara hashchange) e pior: o composer já falava com o servidor novo enquanto a tela
  // mostrava o transcript do antigo (cross-wire). Forma legada #/chat/<nome> segue aceita
  // (serverId null = servidor ativo).
  const chatServerMatch = path.match(/^\/chat\/([^/]+)\/(.+)$/);
  const chatMatch = chatServerMatch ? null : path.match(/^\/chat\/(.+)$/);
  if (chatServerMatch || chatMatch) {
    const serverId = chatServerMatch ? decodeURIComponent(chatServerMatch[1]) : null;
    const sessionName = decodeURIComponent(chatServerMatch ? chatServerMatch[2] : chatMatch![1]);
    // Auto-cura: um hash #/chat/undefined (ou vazio) preso na URL fazia o Chat montar com
    // sessionName "undefined" -> SSE em /sessions/undefined/events (404 em loop eterno).
    // Trata como invalido e cai na lista, em vez de prender o usuario numa sessao fantasma.
    // Barra "undefined" E "null" (string): ambos viravam #/chat/null -> currentSession="null"
    // (truthy) -> Chat monta -> openEventStream("null") -> GET /api/sessions/null/events 404 em loop.
    if (sessionName && sessionName !== 'undefined' && sessionName !== 'null') {
      return { name: 'chat', sessionName, serverId };
    }
  }
  // Grade de comparação (feature #11): #/compare/<ids codificados>, ver encodeCompareIds/
  // parseCompareIds em lib/format.ts. Sem decodeURIComponent aqui — parseCompareIds já decodifica
  // cada campo por dentro (decodificar o param inteiro de novo ia dar decode duplo).
  const compareMatch = path.match(/^\/compare\/(.+)$/);
  if (compareMatch) return { name: 'compare', ids: parseCompareIds(compareMatch[1]) };
  if (path === '/costs') return { name: 'costs' };
  // Deep-link da busca (feature #10): #/archive/<serverId>/<project>/<sid> abre a conversa arquivada
  // direto no servidor dono. #/archive puro segue no browser normal de pastas.
  const archiveDeep = path.match(/^\/archive\/([^/]+)\/([^/]+)\/([^/]+)$/);
  if (archiveDeep) {
    return {
      name: 'archive',
      deepLink: {
        serverId: decodeURIComponent(archiveDeep[1]),
        project: decodeURIComponent(archiveDeep[2]),
        sessionId: decodeURIComponent(archiveDeep[3]),
      },
    };
  }
  if (path === '/archive') return { name: 'archive' };
  // Quadro kanban (visualização irmã da lista+chat) — só existe no desktop; no mobile o render
  // trata board como a lista normal. ANTES do regex de 2 segmentos: só pra deixar explícito que o
  // quadro puro é a forma base (os dois padrões não se sobrepõem — o regex exige serverId+nome).
  if (path === '/board') return { name: 'board', sessionName: null, serverId: null };
  // Overlay do card é ROTA (#/board/<serverId>/<nome>), não estado do shell: deep-link, botão
  // VOLTAR e reload saem de graça, e o servidor ativo vira função da rota (o $effect abaixo aponta
  // ele) em vez de exigir capture/restore manual ao abrir/fechar o overlay.
  const boardMatch = path.match(/^\/board\/([^/]+)\/(.+)$/);
  if (boardMatch) {
    const sessionName = decodeURIComponent(boardMatch[2]);
    // Mesma auto-cura do #/chat: um hash podre montaria o Chat com sessionName "undefined"/"null"
    // -> SSE em /sessions/undefined/events (404 em loop). Aqui degrada pro quadro sem overlay.
    if (sessionName && sessionName !== 'undefined' && sessionName !== 'null') {
      return { name: 'board', sessionName, serverId: decodeURIComponent(boardMatch[1]) };
    }
    return { name: 'board', sessionName: null, serverId: null };
  }
  // Canvas livre (#/canvas) — espelha o board em tudo: rota base + overlay do card com a MESMA
  // auto-cura de undefined/null. Só desktop; no mobile cai na lista (ver render).
  if (path === '/canvas') return { name: 'canvas', sessionName: null, serverId: null };
  const canvasMatch = path.match(/^\/canvas\/([^/]+)\/(.+)$/);
  if (canvasMatch) {
    const sessionName = decodeURIComponent(canvasMatch[2]);
    if (sessionName && sessionName !== 'undefined' && sessionName !== 'null') {
      return { name: 'canvas', sessionName, serverId: decodeURIComponent(canvasMatch[1]) };
    }
    return { name: 'canvas', sessionName: null, serverId: null };
  }
  return { name: 'sessions' };
}
