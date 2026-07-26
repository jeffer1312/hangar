// Tempo relativo curto em pt-BR a partir de um timestamp epoch em segundos.
// Mesma semântica do antigo formatActivity do SessionCard, agora compartilhada.
export function relativeTime(ts: number | null | undefined): string {
  if (!ts) return '';
  const diff = Date.now() / 1000 - ts;
  if (diff < 60) return 'agora';
  if (diff < 3600) return `${Math.floor(diff / 60)} min atrás`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} h atrás`;
  return new Date(ts * 1000).toLocaleDateString('pt-BR');
}

// Tempo relativo FUTURO ("em X") a partir de um epoch em SEGUNDOS — pro reset de rate limit do Codex
// (resetsAt é sempre um instante futuro; relativeTime() acima só serve pra passado e cairia em "agora"
// pra qualquer futuro). Falsy ou já-passado -> string vazia. Pura/testável.
export function resetsIn(ts: number | null | undefined): string {
  if (!ts) return '';
  const diff = ts - Date.now() / 1000;
  if (diff <= 0) return '';
  if (diff < 3600) return `em ${Math.max(1, Math.floor(diff / 60))} min`;
  if (diff < 86400) return `em ${Math.floor(diff / 3600)} h`;
  return `em ${Math.floor(diff / 86400)} d`;
}

// Vocabulário único de estado (label pt-BR + cor) — compartilhado por SessionCard, Sidebar e
// SessionSwitcherSheet pra mesma sessão nunca aparecer com nomes/cores divergentes.
import type { State, ChatEvent } from './types';
export const stateLabels: Record<State, string> = {
  working: 'em execução',
  idle: 'pronto',
  awaiting_input: 'aguardando',
  dead: 'encerrado',
};
export const stateColors: Record<State, string> = {
  working: 'var(--accent)',
  idle: 'var(--success)',
  awaiting_input: 'var(--warning)',
  dead: 'var(--error)',
};

// Conta sessões aguardando resposta numa lista agregada — usado pro contador do header (mobile/
// desktop) E pro badge do ícone do app (feature #13: navigator.setAppBadge). Pure, sem side-effect.
export function countAwaiting(sessions: { state: State }[]): number {
  return sessions.filter((s) => s.state === 'awaiting_input').length;
}

// Proxima sessao "aguardando resposta" a partir da atual, com wrap-around — usado pela pilula de
// triage do mobile (feature #4). Ordena por NOME (mesmo criterio alfabetico do resto da lista de
// sessoes) pra posicao estavel entre chamadas, mesmo com last_activity mudando a todo instante.
// Sem awaiting nenhum -> null. Atual ja aguardando -> pula pra PROXIMA (nao fica nela mesma), exceto
// se for a unica aguardando (nao ha outra opcao). Mesmo padrao de indice do switchRelative (Chat.svelte).
export function nextAwaiting(sessions: { name: string; state: State }[], currentName: string): string | null {
  const names = sessions.filter((s) => s.state === 'awaiting_input').map((s) => s.name).sort();
  if (names.length === 0) return null;
  const i = names.indexOf(currentName);
  return names[(i < 0 ? 0 : i + 1) % names.length];
}

// "Precisa de você" (feature #6): fila de sessões AGUARDANDO resposta, mesclada de TODOS os
// servidores, ordenada por quem espera HÁ MAIS TEMPO primeiro (last_activity mais antigo = topo,
// mais urgente). Pura, sem side-effect (testável). Sem last_activity vai pro fim; empate desempata
// por nome (posição estável entre polls, mesmo com last_activity mudando).
export function attentionFeed<T extends { name: string; state: State; last_activity?: number | null }>(
  sessions: T[],
): T[] {
  return sessions
    .filter((s) => s.state === 'awaiting_input')
    .sort(
      (a, b) =>
        (a.last_activity ?? Infinity) - (b.last_activity ?? Infinity) || a.name.localeCompare(b.name),
    );
}

// Ordenação compartilhada das LISTAS de sessões (Sidebar + SessionList): quem aguarda resposta
// primeiro (realce de atenção), depois alfabético. Estável: rows só trocam de posição quando o
// ESTADO muda, não a cada last_activity. O Board NÃO usa isto (colunas já separam por estado;
// lá é por atividade recente).
export function sortSessions<T extends { name: string; state: State }>(list: T[]): T[] {
  return [...list].sort(
    (a, b) =>
      Number(b.state === 'awaiting_input') - Number(a.state === 'awaiting_input') ||
      a.name.localeCompare(b.name),
  );
}

// Data/hora local curta a partir de um epoch em SEGUNDOS — usado pra rotular os candidatos de resume
// (última atividade de cada transcript) nos dois views. Falsy -> string vazia. Pura/testável.
export function fmtWhen(mtime?: number | null): string {
  if (!mtime) return '';
  return new Date(mtime * 1000).toLocaleString([], { dateStyle: 'short', timeStyle: 'short' });
}

// Iniciais pra avatar/rail (identifica sem o nome inteiro). "claude-pocket" -> CP, "jeffer1312" -> JE.
// Duas palavras -> 1a letra de cada; uma só -> 2 primeiros chars. Puro/testável. Reusado pelo
// avatar da conta (AccountMenu) e pelo rail recolhido da sidebar.
export function initials(name: string): string {
  const parts = name.split(/[^a-zA-Z0-9]+/).filter(Boolean);
  if (parts.length >= 2) return (parts[0][0] + parts[1][0]).toUpperCase();
  return (parts[0] ?? name).slice(0, 2).toUpperCase();
}

// Último segmento não vazio de um caminho absoluto (basename do projeto).
export function basename(path: string): string {
  const parts = path.split('/').filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
}

// Chave de agrupamento por PROJETO (toggle Servidor|Projeto da lista de sessões, feature #3): o cwd
// normalizado (sem barra final) — duas sessões no MESMO caminho caem no mesmo grupo mesmo vindo de
// servidores diferentes. Sessão sem cwd cai numa chave fixa isolada (não mistura com um projeto real).
const NO_CWD_KEY = 'no-cwd'; // sentinela: cwd real sempre comeca com "/" ou "~", nunca colide
export function projectKey(cwd: string | null | undefined): string {
  if (!cwd) return NO_CWD_KEY;
  return cwd.replace(/\/+$/, '') || '/';
}

// Rótulo exibido pro grupo de projeto: basename do cwd (reusa basename); sem cwd -> rótulo fixo.
export function projectLabel(cwd: string | null | undefined): string {
  return cwd ? basename(cwd) : 'sem projeto';
}

// Modo de agrupamento da lista de sessões (toggle Servidor|Projeto, feature #3).
export type GroupBy = 'server' | 'project';

// Modo EFETIVO dada a preferência do usuário e o nº de servidores. Com <2 servidores, agrupar "por
// servidor" produz 1 único grupo gigante (nada pra separar) e o toggle Servidor|Projeto fica sem
// sentido -> força "por projeto", que ainda organiza a lista por repositório. A preferência crua
// fica intacta (volta a valer assim que houver >=2 servidores). Pura/testável.
export function effectiveGroupBy(pref: GroupBy, serverCount: number): GroupBy {
  return serverCount >= 2 ? pref : 'project';
}

// Cluster de pareamento DENTRO de um grupo (servidor/projeto): sessões do mesmo grupo (pair_gid)
// viram um sub-cluster colapsável; as demais ficam soltas. Devolve LINHAS INTERCALADAS (header do
// grupo seguido dos seus membros; solo = só a sessão) pra o template consumir num {#each} plano —
// sem aninhar/extrair o bloco grande da linha. Preserva a ordem: o cluster nasce na posição do 1º
// membro; os demais são puxados pra junto. N grupos = N clusters. Genérico em T (só exige os campos
// de pareamento) — serve Sidebar e SessionList.
export interface PairFields { name: string; pair_gid?: string | null; pair_peers?: string[] | null; pair_task?: string | null; }
export type PairRow<T> =
  | { kind: 'header'; gid: string; label: string; count: number }
  | { kind: 'session'; session: T; gid: string | null };

export function clusterByPair<T extends PairFields>(sessions: T[]): PairRow<T>[] {
  // Pré-agrupa por gid numa passada (O(n)) — evita o filter-dentro-do-loop O(n²), já que roda
  // inline no {#each} a cada render.
  const byGid = new Map<string, T[]>();
  for (const s of sessions) {
    if (!s.pair_gid) continue;
    const arr = byGid.get(s.pair_gid);
    if (arr) arr.push(s); else byGid.set(s.pair_gid, [s]);
  }
  const out: PairRow<T>[] = [];
  const emitted = new Set<string>();
  for (const s of sessions) {
    const gid = s.pair_gid ?? null;
    if (!gid) { out.push({ kind: 'session', session: s, gid: null }); continue; }
    if (emitted.has(gid)) continue;               // membro já entrou no cluster do 1º
    emitted.add(gid);
    const members = byGid.get(gid)!;
    // Rótulo: tarefa (ex: ABC-1234) do 1º que tiver, senão os nomes.
    const task = members.map((m) => m.pair_task).find((t) => t && t.trim());
    const label = task ? task.trim() : members.map((m) => m.name).join(', ');
    out.push({ kind: 'header', gid, label, count: members.length });
    for (const m of members) out.push({ kind: 'session', session: m, gid });
  }
  return out;
}

// Recado de OUTRA sessão Claude (cp-send): "[de: <sessao>] texto" (1:1) ou "[grupo: <sessao>] texto"
// (aviso pro grupo). Devolve remetente + texto sem o prefixo + scope; null = msg normal do usuário.
// Só APRESENTAÇÃO: o texto guardado em events/pending fica intacto (dedup do Chat compara o cru).
const _PEER_RE = /^\[(de|grupo):\s*([^\]]+)\]\s*/;
export function parsePeerMessage(text: string): { from: string; text: string; scope: 'peer' | 'group' } | null {
  const m = _PEER_RE.exec(text);
  if (!m) return null;
  return { from: m[2].trim(), text: text.slice(m[0].length), scope: m[1] === 'grupo' ? 'group' : 'peer' };
}

// Anexos de arquivo por CAMINHO citado na conversa (sua ou minha msg). v1 = só "preview-worthy"
// (mídia + html + pdf); texto/código fora de proposito pra nao virar ruido (caminho de codigo
// aparece toda hora na prosa). O backend so serve o que esta no transcript (consentido).
export type FileKind = 'image' | 'video' | 'audio' | 'html' | 'pdf';
const EXT_KIND: Record<string, FileKind> = {
  png: 'image', jpg: 'image', jpeg: 'image', gif: 'image', webp: 'image', svg: 'image', avif: 'image', bmp: 'image',
  mp4: 'video', mov: 'video', webm: 'video', mkv: 'video', m4v: 'video', avi: 'video',
  mp3: 'audio', wav: 'audio', m4a: 'audio', ogg: 'audio', flac: 'audio', aac: 'audio',
  html: 'html', htm: 'html',
  pdf: 'pdf',
};
const _EXTS = Object.keys(EXT_KIND).join('|');
// Caminho ABSOLUTO (/ ou ~/) — lazy ate a 1a extensao conhecida, seguida de fim/espaco/delimitador
// (pega path COM espaco tipo "/a/WhatsApp Video….mp4"). Lookbehind (?<![\w.~:/]) evita comecar dentro
// de URL ("https://…") ou logo apos "." (o "/" do "./rel.png" e do REL, nao deste). Global + ci.
const _PATH_RE = new RegExp(`(?<![\\w.~:/])(~?/[^\\n]*?\\.(${_EXTS}))(?=$|[\\s)\\]"'\`,])`, 'gi');
// Caminho RELATIVO com DIRETORIO (./x.png, ../a/x.png, sub/dir/x.png) — jeito comum do Claude citar
// arquivo que criou no cwd. Exige >=1 segmento "dir/" -> NAO casa nome puro "x.png" (ruido de prosa).
// O backend resolve contra o cwd da sessao. Lookbehind tira word/`/`/~/./:/- (nao pega pedaco de path
// absoluto nem de dentro de URL).
const _REL_RE = new RegExp(`(?<![\\w/~.:-])((?:[\\w.-]+/)+[\\w.-]+\\.(${_EXTS}))(?=$|[\\s)\\]"'\`,:])`, 'gi');

export interface FileRef { path: string; name: string; kind: FileKind; url?: string; }

export function parseFilePaths(text: string): FileRef[] {
  const out: FileRef[] = [];
  const seen = new Set<string>();
  // Absoluto + relativo-com-dir. Os dois regexes nao se sobrepoem (lookbehind) -> dedup por string.
  for (const re of [_PATH_RE, _REL_RE]) {
    for (const m of text.matchAll(re)) {
      const path = m[1];
      if (seen.has(path)) continue;
      seen.add(path);
      const kind = EXT_KIND[m[2].toLowerCase()];
      out.push({ path, name: path.split('/').filter(Boolean).pop() || path, kind });
    }
  }
  return out;
}

// URLs http(s) de MIDIA (imagem/video/audio) na conversa -> preview inline no chat, pra ver sem sair
// pro navegador. So midia "tocavel": doc/html remoto fica fora (evita iframe de link aleatorio; o link
// clicavel do markdown ja cobre). url = absoluta (FileAttachment usa direto, sem passar pelo backend).
export function parseMediaUrls(text: string): FileRef[] {
  const out: FileRef[] = [];
  const seen = new Set<string>();
  for (const m of text.matchAll(/https?:\/\/[^\s<>"'`\])]+/gi)) {
    const u = m[0].replace(/[.,;:!?]+$/, '');   // tira pontuacao final colada
    if (seen.has(u)) continue;
    const base = u.split(/[?#]/)[0];            // path sem query/fragment
    const ext = base.split('.').pop()?.toLowerCase() ?? '';
    const kind = EXT_KIND[ext];
    if (kind !== 'image' && kind !== 'video' && kind !== 'audio') continue;
    seen.add(u);
    out.push({ path: u, url: u, name: base.split('/').filter(Boolean).pop() || u, kind });
  }
  return out;
}

// Detecta o(s) marcador(es) de imagem nas mensagens do usuario:
// "<legenda> — 📎 imagem: <path1> 📎 imagem: <path2> ..." (1+ imagens, ou sem legenda).
// Devolve { caption, filenames } ou null. Cada filename e o basename do path (sem espaco,
// nome gerado), entao da pra separar varias numa linha so pelo proprio marcador.
export function parseImageMessage(text: string): { caption: string; filenames: string[] } | null {
  const marker = '📎 imagem: ';
  const i = text.indexOf(marker);
  if (i < 0) return null;
  // Tudo a partir do 1o marcador pode conter N "📎 imagem: <path>".
  const filenames = text
    .slice(i)
    .split(marker)
    .map((s) => s.trim())
    .filter(Boolean)
    .map((p) => p.split('/').filter(Boolean).pop() ?? '')
    .filter(Boolean);
  if (!filenames.length) return null;
  let caption = text.slice(0, i).trim();
  if (caption.endsWith('—')) caption = caption.slice(0, -1).trim();
  return { caption, filenames };
}

// Selecao do broadcast (feature #9): agrupa os nomes SELECIONADOS por servidor-dono, na ordem em
// que aparecem em `sessions` — cada grupo vira 1 chamada a broadcast() nesse servidor (selectServer/
// restore, igual ao resto do app). Chave de selecao = "<serverId>:<name>" (mesma composta usada nas
// keys #each da lista). Pura/testavel; servidor sem nenhum selecionado nao entra no Map.
export function groupSelectedByServer(
  sessions: { name: string; serverId: string }[],
  selected: Set<string>,
): Map<string, string[]> {
  const out = new Map<string, string[]>();
  for (const s of sessions) {
    if (!selected.has(`${s.serverId}:${s.name}`)) continue;
    const arr = out.get(s.serverId);
    if (arr) arr.push(s.name);
    else out.set(s.serverId, [s.name]);
  }
  return out;
}

// ── Compare (feature #11): grade lado a lado com a última resposta de N sessões ─────────────────
export interface CompareId { serverId: string; name: string }

// Codifica a seleção pro hash da rota (#/compare/<param>): cada par "serverId:nome" com AMBOS os
// lados URI-encoded separadamente, juntos por vírgula. encodeURIComponent escapa ':' e ',' -> o
// texto codificado nunca contém os separadores literais, então o parse abaixo nunca ambigua.
export function encodeCompareIds(ids: CompareId[]): string {
  return ids.map((s) => `${encodeURIComponent(s.serverId)}:${encodeURIComponent(s.name)}`).join(',');
}

export function parseCompareIds(param: string): CompareId[] {
  if (!param) return [];
  return param
    .split(',')
    .map((pair): CompareId | null => {
      const i = pair.indexOf(':');
      if (i < 0) return null;
      const serverId = decodeURIComponent(pair.slice(0, i));
      const name = decodeURIComponent(pair.slice(i + 1));
      return serverId && name ? { serverId, name } : null;
    })
    .filter((x): x is CompareId => x !== null);
}

// Último assistant_msg com texto de uma lista de eventos (transcript ou stream ao vivo) — usado
// pelo card da grade de comparação pra mostrar só a resposta MAIS RECENTE, sem montar o chat inteiro.
export function latestAssistantEvent(events: ChatEvent[]): ChatEvent | null {
  for (let i = events.length - 1; i >= 0; i--) {
    if (events[i].kind === 'assistant_msg' && events[i].text) return events[i];
  }
  return null;
}

// Cauda CRUA do /history -> só as bolhas que o card do quadro desenha: user/assistant com texto,
// começando no 1º user_msg. A cauda corta no meio de um turno, então o 1º assistant_msg pode ser
// resposta a um prompt que ficou de fora — órfã, sem o "porquê" acima dela.
// Mora AQUI e não na rota: /history?limit é compartilhado com a espiada do hover da Sidebar, que só
// quer o último assistant_msg — cortar no backend jogava fora justamente a resposta que ela procura
// (medido: cauda [assistant_msg, user_msg, tool_use…] -> o corte matava o popover). É preferência de
// RENDERIZAÇÃO do card, não do endpoint. Sem user_msg na janela -> devolve tudo (card vazio é pior).
export function bubblesFromTail(events: ChatEvent[]): ChatEvent[] {
  const msgs = events.filter((e) => (e.kind === 'user_msg' || e.kind === 'assistant_msg') && e.text);
  const start = msgs.findIndex((e) => e.kind === 'user_msg');
  return start > 0 ? msgs.slice(start) : msgs;
}

// Cor determinística por grupo de pareamento (gid): mesmo grupo = mesma cor em QUALQUER view
// (chip 🤝 do card, barra/aro do canvas). Hash simples -> matiz. A LUMINÂNCIA vem do token
// --pair-l (app.css: 0.72 no escuro, mais fundo no claro) — L fixa 0.72 era pastel invisível
// sobre fundo quase-branco (aro de 1.5px sumia no tema claro).
export function pairColor(gid: string): string {
  let h = 0;
  for (let i = 0; i < gid.length; i++) h = (h * 31 + gid.charCodeAt(i)) >>> 0;
  return `oklch(var(--pair-l, 0.72) 0.14 ${h % 360})`;
}

// Janela de contexto legível: 1e6 -> "1M", 1.5e6 -> "1.5M", 200000 -> "200k".
// Separado do abbrevNum porque este quer INTEIRO quando exato ("1M", não "1.0M") e usa 'k'
// minúsculo, casando o que a própria statusline do terminal mostra.
// O corte olha o valor JÁ ARREDONDADO, não o bruto: com o teste em `n >= 1e6`, um total de
// 999_700 caía no ramo k e virava "1000k" — o exato defeito que esta função existe pra evitar.
export function ctxWindow(n: number): string {
  const k = Math.round(n / 1000);
  if (k < 1000) return `${k}k`;
  // `.replace(/\.0$/, '')` como o abbrevNum abaixo: 1e6 e 999_700 viram "1M", não "1.0M".
  return `${(n / 1_000_000).toFixed(1).replace(/\.0$/, '')}M`;
}

// Abrevia contagem grande: 3668662 -> "3.7M", 1.5e9 -> "1.5B", 999 -> "999".
export function abbrevNum(n: number): string {
  for (const [div, suf] of [[1e9, 'B'], [1e6, 'M'], [1e3, 'K']] as const) {
    if (n >= div) return (n / div).toFixed(1).replace(/\.0$/, '') + suf;
  }
  return String(Math.round(n));
}
