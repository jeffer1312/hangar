// @vitest-environment happy-dom
import { beforeEach, describe, expect, it, vi } from 'vitest';
import * as m from '../paraglide/messages';
import { transcribeUploaded, uploadFile } from '@hangar/core';

const lista = { epoca: 0, sessoes: [{ name: 'x', jsonl: 'j1' }] as { name: string; jsonl: string | null }[] };
vi.mock('./sessionsStore.svelte', () => ({
  sessionsStore: {
    epoca: () => lista.epoca,
    retain: vi.fn(),
    release: vi.fn(),
    sessionsForServer: () => lista.sessoes,
  },
}));
vi.mock('@hangar/core', async (orig) => ({
  ...(await orig<typeof import('@hangar/core')>()),
  uploadFile: vi.fn(),
  transcribeUploaded: vi.fn(),
}));

const { dictations, draftStorageKey, dictationBarKey } = await import('./dictationStore.svelte');
const { sessionsStore } = await import('./sessionsStore.svelte');
const flush = () => new Promise((r) => setTimeout(r, 0));
const CAMINHO = '/home/u/.hangar/uploads/p/x/gravacao-1.webm';
const RASCUNHO = draftStorageKey('a', 'x');
const BARRA = dictationBarKey('a', 'x');
const PENDENTE = 'cp-ditado-pendente:a::x';
const audio = () => new File(['a'], 'gravacao-1.webm', { type: 'audio/webm' });
const iniciar = (ditado = true) => dictations.start({
  serverId: 'a', name: 'x', jsonl: 'j1', server: undefined, file: audio(), opts: { ditado },
});

beforeEach(() => {
  dictations._resetForTests();
  localStorage.clear();
  vi.clearAllMocks();
  lista.epoca = 0;
  lista.sessoes = [{ name: 'x', jsonl: 'j1' }];
  vi.mocked(uploadFile).mockResolvedValue({ path: CAMINHO });
  vi.spyOn(console, 'error').mockImplementation(() => {});
});

describe('ditado por sessão', () => {
  it('sobe o áudio, guarda o caminho na hora e transcreve pelo nome solto', async () => {
    vi.mocked(transcribeUploaded).mockReturnValue(new Promise(() => {}));
    iniciar();
    await flush();
    expect(uploadFile).toHaveBeenCalledWith('x', expect.any(File), undefined, undefined, { audioOnly: true });
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'inflight', path: CAMINHO, arquivo: 'gravacao-1.webm' });
    expect(JSON.parse(localStorage.getItem(PENDENTE)!)).toMatchObject({ path: CAMINHO, jsonl: 'j1', error: null });
    expect(transcribeUploaded).toHaveBeenCalledWith('x', 'gravacao-1.webm', { limpar: true, estilo: undefined }, undefined);
  });

  it('trocou de sessão durante a transcrição: o texto vai pro fim do rascunho da origem', async () => {
    localStorage.setItem(RASCUNHO, JSON.stringify({ text: 'antes', jsonl: 'j1' }));
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'olá mundo', raw: 'ola mundo' });
    iniciar();
    await flush();
    expect(JSON.parse(localStorage.getItem(RASCUNHO)!)).toEqual({ text: 'antes olá mundo', jsonl: 'j1' });
    expect(JSON.parse(localStorage.getItem(BARRA)!)).toMatchObject({
      arquivo: 'gravacao-1.webm', before: 'antes ', after: '', raw: 'ola mundo', jsonl: 'j1',
    });
    expect(localStorage.getItem(PENDENTE)).toBeNull();
    expect(dictations.get('a', 'x')?.stored).toBe(true);
    expect(sessionsStore.retain).toHaveBeenCalledOnce();
    expect(sessionsStore.release).toHaveBeenCalledOnce();
  });

  it('rascunho na chave antiga (só o nome) é movido para a do servidor', async () => {
    localStorage.setItem('cp-draft:x', JSON.stringify({ text: 'antes', jsonl: 'j1' }));
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi' });
    iniciar();
    await flush();
    expect(localStorage.getItem('cp-draft:x')).toBeNull();
    expect(JSON.parse(localStorage.getItem(RASCUNHO)!).text).toBe('antes oi');
  });

  it('/clear no meio: grava com o transcript novo, para o Chat não descartar', async () => {
    lista.sessoes = [{ name: 'x', jsonl: 'j2' }];
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi' });
    iniciar();
    await flush();
    expect(JSON.parse(localStorage.getItem(RASCUNHO)!)).toEqual({ text: 'oi', jsonl: 'j2' });
  });

  it('rascunho guardado de outro transcript não é sobrescrito: o texto espera a conversa abrir', async () => {
    localStorage.setItem(RASCUNHO, JSON.stringify({ text: 'de outro', jsonl: 'j0' }));
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi' });
    iniciar();
    await flush();
    expect(JSON.parse(localStorage.getItem(RASCUNHO)!).text).toBe('de outro');
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'ready' });
    expect(dictations.get('a', 'x')?.stored).toBeUndefined();
    const deliver = vi.fn();
    dictations.receive('a', 'x', { deliver });
    await flush();
    expect(deliver).toHaveBeenCalledWith(expect.objectContaining({ result: expect.objectContaining({ text: 'oi' }) }));
  });

  it('áudio anexado terminando fora da tela apaga a barra antiga', async () => {
    localStorage.setItem(BARRA, JSON.stringify({ arquivo: 'velho.webm', raw: 'x', before: '', after: ' fim' }));
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi' });
    iniciar(false);
    await flush();
    expect(localStorage.getItem(BARRA)).toBeNull();
    expect(transcribeUploaded).toHaveBeenCalledWith('x', 'gravacao-1.webm', { limpar: false, estilo: undefined }, undefined);
  });

  it('sessão recriada antes do resultado não recebe o texto', async () => {
    vi.mocked(transcribeUploaded).mockImplementation(async () => {
      lista.epoca = 1;
      return { path: CAMINHO, text: 'oi' };
    });
    iniciar();
    await flush();
    expect(localStorage.getItem(RASCUNHO)).toBeNull();
    expect(localStorage.getItem(PENDENTE)).toBeNull();
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'failed', error: m.composer_ditado_sessao_recriada() });
    expect(dictations.get('a', 'x')?.path).toBeUndefined();
    expect(dictations.retry('a', 'x')).toBe(false);
  });

  it('com quem receba montado, entrega a ele e não mexe no rascunho guardado', async () => {
    const deliver = vi.fn();
    dictations.receive('a', 'x', { deliver });
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi' });
    iniciar();
    await flush();
    expect(deliver).toHaveBeenCalledWith(expect.objectContaining({ status: 'ready', arquivo: 'gravacao-1.webm' }));
    expect(localStorage.getItem(RASCUNHO)).toBeNull();
    expect(localStorage.getItem(PENDENTE)).toBeNull();
    expect(dictations.get('a', 'x')).toBeUndefined();
  });

  it('tela montada que ainda não aceita: segura na memória e entrega no redeliver', async () => {
    let pronto = false;
    const deliver = vi.fn();
    dictations.receive('a', 'x', { deliver, accepts: () => pronto });
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi' });
    iniciar();
    await flush();
    expect(deliver).not.toHaveBeenCalled();
    expect(localStorage.getItem(RASCUNHO)).toBeNull();
    pronto = true;
    dictations.redeliver('a', 'x');
    expect(deliver).toHaveBeenCalledOnce();
  });

  it('remontado no meio vê "em voo" e não começa outra', () => {
    vi.mocked(uploadFile).mockReturnValue(new Promise(() => {}));
    expect(iniciar()).toBe('started');
    expect(dictations.get('a', 'x')?.status).toBe('inflight');
    expect(iniciar()).toBe('inflight');
  });

  it('quem monta depois de guardado só recebe o aviso', async () => {
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi', aviso: 'Transcrito pelo B: A caiu' });
    iniciar();
    await flush();
    const deliver = vi.fn();
    const restored = vi.fn();
    dictations.receive('a', 'x', { deliver, restored });
    await flush();
    expect(deliver).not.toHaveBeenCalled();
    expect(restored).toHaveBeenCalledWith(expect.objectContaining({ result: expect.objectContaining({ aviso: 'Transcrito pelo B: A caiu' }) }));
  });

  it('falha da transcrição: o "de novo" no mesmo transcript manda o nome solto, sem subir de novo', async () => {
    vi.mocked(transcribeUploaded).mockRejectedValueOnce(Object.assign(new Error('502: fora'), { status: 502 }));
    iniciar();
    await flush();
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'failed', path: CAMINHO, error: '502: fora' });
    vi.mocked(transcribeUploaded).mockReturnValue(new Promise(() => {}));
    expect(dictations.retry('a', 'x', undefined, 'j1')).toBe(true);
    await flush();
    expect(uploadFile).toHaveBeenCalledOnce();
    expect(transcribeUploaded).toHaveBeenLastCalledWith('x', 'gravacao-1.webm', { limpar: true, estilo: undefined }, undefined);
  });

  it('"de novo" depois de /clear manda o caminho absoluto', async () => {
    vi.mocked(transcribeUploaded).mockRejectedValueOnce(Object.assign(new Error('502: fora'), { status: 502 }));
    iniciar();
    await flush();
    lista.sessoes = [{ name: 'x', jsonl: 'j2' }];
    vi.mocked(transcribeUploaded).mockReturnValue(new Promise(() => {}));
    expect(dictations.retry('a', 'x')).toBe(true);
    await flush();
    expect(uploadFile).toHaveBeenCalledOnce();
    expect(transcribeUploaded).toHaveBeenLastCalledWith('x', CAMINHO, { limpar: true, estilo: undefined }, undefined);
  });

  it('falha do upload: o "de novo" sobe o blob da aba de novo', async () => {
    vi.mocked(uploadFile).mockRejectedValueOnce(new Error('rede'));
    iniciar();
    await flush();
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'failed' });
    expect(dictations.get('a', 'x')?.path).toBeUndefined();
    expect(localStorage.getItem(PENDENTE)).toBeNull();
    vi.mocked(transcribeUploaded).mockReturnValue(new Promise(() => {}));
    expect(dictations.retry('a', 'x')).toBe(true);
    await flush();
    expect(uploadFile).toHaveBeenCalledTimes(2);
  });

  it('falha sobrevive a recarregar e a /clear: o erro e o caminho voltam e o "de novo" funciona', async () => {
    vi.mocked(transcribeUploaded).mockRejectedValueOnce(Object.assign(new Error('502: fora'), { status: 502 }));
    iniciar();
    await flush();
    dictations._resetForTests();   // a memória da aba morreu; o aparelho guardou
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'failed', error: '502: fora', path: CAMINHO });
    vi.mocked(transcribeUploaded).mockReturnValue(new Promise(() => {}));
    expect(dictations.retry('a', 'x', undefined, 'j2')).toBe(true);
    await flush();
    expect(transcribeUploaded).toHaveBeenLastCalledWith('x', CAMINHO, { limpar: true, estilo: undefined }, undefined);
  });

  it('pendente relido some quando a sessão é recriada', () => {
    localStorage.setItem(PENDENTE, JSON.stringify({ arquivo: 'gravacao-1.webm', path: CAMINHO, opts: { ditado: true }, error: '502: fora', jsonl: 'j1' }));
    expect(dictations.get('a', 'x')?.status).toBe('failed');
    lista.epoca = 1;
    expect(dictations.get('a', 'x')).toBeUndefined();
    expect(dictations.retry('a', 'x')).toBe(false);
  });

  it('PWA morta com a transcrição no ar volta como "interrompido"', () => {
    localStorage.setItem(PENDENTE, JSON.stringify({ arquivo: 'gravacao-1.webm', path: CAMINHO, opts: { ditado: true }, error: null, jsonl: 'j1' }));
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'failed', error: m.composer_ditado_interrompido() });
    dictations.clear('a', 'x');
    expect(localStorage.getItem(PENDENTE)).toBeNull();
    expect(dictations.get('a', 'x')).toBeUndefined();
  });

  it('áudio da galeria não substitui a gravação que só existe no aparelho', async () => {
    vi.mocked(uploadFile).mockRejectedValueOnce(new Error('rede'));
    iniciar();
    await flush();
    const falha = dictations.get('a', 'x');
    expect(falha).toMatchObject({ status: 'failed', file: expect.any(File) });
    expect(dictations.start({ serverId: 'a', name: 'x', jsonl: 'j1', server: undefined,
      arquivo: 'outro.webm', opts: { ditado: true } })).toBe('onlyOnDevice');
    expect(dictations.get('a', 'x')).toBe(falha);
    expect(transcribeUploaded).not.toHaveBeenCalled();
  });

  it('gravação nova não substitui a que só existe no aparelho', async () => {
    vi.mocked(uploadFile).mockRejectedValueOnce(new Error('rede'));
    iniciar();
    await flush();
    const falha = dictations.get('a', 'x');
    expect(iniciar()).toBe('onlyOnDevice');
    expect(dictations.get('a', 'x')).toBe(falha);
    expect(uploadFile).toHaveBeenCalledOnce();
  });

  it('resultado ainda não entregue não é substituído por outro áudio', async () => {
    localStorage.setItem(RASCUNHO, JSON.stringify({ text: 'de outro', jsonl: 'j0' }));
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi' });
    iniciar();
    await flush();
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'ready' });
    expect(dictations.start({ serverId: 'a', name: 'x', jsonl: 'j1', server: undefined,
      arquivo: 'outro.webm', opts: { ditado: true } })).toBe('undelivered');
    expect(iniciar()).toBe('undelivered');
    expect(dictations.get('a', 'x')?.result?.text).toBe('oi');
  });

  it('resultado na memória não entra na sessão recriada com o mesmo nome, e avisa', async () => {
    localStorage.setItem(RASCUNHO, JSON.stringify({ text: 'de outro', jsonl: 'j0' }));
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'oi' });
    iniciar();
    await flush();
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'ready' });
    lista.epoca = 1;
    const deliver = vi.fn();
    dictations.receive('a', 'x', { deliver });
    await flush();
    expect(deliver).not.toHaveBeenCalled();
    expect(dictations.get('a', 'x')).toMatchObject({ status: 'failed', error: m.composer_ditado_sessao_recriada() });
    expect(dictations.retry('a', 'x')).toBe(false);
  });

  it('campo que quebra ao inserir: o texto vai pro rascunho e fica à vista no aviso', async () => {
    dictations.receive('a', 'x', { deliver: async () => { throw new Error('quebrou'); } });
    vi.mocked(transcribeUploaded).mockResolvedValue({ path: CAMINHO, text: 'olá mundo' });
    iniciar();
    await flush();
    expect(JSON.parse(localStorage.getItem(RASCUNHO)!).text).toBe('olá mundo');
    const falha = dictations.get('a', 'x');
    expect(falha).toMatchObject({ status: 'failed', path: CAMINHO });
    expect(falha?.error).toContain('olá mundo');
    expect(falha?.error).toContain('quebrou');
    expect(JSON.parse(localStorage.getItem(PENDENTE)!).error).toContain('olá mundo');
  });

  it('estilo e aviso de teto sobrevivem a recarregar: o "de novo" usa o estilo escolhido', async () => {
    vi.mocked(transcribeUploaded).mockRejectedValueOnce(new Error('502: fora'));
    dictations.start({ serverId: 'a', name: 'x', jsonl: 'j1', server: undefined, file: audio(),
      opts: { ditado: true, estilo: 'prosa', avisoTeto: true } });
    await flush();
    dictations._resetForTests();
    expect(dictations.get('a', 'x')?.opts).toMatchObject({ estilo: 'prosa', avisoTeto: true });
    vi.mocked(transcribeUploaded).mockReturnValue(new Promise(() => {}));
    expect(dictations.retry('a', 'x', undefined, 'j1')).toBe(true);
    await flush();
    expect(transcribeUploaded).toHaveBeenLastCalledWith('x', 'gravacao-1.webm', { limpar: true, estilo: 'prosa' }, undefined);
  });

  it('503 de configuração ausente diz onde configurar o serviço', async () => {
    vi.mocked(transcribeUploaded).mockRejectedValue(Object.assign(new Error('x'), { status: 503, code: 'transcription_not_configured' }));
    iniciar();
    await flush();
    expect(dictations.get('a', 'x')?.error).toBe(m.composer_transcription_not_configured());
  });
  it('503 do servidor Rust mostra a falha e preserva o áudio para repetir', async () => {
    vi.mocked(transcribeUploaded).mockRejectedValue(Object.assign(new Error('Servidor Rust indisponível'), { status: 503, code: 'transcription_rust_unavailable' }));
    iniciar();
    await flush();
    expect(dictations.get('a', 'x')?.error).toBe('Servidor Rust indisponível');
    expect(dictations.get('a', 'x')?.file).toBeDefined();
  });
});
