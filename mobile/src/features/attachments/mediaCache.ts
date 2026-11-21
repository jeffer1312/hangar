import { Platform, Share } from 'react-native';
import { Directory, File, Paths } from 'expo-file-system';
import { startActivityAsync } from 'expo-intent-launcher';
import * as m from '../../paraglide/messages';

const FLAG_GRANT_READ_URI_PERMISSION = 1;

// Arquivo do servidor exige o token no header, e nem o player do WebView (pede a mídia em pedaços,
// sem header) nem o "Abrir em…" do sistema o mandam: baixa pro cache e entrega o local.
// Baixa a cada abertura porque o arquivo pode ter sido regerado no mesmo caminho. O download vai
// para um nome temporário e só substitui a cópia ao terminar: no Android ele escreve direto no
// destino, e um download interrompido deixaria o arquivo cortado.
export async function localCopy(uri: string, headers: Record<string, string> | undefined, name: string): Promise<File> {
  const dir = new Directory(Paths.cache, 'chat-media');
  dir.create({ idempotent: true, intermediates: true });
  const target = new File(dir, `${keyHash(uri)}-${name.replace(/[^\w.-]/g, '_')}`);
  const tmp = new File(dir, `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}.part`);
  try {
    await File.downloadFileAsync(uri, tmp, { headers });
    tmp.moveSync(target, { overwrite: true });
  } catch (e) {
    if (tmp.exists) tmp.delete();
    throw e;
  }
  return target;
}

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

// Cópia guardada da tela "Mídia e arquivos", pela chave de `keptKey`.
function keptFile(key: string, name: string): File {
  const dir = new Directory(Paths.cache, 'media-files');
  dir.create({ idempotent: true, intermediates: true });
  return new File(dir, `${keyHash(key)}-${name.replace(/[^\w.-]/g, '_')}`);
}

/** A cópia já baixada, ou null. */
export function keptCopy(key: string, name: string): File | null {
  try {
    const f = keptFile(key, name);
    return f.exists ? f : null;
  } catch {
    return null;
  }
}

/** Baixa para a cópia guardada com progresso (0..1, ou null sem tamanho) e cancelamento pelo `signal`.
 *  Vai para um nome temporário e só substitui a cópia ao terminar, como o `localCopy`. */
export async function downloadKept(
  key: string, uri: string, headers: Record<string, string> | undefined, name: string,
  onProgress: (fraction: number | null) => void, signal: AbortSignal,
): Promise<File> {
  const target = keptFile(key, name);
  const tmp = new File(target.parentDirectory, `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}.part`);
  try {
    const task = File.createDownloadTask(uri, tmp, {
      headers,
      // Fora do fundo: cancelar pela tela precisa do JS vivo, e o arquivo é pedido agora.
      sessionType: 'foreground',
      signal,
      onProgress: ({ bytesWritten, totalBytes }) => onProgress(totalBytes > 0 ? Math.min(1, bytesWritten / totalBytes) : null),
    });
    const done = await task.downloadAsync();
    if (!done) throw new Error('paused');
    tmp.moveSync(target, { overwrite: true });
  } catch (e) {
    if (tmp.exists) tmp.delete();
    throw e;
  }
  return target;
}

export async function openPdfFile(uri: string, headers: Record<string, string> | undefined, name: string, isMounted: () => boolean): Promise<void> {
  let file: File;
  try {
    file = await localCopy(uri, headers, name);
    if (!isMounted()) return;
    const dir = new Directory(Paths.cache, 'pdf-reader', file.name);
    dir.create({ idempotent: true, intermediates: true });
    const safeName = name.replace(/[^\w.-]/g, '_');
    const filename = !safeName || safeName === '.' || safeName === '..' ? 'documento.pdf' : safeName;
    const target = new File(dir, filename);
    file.moveSync(target, { overwrite: true });
    file = target;
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

// Só o iOS compartilha arquivo pelo Share do RN (o Android só manda texto): ali a folha do sistema
// já traz "Salvar em Arquivos" e "Abrir em…", que é o baixar e o abrir fora do web.
export const canShareFile = Platform.OS === 'ios';

export async function shareFile(uri: string, headers: Record<string, string> | undefined, name: string): Promise<void> {
  const f = await localCopy(uri, headers, name);
  await Share.share({ url: f.uri });
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
  // A tela "Mídia e arquivos" também entrega estes ao app do sistema depois de baixar.
  pdf: 'application/pdf', html: 'text/html', htm: 'text/html',
  mp3: 'audio/mpeg', m4a: 'audio/mp4', wav: 'audio/wav', ogg: 'audio/ogg', flac: 'audio/flac', aac: 'audio/aac',
};

export function mimeType(name: string): string {
  return MIME[name.split('.').pop()?.toLowerCase() ?? ''] ?? '*/*';
}

// Documento citado: o iOS entrega pela folha do sistema (Salvar em Arquivos, Abrir em…); o Android não
// compartilha arquivo pelo Share do RN, então abre direto no app que lê o tipo. 'empty' = o servidor
// mandou 0 bytes e não há o que entregar.
// iOS: a folha não diz para onde o arquivo foi (Copiar, Mail, Salvar em Arquivos), então volta 'shared'.
// Android: 'opened' = entregue ao app que lê o tipo; a cópia fica só no cache, não foi baixada.
export async function deliverFile(uri: string, headers: Record<string, string> | undefined, name: string): Promise<'opened' | 'shared' | 'empty' | 'cancelled' | 'no-app'> {
  return deliverLocalFile(await localCopy(uri, headers, name), name);
}

// Arquivo já no aparelho (baixado na tela "Mídia e arquivos"): entrega sem rede.
export async function deliverLocalFile(f: File, name: string): Promise<'opened' | 'shared' | 'empty' | 'cancelled' | 'no-app'> {
  if (f.size === 0) return 'empty';
  if (Platform.OS === 'ios') {
    const r = await Share.share({ url: f.uri });
    return r.action === Share.dismissedAction ? 'cancelled' : 'shared';
  }
  try {
    await startActivityAsync('android.intent.action.VIEW', { data: f.contentUri, type: mimeType(name), flags: FLAG_GRANT_READ_URI_PERMISSION });
  } catch (e) {
    // Sem app que abra o tipo (zip, rar…): tentar de novo não resolve.
    if (String(e instanceof Error ? e.message : e).includes('ActivityNotFoundException')) return 'no-app';
    throw e;
  }
  return 'opened';
}

// Imagem do visor: no iOS a folha já traz "Salvar imagem"; no Android vai direto para a galeria.
// Devolve true quando gravou na galeria (o iOS só abre a folha e não sabe o que a pessoa escolheu).
export async function saveImage(uri: string, headers: Record<string, string> | undefined, name: string): Promise<boolean> {
  const f = await localCopy(uri, headers, name);
  if (Platform.OS === 'ios') {
    await Share.share({ url: f.uri });
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
  await Share.share({ url: f.uri });
}
