import { Platform } from 'react-native';
import { Directory, DownloadTask, File, Paths } from 'expo-file-system';
import { startActivityAsync } from 'expo-intent-launcher';
import * as Sharing from 'expo-sharing';
import { createMMKV } from 'react-native-mmkv';
import { fileKind, registrarDiag } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { downloadedVersion, keptIsCurrent, type KeptVersion } from './attachmentUrls';

const FLAG_GRANT_READ_URI_PERMISSION = 1;

/** Teto da pasta de anexos baixados; acima dele saem os usados há mais tempo. */
export const MEDIA_CACHE_LIMIT = 1024 * 1024 * 1024;

const MEDIA_DIR = 'chat-media';
// O leitor de PDF e a folha de texto da página guardam cópias próprias; o "Limpar" leva as duas.
const SIDE_DIRS = ['pdf-reader', 'chat-page-source'];
// Onde a versão anterior guardava as cópias de "Mídia e arquivos", fora do teto: hoje moram em `chat-media`.
const LEGACY_DIRS = ['media-files', 'media-files-version'];
// Prefixo das cópias guardadas: o nome não cruza com o das cópias da conversa (`hash-nome`).
const KEPT_PREFIX = 'kept-';
const PROBE_TIMEOUT_MS = 4000;
// Só o Android retoma pelo pedaço já gravado; no iOS a sessão em segundo plano espera a rede sozinha.
const RESUME_DELAYS_MS = [2000, 5000];
// Sufixo reservado do download em andamento: anexo da pessoa terminado em `.part` é arquivo, não pedaço.
export const PART_SUFFIX = '.hangar-part';
// Pedaço mais novo que isto pode ser de um download em andamento noutra tela.
const STALE_PART_MS = 60 * 60 * 1000;
// Arquivo usado há pouco pode estar sendo lido pela folha de compartilhar ou pelo app que o abriu.
const RECENT_USE_MS = 10 * 60 * 1000;

export interface DownloadProgress {
  written: number;
  /** null = o servidor não disse o tamanho: o anel fica indeterminado. */
  total: number | null;
}

export interface DownloadOptions {
  onProgress?: (p: DownloadProgress) => void;
  signal?: AbortSignal;
}

// `version`: só a cópia guardada da tela "Mídia e arquivos", com o tamanho e a data que o servidor dizia ao baixar.
interface Entry { etag: string | null; size: number; used: number; version?: KeptVersion }

const index = createMMKV({ id: 'hangar-media-cache' });

function readEntry(key: string): Entry | null {
  const raw = index.getString(key);
  if (!raw) return null;
  try { return JSON.parse(raw) as Entry; } catch { return null; }
}
const writeEntry = (key: string, e: Entry) => index.set(key, JSON.stringify(e));

function mediaDir(): Directory {
  const dir = new Directory(Paths.cache, MEDIA_DIR);
  dir.create({ idempotent: true, intermediates: true });
  return dir;
}

const safeName = (name: string) => name.replace(/[^\w.-]/g, '_');

// cyrb53 com duas sementes (~106 bits): com 32 bits, duas chaves de nomes iguais podiam cair no mesmo
// arquivo e uma abria a cópia da outra.
function cyrb53(s: string, seed: number): number {
  let h1 = 0xdeadbeef ^ seed;
  let h2 = 0x41c6ce57 ^ seed;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 2654435761);
    h2 = Math.imul(h2 ^ c, 1597334677);
  }
  h1 = Math.imul(h1 ^ (h1 >>> 16), 2246822507) ^ Math.imul(h2 ^ (h2 >>> 13), 3266489909);
  h2 = Math.imul(h2 ^ (h2 >>> 16), 2246822507) ^ Math.imul(h1 ^ (h1 >>> 13), 3266489909);
  return 4294967296 * (2097151 & h2) + (h1 >>> 0);
}

export function keyHash(s: string): string {
  return `${cyrb53(s, 0).toString(36)}${cyrb53(s, 1).toString(36)}`;
}

// O nome guarda o endereço inteiro: o mesmo caminho em outra sessão ou máquina é outro arquivo.
function cacheKey(uri: string, name: string): string {
  return `${keyHash(uri)}-${safeName(name)}`;
}

export class HttpStatusError extends Error {
  constructor(readonly status: number) {
    // O mesmo texto que o download nativo usa, que é o que as telas reconhecem como 404.
    super(`response has status: ${status}`);
  }
}

// Falha de arrumação do cache não muda o resultado de quem pediu: vai ao diário com o nome e o texto do erro.
function diagCache(codigo: string, e: unknown): void {
  const detalhe = e instanceof Error ? `${e.name}: ${e.message}` : String(e);
  registrarDiag({ evento: 'anexos.cache_falhou', nivel: 'aviso', tela: 'anexos', codigo, detalhe: detalhe.slice(0, 300) });
}

// O erro que a tela mostra é o do download (cancelado continua cancelado, 404 continua 404), não o da limpeza do pedaço.
function dropPart(tmp: File): void {
  try { if (tmp.exists) tmp.delete(); } catch (d) { diagCache('part_delete_failed', d); }
}

// Roda com a cópia já gravada: o download deu certo mesmo se a limpeza falhar.
function evictQuietly(keep: string): void {
  try { evict(keep); } catch (e) { diagCache('evict_failed', e); }
}

export const isMissing = (e: unknown) => /(status:?|HTTP) 404\b/.test(e instanceof Error ? e.message : String(e));
export const isAbort = (e: unknown) => e instanceof Error && e.name === 'AbortError';

function abortError(): Error {
  const e = new Error('aborted');
  e.name = 'AbortError';
  return e;
}

function throwIfAborted(signal?: AbortSignal): void {
  if (signal?.aborted) throw abortError();
}

function wait(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) return reject(abortError());
    const t = setTimeout(() => { signal?.removeEventListener('abort', stop); resolve(); }, ms);
    const stop = () => { clearTimeout(t); reject(abortError()); };
    signal?.addEventListener('abort', stop, { once: true });
  });
}

interface Probe { etag: string | null; size: number | null }

// Um byte só: devolve o ETag (o servidor o muda quando o arquivo é regerado no mesmo caminho) e o
// tamanho total pelo Content-Range. null = sem rede; resposta de erro do servidor vira exceção.
// O Content-Length de um 200 não serve de tamanho: pode ser do corpo comprimido.
export async function probe(uri: string, headers: Record<string, string> | undefined, signal?: AbortSignal): Promise<Probe | null> {
  const ctl = new AbortController();
  const stop = () => ctl.abort();
  signal?.addEventListener('abort', stop, { once: true });
  const timer = setTimeout(stop, PROBE_TIMEOUT_MS);
  try {
    const r = await fetch(uri, { headers: { ...headers, Range: 'bytes=0-0', 'Cache-Control': 'no-cache' }, signal: ctl.signal });
    if (r.status === 416) {
      const total = /\/(\d+)/.exec(r.headers.get('content-range') ?? '');
      return { etag: r.headers.get('etag'), size: total ? Number(total[1]) : 0 };
    }
    if (!r.ok) throw new HttpStatusError(r.status);
    const range = r.status === 206 ? /\/(\d+)/.exec(r.headers.get('content-range') ?? '') : null;
    return { etag: r.headers.get('etag'), size: range ? Number(range[1]) : null };
  } catch (e) {
    if (signal?.aborted) throw abortError();
    if (e instanceof HttpStatusError) throw e;
    return null;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', stop);
    // Só os cabeçalhos interessam: servidor que ignora o Range mandaria o arquivo inteiro junto com o download.
    ctl.abort();
  }
}

// A queda de rede no Android deixa o pedaço gravado, e o resumo do download nativo de lá é o tamanho
// dele: o pedido continua com Range. Erro HTTP ou cancelamento não se retomam.
async function downloadResuming(uri: string, headers: Record<string, string> | undefined, tmp: File, expected: number | null, opts: DownloadOptions): Promise<void> {
  const onProgress = opts.onProgress
    ? ({ bytesWritten, totalBytes }: { bytesWritten: number; totalBytes: number }) =>
      opts.onProgress!({ written: bytesWritten, total: totalBytes > 0 ? totalBytes : expected })
    : undefined;
  throwIfAborted(opts.signal);
  let task = File.createDownloadTask(uri, tmp, { headers, onProgress, signal: opts.signal });
  let start = () => task.downloadAsync();
  for (let attempt = 0; ; attempt++) {
    let done: File | null;
    try {
      done = await start();
    } catch (e) {
      task.release();
      if (opts.signal?.aborted || isAbort(e)) throw abortError();
      const http = /\b(HTTP|status:?) \d{3}\b/.test(e instanceof Error ? e.message : String(e));
      if (Platform.OS !== 'android' || http || attempt >= RESUME_DELAYS_MS.length || !tmp.exists || tmp.size <= 0) throw e;
      await wait(RESUME_DELAYS_MS[attempt], opts.signal);
      throwIfAborted(opts.signal);
      task = DownloadTask.fromSavable(
        { url: uri, fileUri: tmp.uri, isDirectory: false, headers, resumeData: String(tmp.size) },
        { onProgress, signal: opts.signal },
      );
      start = () => task.resumeAsync();
      continue;
    }
    task.release();
    // null = pausado; ninguém pausa aqui, então é um download que não terminou.
    if (!done) throw new Error(m.file_card_failed());
    return;
  }
}

/**
 * Cópia local do anexo do servidor (o "Abrir em…" do sistema e a folha de compartilhar não mandam o
 * token). Fica guardada e é reaproveitada: com rede, um pedido de um byte confere se o arquivo mudou
 * no servidor; sem rede, entrega a guardada. O download vai para um nome temporário e só substitui a
 * cópia ao terminar e conferir o tamanho: um download cortado nunca vira o arquivo guardado.
 */
export async function localCopy(uri: string, headers: Record<string, string> | undefined, name: string, opts: DownloadOptions = {}): Promise<File> {
  const dir = mediaDir();
  const key = cacheKey(uri, name);
  const target = new File(dir, key);
  const entry = readEntry(key);
  let info: Probe | null = null;
  if (entry && target.exists) {
    // Servidor recusando (arquivo apagado, sessão expirada) não tira a cópia que a pessoa já tem.
    try { info = await probe(uri, headers, opts.signal); } catch (e) { if (isAbort(e)) throw e; info = null; }
    const same = !info || (info.etag ? info.etag === entry.etag : info.size === target.size);
    if (same) {
      writeEntry(key, { ...entry, used: Date.now() });
      opts.onProgress?.({ written: target.size, total: target.size });
      return target;
    }
  } else {
    info = await probe(uri, headers, opts.signal);
  }
  const tmp = new File(dir, `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}${PART_SUFFIX}`);
  try {
    await downloadResuming(uri, headers, tmp, info?.size ?? null, opts);
    if (info?.size != null && tmp.size !== info.size) throw new Error(m.file_card_failed());
    throwIfAborted(opts.signal);
    tmp.moveSync(target, { overwrite: true });
  } catch (e) {
    dropPart(tmp);
    throw e;
  }
  writeEntry(key, { etag: info?.etag ?? null, size: target.size, used: Date.now() });
  evictQuietly(key);
  return target;
}

interface Stored { key: string; file: File; entry: Entry | null }

function stored(): Stored[] {
  const dir = new Directory(Paths.cache, MEDIA_DIR);
  if (!dir.exists) return [];
  return dir.list().filter((f): f is File => f instanceof File).map((file) => ({ key: file.name, file, entry: readEntry(file.name) }));
}

// Os usados há mais tempo saem primeiro, até caber no teto; o que acabou de ser pedido e o usado há
// menos de dez minutos ficam. Sobra de versão antiga (sem registro) e pedaço abandonado saem também.
export function evict(keep?: string): void {
  for (const d of LEGACY_DIRS) {
    const dir = new Directory(Paths.cache, d);
    if (dir.exists) dir.delete();
  }
  const now = Date.now();
  const alive: Stored[] = [];
  for (const s of stored()) {
    const part = s.key.endsWith(PART_SUFFIX);
    if (part && now - (s.file.modificationTime ?? now) > STALE_PART_MS) s.file.delete();
    else if (!part && !s.entry) s.file.delete();
    else if (!part) alive.push(s);
  }
  for (const k of index.getAllKeys()) if (!alive.some((s) => s.key === k)) index.remove(k);
  let total = alive.reduce((n, s) => n + s.file.size, 0);
  for (const s of alive.sort((a, b) => a.entry!.used - b.entry!.used)) {
    if (total <= MEDIA_CACHE_LIMIT) break;
    if (s.key === keep || now - s.entry!.used < RECENT_USE_MS) continue;
    total -= s.file.size;
    s.file.delete();
    index.remove(s.key);
  }
}

// Cópia guardada da tela "Mídia e arquivos", pela chave de `keptKey`. Mora na pasta do cache: conta no teto e
// sai pela mesma ordem de uso que as outras.
function keptFile(key: string, name: string): File {
  return new File(mediaDir(), `${KEPT_PREFIX}${keyHash(key)}-${safeName(name)}`);
}

/** A cópia já baixada, ou null; também null quando o servidor diz que o arquivo mudou desde o download. */
export function keptCopy(key: string, name: string, now: KeptVersion): File | null {
  try {
    const f = keptFile(key, name);
    const entry = readEntry(f.name);
    // Sem registro a limpeza a apagaria: não vale como cópia.
    if (!f.exists || !entry) return null;
    return keptIsCurrent(entry.version ?? null, f.size, now) ? f : null;
  } catch (e) {
    diagCache('kept_read_failed', e);
    return null;
  }
}

/** Baixa para a cópia guardada pelo mesmo download do cache (progresso, cancelamento pelo `signal`,
 *  retomada no Android). A versão gravada é a dos bytes (`downloadedVersion`): o download do
 *  expo-file-system não devolve os cabeçalhos da resposta, então a medida do começo e a do fim são
 *  lidas e comparadas. */
export async function downloadKept(
  key: string, uri: string, headers: Record<string, string> | undefined, name: string,
  version: () => KeptVersion, opts: DownloadOptions = {},
): Promise<File> {
  const start = version();
  const target = keptFile(key, name);
  const tmp = new File(mediaDir(), `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}${PART_SUFFIX}`);
  try {
    await downloadResuming(uri, headers, tmp, start.size, opts);
    throwIfAborted(opts.signal);
    tmp.moveSync(target, { overwrite: true });
  } catch (e) {
    dropPart(tmp);
    throw e;
  }
  writeEntry(target.name, { etag: null, size: target.size, used: Date.now(), version: downloadedVersion(start, version()) });
  evictQuietly(target.name);
  return target;
}

export type CacheKind = 'image' | 'video' | 'audio' | 'document';

export interface CacheUsage {
  byKind: Record<CacheKind, number>;
  files: number;
  /** Bytes dos anexos baixados (o teto é `MEDIA_CACHE_LIMIT`). */
  total: number;
  /** Bytes do cache de reprodução do expo-video; null = o módulo não respondeu. */
  video: number | null;
}

// O nome guardado termina no nome original, então a extensão diz o tipo.
const kindOf = (key: string): CacheKind => {
  const k = fileKind(key);
  return k === 'image' || k === 'video' || k === 'audio' ? k : 'document';
};

export async function cacheUsage(): Promise<CacheUsage> {
  const byKind: Record<CacheKind, number> = { image: 0, video: 0, audio: 0, document: 0 };
  let files = 0;
  for (const s of stored()) {
    if (s.key.endsWith(PART_SUFFIX)) continue;
    byKind[kindOf(s.key)] += s.file.size;
    files++;
  }
  for (const d of SIDE_DIRS) {
    const dir = new Directory(Paths.cache, d);
    if (dir.exists) byKind.document += dir.size ?? 0;
  }
  let video: number | null = null;
  try {
    const { getCurrentVideoCacheSize } = await import('expo-video');
    video = getCurrentVideoCacheSize();
  } catch (e) {
    // Sem o módulo nativo a linha mostra "—".
    diagCache('video_cache_size_failed', e);
  }
  return { byKind, files, total: byKind.image + byKind.video + byKind.audio + byKind.document, video };
}

/**
 * Apaga as cópias baixadas e o cache de reprodução. Falha ao apagar arquivo é exceção. O cache do
 * expo-video só se apaga sem nenhum player vivo: false = os arquivos saíram, ele ficou.
 */
export async function clearMediaCache(): Promise<boolean> {
  for (const d of [MEDIA_DIR, ...SIDE_DIRS, ...LEGACY_DIRS]) {
    const dir = new Directory(Paths.cache, d);
    if (dir.exists) dir.delete();
  }
  index.clearAll();
  try {
    const { clearVideoCacheAsync } = await import('expo-video');
    await clearVideoCacheAsync();
    return true;
  } catch (e) {
    diagCache('video_cache_clear_failed', e);
    return false;
  }
}

const MIME: Record<string, string> = {
  doc: 'application/msword', docx: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
  odt: 'application/vnd.oasis.opendocument.text', rtf: 'application/rtf',
  xls: 'application/vnd.ms-excel', xlsx: 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet',
  ods: 'application/vnd.oasis.opendocument.spreadsheet',
  ppt: 'application/vnd.ms-powerpoint', pptx: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
  odp: 'application/vnd.oasis.opendocument.presentation',
  csv: 'text/csv', tsv: 'text/tab-separated-values', txt: 'text/plain',
  zip: 'application/zip', tar: 'application/x-tar', gz: 'application/gzip', tgz: 'application/gzip',
  '7z': 'application/x-7z-compressed', rar: 'application/vnd.rar',
  pdf: 'application/pdf', html: 'text/html', htm: 'text/html',
  png: 'image/png', jpg: 'image/jpeg', jpeg: 'image/jpeg', gif: 'image/gif', webp: 'image/webp', svg: 'image/svg+xml',
  avif: 'image/avif', bmp: 'image/bmp',
  mp4: 'video/mp4', mov: 'video/quicktime', webm: 'video/webm', mkv: 'video/x-matroska', m4v: 'video/x-m4v', avi: 'video/x-msvideo',
  mp3: 'audio/mpeg', wav: 'audio/wav', m4a: 'audio/mp4', ogg: 'audio/ogg', flac: 'audio/flac', aac: 'audio/aac',
};

// O iOS escolhe as ações da folha pelo tipo: sem ele um .docx não oferece o Pages nem "Salvar em Arquivos" com ícone certo.
const UTI: Record<string, string> = {
  doc: 'com.microsoft.word.doc', docx: 'org.openxmlformats.wordprocessingml.document',
  xls: 'com.microsoft.excel.xls', xlsx: 'org.openxmlformats.spreadsheetml.sheet',
  ppt: 'com.microsoft.powerpoint.ppt', pptx: 'org.openxmlformats.presentationml.presentation',
  rtf: 'public.rtf', csv: 'public.comma-separated-values-text', tsv: 'public.tab-separated-values-text', txt: 'public.plain-text',
  zip: 'public.zip-archive', gz: 'org.gnu.gnu-zip-archive', tgz: 'org.gnu.gnu-zip-archive', tar: 'public.tar-archive',
  pdf: 'com.adobe.pdf', html: 'public.html', htm: 'public.html',
  png: 'public.png', jpg: 'public.jpeg', jpeg: 'public.jpeg', gif: 'com.compuserve.gif', webp: 'org.webmproject.webp',
  svg: 'public.svg-image', bmp: 'com.microsoft.bmp', avif: 'public.avif',
  mp4: 'public.mpeg-4', mov: 'com.apple.quicktime-movie', m4v: 'com.apple.m4v-video', avi: 'public.avi',
  mp3: 'public.mp3', wav: 'com.microsoft.waveform-audio', m4a: 'com.apple.m4a-audio', aac: 'public.aac-audio',
};

const extOf = (name: string) => name.split('.').pop()?.toLowerCase() ?? '';

export function mimeType(name: string): string {
  return MIME[extOf(name)] ?? '*/*';
}

/** O expo-sharing manda arquivo nos dois sistemas; o iOS traz "Salvar em Arquivos" na folha. */
export const canShareFile = Platform.OS === 'ios' || Platform.OS === 'android';

async function shareLocal(f: File, name: string): Promise<void> {
  await Sharing.shareAsync(f.uri, { mimeType: mimeType(name), UTI: UTI[extOf(name)] ?? 'public.data', dialogTitle: name });
}

export async function shareFile(uri: string, headers: Record<string, string> | undefined, name: string, opts?: DownloadOptions): Promise<void> {
  const f = await localCopy(uri, headers, name, opts);
  throwIfAborted(opts?.signal);
  await shareLocal(f, name);
}

/** Android: entrega a cópia ao app que lê o tipo ("Abrir com"). 'no-app' = nenhum abre; compartilhar continua valendo. */
export async function openWith(uri: string, headers: Record<string, string> | undefined, name: string, opts?: DownloadOptions): Promise<'opened' | 'empty' | 'no-app'> {
  const f = await localCopy(uri, headers, name, opts);
  if (f.size === 0) return 'empty';
  throwIfAborted(opts?.signal);
  return openLocal(f, name);
}

async function openLocal(f: File, name: string): Promise<'opened' | 'no-app'> {
  try {
    await startActivityAsync('android.intent.action.VIEW', { data: f.contentUri, type: mimeType(name), flags: FLAG_GRANT_READ_URI_PERMISSION });
  } catch (e) {
    // Sem app que abra o tipo (zip, rar…): tentar de novo não resolve.
    if (String(e instanceof Error ? e.message : e).includes('ActivityNotFoundException')) return 'no-app';
    throw e;
  }
  return 'opened';
}

// Documento citado: o iOS entrega pela folha do sistema (Salvar em Arquivos, Abrir em…), que não diz
// para onde o arquivo foi, então volta 'shared'; o Android abre no app que lê o tipo. 'empty' = o
// servidor mandou 0 bytes e não há o que entregar.
export async function deliverFile(uri: string, headers: Record<string, string> | undefined, name: string, opts?: DownloadOptions): Promise<'opened' | 'shared' | 'empty' | 'no-app'> {
  if (Platform.OS !== 'ios') return openWith(uri, headers, name, opts);
  const f = await localCopy(uri, headers, name, opts);
  if (f.size === 0) return 'empty';
  throwIfAborted(opts?.signal);
  await shareLocal(f, name);
  return 'shared';
}

// Arquivo já no aparelho (baixado na tela "Mídia e arquivos"): entrega sem rede. Conta como uso: a limpeza não
// apaga o que a folha ou o app que o abriu ainda podem estar lendo.
export async function deliverLocalFile(f: File, name: string): Promise<'opened' | 'shared' | 'empty' | 'no-app'> {
  if (f.size === 0) return 'empty';
  const entry = readEntry(f.name);
  if (entry) writeEntry(f.name, { ...entry, used: Date.now() });
  if (Platform.OS !== 'ios') return openLocal(f, name);
  await shareLocal(f, name);
  return 'shared';
}

export async function openPdfFile(uri: string, headers: Record<string, string> | undefined, name: string, isMounted: () => boolean): Promise<void> {
  let file: File;
  try {
    const cached = await localCopy(uri, headers, name);
    if (!isMounted()) return;
    const dir = new Directory(Paths.cache, 'pdf-reader', cached.name);
    dir.create({ idempotent: true, intermediates: true });
    const safe = safeName(name);
    const filename = !safe || safe === '.' || safe === '..' ? 'documento.pdf' : safe;
    file = new File(dir, filename);
    // Cópia, não mover: a guardada continua servindo à próxima abertura.
    cached.copySync(file, { overwrite: true });
  } catch (e) {
    throw new Error(m.native_pdf_download_failed({ reason: e instanceof Error ? e.message : String(e) }));
  }
  try {
    await startActivityAsync('android.intent.action.VIEW', {
      data: file.contentUri, type: 'application/pdf', flags: FLAG_GRANT_READ_URI_PERMISSION,
    });
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    if (reason.includes('ActivityNotFoundException')) throw new Error(m.arq_pdf_sem_leitor());
    throw new Error(m.native_pdf_reader_failed({ reason }));
  }
}

// Imagem ou vídeo do visor: no iOS a folha já traz "Salvar imagem"/"Salvar vídeo"; no Android vai
// direto para a galeria. Devolve true quando gravou na galeria (o iOS não sabe o que a pessoa escolheu).
export async function saveImage(uri: string, headers: Record<string, string> | undefined, name: string, opts?: DownloadOptions): Promise<boolean> {
  const f = await localCopy(uri, headers, name, opts);
  throwIfAborted(opts?.signal);
  if (Platform.OS === 'ios') {
    await shareLocal(f, name);
    return false;
  }
  // Carregado só aqui: é o único uso, e só no Android.
  const { Asset, requestPermissionsAsync } = await import('expo-media-library');
  const perm = await requestPermissionsAsync(true);
  if (!perm.granted) throw new Error(m.composer_sem_acesso_fotos());
  await Asset.create(f.uri);
  return true;
}

// Texto que já está na tela vira arquivo do cache e vai pela mesma folha: sem baixar de novo nem depender da rede.
// `name` já vem sem o que o sistema de arquivos recusa (`pageFileName`). Pasta própria: em `chat-media` o nome podia
// coincidir com o de um anexo baixado (`hash-nome`) e sobrescrevê-lo.
export async function shareTextFile(text: string, name: string): Promise<void> {
  const dir = new Directory(Paths.cache, 'chat-page-source');
  dir.create({ idempotent: true, intermediates: true });
  const f = new File(dir, name);
  f.write(text);
  await shareLocal(f, name);
}
