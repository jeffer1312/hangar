import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

// Sistema de arquivos em memória: só o tamanho e a data de cada caminho, que é o que o cache lê.
type Job = (task: FakeTask) => Promise<string | null>;
interface FakeTask {
  url: string; dest: string; opts: { headers?: Record<string, string>; onProgress?: (p: { bytesWritten: number; totalBytes: number }) => void; signal?: AbortSignal };
  state: string; resumeData?: string; released: boolean;
}
const fs = vi.hoisted(() => ({
  files: new Map<string, { size: number; mtime: number }>(),
  dirs: new Set<string>(),
  jobs: [] as Job[],
  tasks: [] as FakeTask[],
  restored: [] as { state: Record<string, unknown>; opts: unknown }[],
  store: new Map<string, string>(),
  shared: [] as { url: string; options: Record<string, string> }[],
  intent: vi.fn(),
  clearVideo: vi.fn(async () => {}),
  platform: { OS: 'android' as 'android' | 'ios' },
}));

vi.mock('expo-file-system', () => {
  const path = (parts: unknown[]) => parts.map((p) => (typeof p === 'string' ? p : (p as { uri: string }).uri)).join('/');
  class Directory {
    uri: string;
    constructor(...parts: unknown[]) { this.uri = path(parts); }
    get name() { return this.uri.split('/').pop()!; }
    create() { fs.dirs.add(this.uri); }
    get exists() { return fs.dirs.has(this.uri) || [...fs.files.keys()].some((k) => k.startsWith(`${this.uri}/`)); }
    get size() { return [...fs.files].filter(([k]) => k.startsWith(`${this.uri}/`)).reduce((n, [, f]) => n + f.size, 0); }
    list() { return [...fs.files.keys()].filter((k) => k.startsWith(`${this.uri}/`) && !k.slice(this.uri.length + 1).includes('/')).map((k) => new File(k)); }
    delete() { for (const k of [...fs.files.keys()]) if (k.startsWith(`${this.uri}/`)) fs.files.delete(k); fs.dirs.delete(this.uri); }
  }
  class File {
    uri: string;
    constructor(...parts: unknown[]) { this.uri = path(parts); }
    get name() { return this.uri.split('/').pop()!; }
    get exists() { return fs.files.has(this.uri); }
    get size() { return fs.files.get(this.uri)?.size ?? 0; }
    get modificationTime() { return fs.files.get(this.uri)?.mtime ?? null; }
    get contentUri() { return `content://${this.uri}`; }
    delete() { fs.files.delete(this.uri); }
    write(text: string) { fs.files.set(this.uri, { size: text.length, mtime: Date.now() }); }
    moveSync(dest: File) { fs.files.set(dest.uri, fs.files.get(this.uri)!); fs.files.delete(this.uri); this.uri = dest.uri; }
    copySync(dest: File) { fs.files.set(dest.uri, { ...fs.files.get(this.uri)! }); }
    static createDownloadTask(url: string, dest: File, opts: FakeTask['opts']) { return makeTask(url, dest.uri, opts); }
  }
  function makeTask(url: string, dest: string, opts: FakeTask['opts'], resumeData?: string) {
    const t: FakeTask = { url, dest, opts, state: resumeData ? 'paused' : 'idle', resumeData, released: false };
    fs.tasks.push(t);
    const run = async () => {
      t.state = 'active';
      const job = fs.jobs.shift();
      if (!job) throw new Error('no job queued');
      try {
        const r = await job(t);
        t.state = r ? 'completed' : 'paused';
        return r ? new File(r) : null;
      } catch (e) {
        t.state = 'error';
        throw e;
      }
    };
    return Object.assign(t, { downloadAsync: run, resumeAsync: run, release: () => { t.released = true; } });
  }
  const DownloadTask = {
    fromSavable(state: { url: string; fileUri: string; resumeData: string }, opts: unknown) {
      fs.restored.push({ state, opts });
      return makeTask(state.url, state.fileUri, { ...(opts as object), headers: (state as { headers?: Record<string, string> }).headers }, state.resumeData);
    },
  };
  return { Directory, File, DownloadTask, Paths: { cache: new Directory('cache') } };
});
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (k: string) => fs.store.get(k),
    set: (k: string, v: string) => { fs.store.set(k, v); },
    remove: (k: string) => fs.store.delete(k),
    getAllKeys: () => [...fs.store.keys()],
    clearAll: () => fs.store.clear(),
  }),
}));
vi.mock('react-native', () => ({ Platform: fs.platform }));
vi.mock('expo-intent-launcher', () => ({ startActivityAsync: fs.intent }));
vi.mock('expo-sharing', () => ({ shareAsync: async (url: string, options: Record<string, string>) => { fs.shared.push({ url, options }); } }));
vi.mock('expo-video', () => ({ clearVideoCacheAsync: fs.clearVideo, getCurrentVideoCacheSize: () => 7 }));
vi.mock('../../paraglide/messages', () => ({
  file_card_failed: () => 'failed',
  native_pdf_download_failed: ({ reason }: { reason: string }) => reason,
  native_pdf_reader_failed: ({ reason }: { reason: string }) => reason,
  arq_pdf_sem_leitor: () => 'no-reader',
  composer_sem_acesso_fotos: () => 'no-photos',
}));

import { cacheUsage, clearMediaCache, deliverFile, evict, isAbort, isMissing, localCopy, MEDIA_CACHE_LIMIT } from './mediaCache';

// Guardado = arquivo na pasta e registro no índice; o nome guardado termina no nome original.
const isCached = (_uri: string, name: string) => {
  const key = [...fs.files.keys()].find((k) => k.startsWith('cache/chat-media/') && k.endsWith(`-${name}`));
  return !!key && fs.store.has(key.split('/').pop()!);
};

const URI = 'https://srv/api/sessions/s/file?path=%2Ftmp%2Frel.docx';
const H = { Authorization: 'Bearer t' };

// Resposta do pedido de um byte: ETag e tamanho total pelo Content-Range.
function probeReply(etag: string, size: number, status = 206) {
  return { status, ok: status >= 200 && status < 300, headers: new Headers({ etag, 'content-range': `bytes 0-0/${size}` }) };
}
// Download que grava `size` bytes no destino e anuncia o progresso.
const writes = (size: number): Job => async (t) => {
  t.opts.onProgress?.({ bytesWritten: size, totalBytes: size });
  fs.files.set(t.dest, { size, mtime: Date.now() });
  return t.dest;
};

const fetchMock = vi.fn();
beforeEach(() => {
  fs.files.clear(); fs.dirs.clear(); fs.jobs.length = 0; fs.tasks.length = 0; fs.restored.length = 0;
  fs.store.clear(); fs.shared.length = 0; fs.intent.mockReset(); fs.clearVideo.mockReset(); fs.platform.OS = 'android';
  fetchMock.mockReset();
  vi.stubGlobal('fetch', fetchMock);
});
afterEach(() => { vi.unstubAllGlobals(); vi.useRealTimers(); });

describe('localCopy', () => {
  it('baixa uma vez com progresso e reaproveita quando o arquivo não mudou no servidor', async () => {
    fetchMock.mockResolvedValue(probeReply('"a"', 100));
    fs.jobs.push(writes(100));
    const progress = vi.fn();
    const f = await localCopy(URI, H, 'rel.docx', { onProgress: progress });
    expect(f.size).toBe(100);
    expect(progress).toHaveBeenCalledWith({ written: 100, total: 100 });
    expect(fs.tasks[0].opts.headers).toEqual(H);
    expect(fetchMock.mock.calls[0][1].headers).toMatchObject({ ...H, Range: 'bytes=0-0' });
    expect(isCached(URI, 'rel.docx')).toBe(true);
    const again = await localCopy(URI, H, 'rel.docx');
    expect(again.uri).toBe(f.uri);
    expect(fs.tasks).toHaveLength(1);
    // Nenhum `.part` sobra na pasta.
    expect([...fs.files.keys()].filter((k) => k.endsWith('.part'))).toEqual([]);
  });

  it('baixa de novo quando o ETag mudou (arquivo regerado no mesmo caminho)', async () => {
    fetchMock.mockResolvedValueOnce(probeReply('"a"', 100)).mockResolvedValueOnce(probeReply('"b"', 120));
    fs.jobs.push(writes(100), writes(120));
    await localCopy(URI, H, 'rel.docx');
    const f = await localCopy(URI, H, 'rel.docx');
    expect(fs.tasks).toHaveLength(2);
    expect(f.size).toBe(120);
  });

  it('sem rede entrega a cópia guardada; sem cópia, a falha aparece', async () => {
    fetchMock.mockResolvedValueOnce(probeReply('"a"', 100));
    fs.jobs.push(writes(100));
    await localCopy(URI, H, 'rel.docx');
    fetchMock.mockRejectedValue(new TypeError('Network request failed'));
    const f = await localCopy(URI, H, 'rel.docx');
    expect(f.size).toBe(100);
    expect(fs.tasks).toHaveLength(1);
    fs.jobs.push(async () => { throw new Error('Unable to resolve host'); });
    await expect(localCopy('https://srv/outro.zip', H, 'outro.zip')).rejects.toThrow('Unable to resolve host');
  });

  it('404 sem cópia vira "não encontrado"; com cópia, a guardada continua valendo', async () => {
    fetchMock.mockResolvedValue({ status: 404, ok: false, headers: new Headers() });
    const err = await localCopy(URI, H, 'rel.docx').catch((e: unknown) => e);
    expect(isMissing(err)).toBe(true);
    expect(fs.tasks).toHaveLength(0);
    fetchMock.mockResolvedValueOnce(probeReply('"a"', 100));
    fs.jobs.push(writes(100));
    await localCopy(URI, H, 'rel.docx');
    fetchMock.mockResolvedValue({ status: 404, ok: false, headers: new Headers() });
    expect((await localCopy(URI, H, 'rel.docx')).size).toBe(100);
  });

  it('download que chega com tamanho diferente do anunciado não vira a cópia guardada', async () => {
    fetchMock.mockResolvedValue(probeReply('"a"', 100));
    fs.jobs.push(writes(60));
    await expect(localCopy(URI, H, 'rel.docx')).rejects.toThrow('failed');
    expect(isCached(URI, 'rel.docx')).toBe(false);
    expect([...fs.files.keys()]).toEqual([]);
  });

  it('Android retoma do pedaço gravado depois da queda de rede', async () => {
    vi.useFakeTimers();
    fetchMock.mockResolvedValue(probeReply('"a"', 100));
    fs.jobs.push(async (t) => {
      fs.files.set(t.dest, { size: 40, mtime: Date.now() });
      throw new Error('Download failed: Software caused connection abort');
    });
    fs.jobs.push(async (t) => {
      expect(t.resumeData).toBe('40');
      fs.files.set(t.dest, { size: 100, mtime: Date.now() });
      return t.dest;
    });
    const p = localCopy(URI, H, 'rel.docx');
    // Passa da espera de 2 s antes da retomada.
    await vi.advanceTimersByTimeAsync(3000);
    const f = await p;
    expect(f.size).toBe(100);
    expect(fs.restored).toHaveLength(1);
    expect(fs.restored[0].state).toMatchObject({ url: URI, isDirectory: false, headers: H, resumeData: '40' });
    expect(fs.tasks.every((t) => t.released)).toBe(true);
  });

  it('erro HTTP no meio do download não se retoma', async () => {
    fetchMock.mockResolvedValue(probeReply('"a"', 100));
    fs.jobs.push(async (t) => { fs.files.set(t.dest, { size: 10, mtime: 0 }); throw new Error('HTTP 500'); });
    await expect(localCopy(URI, H, 'rel.docx')).rejects.toThrow('HTTP 500');
    expect(fs.restored).toHaveLength(0);
  });

  it('cancelar interrompe sem tentar de novo e sem deixar arquivo', async () => {
    fetchMock.mockResolvedValue(probeReply('"a"', 100));
    const ctl = new AbortController();
    fs.jobs.push(async (t) => {
      fs.files.set(t.dest, { size: 10, mtime: 0 });
      ctl.abort();
      const e = new Error('aborted');
      e.name = 'AbortError';
      throw e;
    });
    const err = await localCopy(URI, H, 'rel.docx', { signal: ctl.signal }).catch((e: unknown) => e);
    expect(isAbort(err)).toBe(true);
    expect(fs.restored).toHaveLength(0);
    expect([...fs.files.keys()]).toEqual([]);
  });
});

describe('teto e limpeza', () => {
  it('passando do teto, sai o usado há mais tempo e fica o que acabou de chegar', async () => {
    const big = Math.floor(MEDIA_CACHE_LIMIT / 2.5);
    const names = ['a.mp4', 'b.mp4', 'c.mp4'];
    for (const [i, name] of names.entries()) {
      vi.setSystemTime(1000 + i * 1000);
      fetchMock.mockResolvedValueOnce(probeReply(`"${name}"`, big));
      fs.jobs.push(writes(big));
      await localCopy(`https://srv/${name}`, H, name);
    }
    vi.useRealTimers();
    expect(isCached('https://srv/a.mp4', 'a.mp4')).toBe(false);
    expect(isCached('https://srv/b.mp4', 'b.mp4')).toBe(true);
    expect(isCached('https://srv/c.mp4', 'c.mp4')).toBe(true);
  });

  it('sobra sem registro (cópia de versão antiga) sai na varredura', () => {
    fs.files.set('cache/chat-media/abc-velho.docx', { size: 5, mtime: 0 });
    evict();
    expect(fs.files.size).toBe(0);
  });

  it('mede por tipo, conta o cache dos vídeos e limpa tudo', async () => {
    for (const [name, size] of [['f.png', 10], ['v.mp4', 20], ['s.m4a', 30], ['r.docx', 40]] as const) {
      fetchMock.mockResolvedValueOnce(probeReply(`"${name}"`, size));
      fs.jobs.push(writes(size));
      await localCopy(`https://srv/${name}`, H, name);
    }
    const u = await cacheUsage();
    expect(u.byKind).toEqual({ image: 10, video: 20, audio: 30, document: 40 });
    expect(u.total).toBe(100);
    expect(u.video).toBe(7);
    expect(await clearMediaCache()).toBe(true);
    expect(fs.files.size).toBe(0);
    expect(fs.store.size).toBe(0);
    expect(fs.clearVideo).toHaveBeenCalledTimes(1);
  });

  it('com player vivo os arquivos saem e o cache do vídeo fica, dito em false', async () => {
    fs.clearVideo.mockRejectedValueOnce(new Error('players exist'));
    fs.files.set('cache/chat-media/x-a.png', { size: 1, mtime: 0 });
    expect(await clearMediaCache()).toBe(false);
    expect(fs.files.size).toBe(0);
  });
});

describe('entrega', () => {
  it('iOS entrega pela folha do sistema com o tipo do arquivo', async () => {
    fs.platform.OS = 'ios';
    fetchMock.mockResolvedValue(probeReply('"a"', 100));
    fs.jobs.push(writes(100));
    expect(await deliverFile(URI, H, 'rel.docx')).toBe('shared');
    expect(fs.shared).toHaveLength(1);
    expect(fs.shared[0].options).toMatchObject({
      mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document',
      UTI: 'org.openxmlformats.wordprocessingml.document',
    });
    expect(fs.intent).not.toHaveBeenCalled();
  });

  it('Android abre no app que lê o tipo e diz quando nenhum abre', async () => {
    fetchMock.mockResolvedValue(probeReply('"a"', 100));
    fs.jobs.push(writes(100));
    fs.intent.mockRejectedValueOnce(new Error('ActivityNotFoundException'));
    expect(await deliverFile(URI, H, 'rel.docx')).toBe('no-app');
    expect(fs.intent.mock.calls[0][1]).toMatchObject({ type: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' });
    expect(fs.shared).toHaveLength(0);
  });

  it('arquivo vazio não é entregue', async () => {
    fetchMock.mockResolvedValue({ status: 416, ok: false, headers: new Headers({ etag: '"e"', 'content-range': 'bytes */0' }) });
    fs.jobs.push(writes(0));
    expect(await deliverFile(URI, H, 'rel.docx')).toBe('empty');
  });
});
