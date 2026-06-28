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

// Último segmento não vazio de um caminho absoluto (basename do projeto).
export function basename(path: string): string {
  const parts = path.split('/').filter(Boolean);
  return parts.length ? parts[parts.length - 1] : path;
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
// Caminho absoluto (/ ou ~/) — lazy ate a 1a extensao conhecida, seguida de fim/espaco/delimitador
// (pega path COM espaco tipo "/a/WhatsApp Video….mp4"). Global + case-insensitive.
const _PATH_RE = new RegExp(`(~?/[^\\n]*?\\.(${_EXTS}))(?=$|[\\s)\\]"'\`,])`, 'gi');

export interface FileRef { path: string; name: string; kind: FileKind; url?: string; }

export function parseFilePaths(text: string): FileRef[] {
  const out: FileRef[] = [];
  const seen = new Set<string>();
  for (const m of text.matchAll(_PATH_RE)) {
    const path = m[1];
    if (seen.has(path)) continue;
    seen.add(path);
    const kind = EXT_KIND[m[2].toLowerCase()];
    out.push({ path, name: path.split('/').filter(Boolean).pop() || path, kind });
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
