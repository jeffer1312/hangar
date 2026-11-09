import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { DictationDraft } from '../../stores/drafts';

const { memory, core } = vi.hoisted(() => ({ memory: new Map<string, string>(),
  core: { transcribe: vi.fn(), upload: vi.fn(), relimpar: vi.fn(), diag: vi.fn() } }));
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (key: string) => memory.get(key),
    set: (key: string, value: string) => { memory.set(key, value); },
    remove: (key: string) => { memory.delete(key); },
  }),
}));
vi.mock('@hangar/core', async (original) => ({
  ...await original<typeof import('@hangar/core')>(),
  transcribeUploadedForServer: core.transcribe,
  uploadFileForServer: core.upload,
  relimparDitado: core.relimpar,
  registrarDiag: core.diag,
}));
vi.mock('../../stores/chat', () => ({ chatStore: () => ({ use: { setState: () => {} } }) }));
// expo-file-system não carrega no node; a galeria só apaga a cópia local do ditado substituído.
vi.mock('../../chat/draftAttachments', () => ({ removeDraftAttachment: vi.fn() }));

import { applyReadyDictation, readDictation, readDraft, recoverDictation, setDictationInFlight, writeDictation, writeDraft } from '../../stores/drafts';
import { dictateUpload, dictationAudio, runDictation } from './dictationRun';
import { removeDraftAttachment } from '../../chat/draftAttachments';
import { useDictationOrganizationStore } from './dictationOrganization';
import type { PiecesHandle, PiecesText } from './livePieces';

const server = { id: 's1', label: 'A', baseUrl: 'https://a.test', token: 't' };
const voice = (patch: Partial<DictationDraft> = {}): DictationDraft => ({
  version: 1, id: 'v1', audio: { uri: 'file:///doc/draft-attachments/1-1.m4a', name: 'd.m4a', mime: 'audio/m4a', kind: 'file' },
  transcript: null, draftRevision: 0, before: '', motivo: 'botao', status: 'pending', text: '', raw: '', issue: '', ...patch,
});

describe('runDictation', () => {
  beforeEach(() => { memory.clear(); core.transcribe.mockReset(); core.upload.mockReset(); });

  it('sobe o áudio e guarda o caminho antes de transcrever; a falha da transcrição o mantém', async () => {
    core.upload.mockResolvedValueOnce({ path: '/up/sess/d.m4a' });
    let savedBefore: string | undefined;
    core.transcribe.mockImplementationOnce(async () => {
      savedBefore = readDictation('s1', 'sess')?.serverPath;
      throw new Error('502: groq');
    });
    writeDictation('s1', 'sess', voice());
    await expect(runDictation(server, 's1', 'sess', voice(), { file: new File(['a'], 'd.m4a') }, { isActive: () => false }))
      .rejects.toMatchObject({ message: '502: groq', lost: false });
    expect(savedBefore).toBe('/up/sess/d.m4a');
    expect(core.upload).toHaveBeenCalledExactlyOnceWith(server, 'sess', expect.any(File), { audioOnly: true });
    expect(core.transcribe).toHaveBeenCalledExactlyOnceWith(server, 'sess', 'd.m4a', { limpar: true, estilo: undefined });
    expect(readDictation('s1', 'sess')).toMatchObject({ status: 'failed', serverPath: '/up/sess/d.m4a', issue: '502: groq' });
  });

  it('"de novo" pelo caminho guardado não sobe outra cópia; sem estilo_aplicado vale o estilo pedido', async () => {
    core.transcribe.mockResolvedValueOnce({ path: '/up/sess/d.m4a', text: 'ok', raw: 'ok cru', aviso: null });
    writeDictation('s1', 'sess', voice({ audio: null, estilo: 'limpar' }));
    await runDictation(server, 's1', 'sess', voice({ audio: null, estilo: 'limpar' }), { arquivo: '/up/sess/d.m4a' }, { isActive: () => false });
    expect(core.upload).not.toHaveBeenCalled();
    expect(core.transcribe).toHaveBeenCalledExactlyOnceWith(server, 'sess', '/up/sess/d.m4a', { limpar: true, estilo: 'limpar' });
    expect(readDictation('s1', 'sess')).toMatchObject({ status: 'ready', serverPath: '/up/sess/d.m4a', applied: 'limpar' });
  });

  it('"de novo" pelo nome solto, sem `path` na resposta, mantém o caminho absoluto guardado', async () => {
    core.transcribe.mockResolvedValueOnce({ path: '', text: 'ok', raw: 'ok', aviso: null });
    const v = voice({ serverPath: '/up/sess/d.m4a' });
    writeDictation('s1', 'sess', v);
    await runDictation(server, 's1', 'sess', v, { arquivo: 'd.m4a' }, { isActive: () => false });
    expect(readDictation('s1', 'sess')?.serverPath).toBe('/up/sess/d.m4a');
  });
});

describe('runDictation com pedaços', () => {
  const deferred = <T,>() => {
    let resolve!: (v: T) => void;
    let reject!: (e: Error) => void;
    const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail; });
    return { promise, resolve, reject };
  };
  const pieces = (text: Promise<Omit<PiecesText, 'count'>>) => {
    const result = text.then((r) => ({ count: 1, ...r }));
    result.catch(() => {});
    return { result, abort: vi.fn<() => void>() } satisfies PiecesHandle;
  };
  const wav = () => new File(['RIFF'], 'ditado.wav', { type: 'audio/wav' });
  const draft = () => writeDraft('s1', 'sess', { version: 1, text: '', revision: 0, transcript: null, attachments: [], submission: null });

  const diag = core.diag;
  beforeEach(() => {
    memory.clear(); core.transcribe.mockReset(); core.upload.mockReset(); core.relimpar.mockReset();
    vi.mocked(removeDraftAttachment).mockClear();
    useDictationOrganizationStore.setState({ byServer: {} });
    diag.mockReset();
  });
  const localCopy = 'file:///doc/draft-attachments/1-1.m4a';

  it('organiza o cru juntado uma vez, sem esperar a subida; o caminho do áudio entra depois do texto', async () => {
    const upload = deferred<{ path: string }>();
    core.upload.mockReturnValueOnce(upload.promise);
    core.relimpar.mockResolvedValueOnce({ text: ' Ela trava. Não sei. ', aviso: 'organização parcial', estilo_aplicado: 'prosa' });
    draft();
    const v = voice({ estilo: 'prosa' });
    writeDictation('s1', 'sess', v);
    const out = await runDictation(server, 's1', 'sess', v,
      { file: wav(), pieces: pieces(Promise.resolve({ raw: 'ela trava. Não sei', aviso: 'reserva usada', provider: 'Groq' })) },
      { isActive: () => true });
    expect(core.relimpar).toHaveBeenCalledExactlyOnceWith('ela trava. Não sei', 'prosa', server, { session: 'sess' });
    expect(core.transcribe).not.toHaveBeenCalled();
    expect(out).toMatchObject({ text: 'Ela trava. Não sei.', raw: 'ela trava. Não sei', aviso: 'reserva usada · organização parcial', applied: 'prosa' });
    expect(readDraft('s1', 'sess')).toMatchObject({ text: 'Ela trava. Não sei.', dictationId: 'v1' });
    expect(readDraft('s1', 'sess')?.recordings ?? []).toEqual([]);
    upload.resolve({ path: '/up/sess/ditado.wav' });
    await out.saved;
    expect(readDraft('s1', 'sess')?.recordings).toEqual([{ id: 'v1', serverPath: '/up/sess/ditado.wav' }]);
    expect(core.upload).toHaveBeenCalledExactlyOnceWith(server, 'sess', expect.any(File), { audioOnly: true });
    // O áudio está no servidor e na gravação do rascunho: a cópia local sobra.
    expect(removeDraftAttachment).toHaveBeenCalledExactlyOnceWith(localCopy);
  });

  it('subida que termina depois de o envio sair: o áudio não entra no rascunho vazio, a cópia sai e fica registrado', async () => {
    const upload = deferred<{ path: string }>();
    core.upload.mockReturnValueOnce(upload.promise);
    core.relimpar.mockResolvedValueOnce({ text: 'ok', aviso: null, estilo_aplicado: 'prosa' });
    draft();
    const v = voice({ estilo: 'prosa' });
    writeDictation('s1', 'sess', v);
    const out = await runDictation(server, 's1', 'sess', v, { file: wav(), pieces: pieces(Promise.resolve({ raw: 'ok', aviso: '' })) },
      { isActive: () => true });
    // Como o envio entregue deixa o rascunho: campo vazio, sem o ditado.
    const { dictationId: _sent, ...sent } = readDraft('s1', 'sess')!;
    writeDraft('s1', 'sess', { ...sent, text: '', revision: sent.revision + 1, recordings: [], submission: null });
    upload.resolve({ path: '/up/sess/ditado.wav' });
    await out.saved;
    expect(readDraft('s1', 'sess')?.recordings).toEqual([]);
    expect(removeDraftAttachment).toHaveBeenCalledExactlyOnceWith(localCopy);
    expect(diag).toHaveBeenCalledWith(expect.objectContaining({ evento: 'dictation.audio_unattached' }));
  });

  it('subida que falha com o texto já no campo: o áudio fica no aparelho, guardado com o ditado e tocável', async () => {
    core.upload.mockRejectedValueOnce(new Error('503: cofre fora'));
    core.relimpar.mockResolvedValueOnce({ text: 'ok', aviso: null, estilo_aplicado: 'prosa' });
    draft();
    const v = voice({ estilo: 'prosa' });
    writeDictation('s1', 'sess', v);
    const out = await runDictation(server, 's1', 'sess', v, { file: wav(), pieces: pieces(Promise.resolve({ raw: 'ok', aviso: '' })) },
      { isActive: () => true });
    expect(readDraft('s1', 'sess')?.text).toBe('ok');
    await expect(out.saved).rejects.toThrow('503: cofre fora');
    const kept = readDictation('s1', 'sess');
    expect(kept).toMatchObject({ id: 'v1', status: 'applied', text: 'ok', audio: { uri: localCopy } });
    expect(dictationAudio(server, 'sess', kept!.audio, kept!.serverPath)?.uri).toBe(localCopy);
    expect(removeDraftAttachment).not.toHaveBeenCalled();
    expect(diag).toHaveBeenCalledWith(expect.objectContaining({ evento: 'dictation.upload_failed' }));
  });

  it('subida que falha com o ditado ainda para recuperar: o áudio fica e o Recuperar ainda funciona', async () => {
    core.upload.mockRejectedValueOnce(new Error('413: grande demais'));
    core.relimpar.mockResolvedValueOnce({ text: 'ok', aviso: null, estilo_aplicado: 'prosa' });
    const v = voice({ estilo: 'prosa' });
    writeDictation('s1', 'sess', v);
    const out = await runDictation(server, 's1', 'sess', v, { file: wav(), pieces: pieces(Promise.resolve({ raw: 'ok', aviso: '' })) },
      { isActive: () => false });
    await expect(out.saved).rejects.toThrow('413: grande demais');
    expect(readDictation('s1', 'sess')).toMatchObject({ status: 'ready', text: 'ok', audio: { uri: localCopy }, issue: '413: grande demais' });
    expect(removeDraftAttachment).not.toHaveBeenCalled();
    expect(recoverDictation('s1', 'sess', 'v1').draft?.text).toBe('ok');
  });

  it('ditado que fica para recuperar recebe o caminho guardado quando a subida termina', async () => {
    const upload = deferred<{ path: string }>();
    core.upload.mockReturnValueOnce(upload.promise);
    core.relimpar.mockResolvedValueOnce({ text: 'ok', aviso: null, estilo_aplicado: 'limpar' });
    const v = voice({ estilo: 'limpar' });
    writeDictation('s1', 'sess', v);
    const out = await runDictation(server, 's1', 'sess', v, { file: wav(), pieces: pieces(Promise.resolve({ raw: 'ok cru', aviso: '' })) },
      { isActive: () => false });
    expect(readDictation('s1', 'sess')).toMatchObject({ status: 'ready', text: 'ok' });
    upload.resolve({ path: '/up/sess/ditado.wav' });
    await out.saved;
    expect(readDictation('s1', 'sess')).toMatchObject({ status: 'ready', serverPath: '/up/sess/ditado.wav', audio: { uri: localCopy } });
    // O ditado guardado ainda toca a cópia local até ser recuperado.
    expect(removeDraftAttachment).not.toHaveBeenCalled();
  });

  it.each([
    ['um pedaço falhou', () => Promise.reject(new Error('503: provedor fora')), () => {}],
    ['a organização falhou', () => Promise.resolve({ raw: 'cru', aviso: '' }), () => { core.relimpar.mockRejectedValueOnce(new Error('502: llm')); }],
    ['o cru juntado veio vazio', () => Promise.resolve({ raw: '  ', aviso: '' }), () => {}],
  ])('%s: aborta os pedaços e segue o caminho de hoje com o arquivo inteiro', async (_case, result, arrange) => {
    arrange();
    core.upload.mockResolvedValueOnce({ path: '/up/sess/ditado.wav' });
    core.transcribe.mockResolvedValueOnce({ path: '/up/sess/ditado.wav', text: 'pelo arquivo', raw: 'pelo arquivo cru', aviso: null });
    const handle = pieces(result());
    const v = voice({ estilo: 'prosa' });
    writeDictation('s1', 'sess', v);
    const out = await runDictation(server, 's1', 'sess', v, { file: wav(), pieces: handle }, { isActive: () => false });
    expect(handle.abort).toHaveBeenCalled();
    expect(core.upload).toHaveBeenCalledTimes(1);
    expect(core.transcribe).toHaveBeenCalledExactlyOnceWith(server, 'sess', 'ditado.wav', { limpar: true, estilo: 'prosa' });
    expect(out).toMatchObject({ text: 'pelo arquivo', path: '/up/sess/ditado.wav' });
    expect(out.saved).toBeUndefined();
  });

  it('sem estilo escolhido nem lido do servidor: o arquivo inteiro deixa o servidor usar o estilo salvo', async () => {
    core.upload.mockResolvedValueOnce({ path: '/up/sess/ditado.wav' });
    core.transcribe.mockResolvedValueOnce({ path: '/up/sess/ditado.wav', text: 'pelo arquivo', raw: 'cru', aviso: null });
    const handle = pieces(Promise.resolve({ raw: 'cru', aviso: '' }));
    writeDictation('s1', 'sess', voice());
    const out = await runDictation(server, 's1', 'sess', voice(), { file: wav(), pieces: handle }, { isActive: () => false });
    expect(core.relimpar).not.toHaveBeenCalled();
    expect(core.transcribe).toHaveBeenCalledExactlyOnceWith(server, 'sess', 'ditado.wav', { limpar: true, estilo: undefined });
    expect(out).toMatchObject({ text: 'pelo arquivo' });
  });

  it('organização desligada no servidor: o cru juntado é o texto, sem chamar o relimpar', async () => {
    useDictationOrganizationStore.setState({ byServer: { s1: 'off' } });
    core.upload.mockResolvedValueOnce({ path: '/up/sess/ditado.wav' });
    const v = voice({ estilo: 'prosa' });
    writeDictation('s1', 'sess', v);
    const out = await runDictation(server, 's1', 'sess', v, { file: wav(), pieces: pieces(Promise.resolve({ raw: 'ela trava. Não sei', aviso: '' })) },
      { isActive: () => false });
    expect(core.relimpar).not.toHaveBeenCalled();
    expect(out).toMatchObject({ text: 'ela trava. Não sei', raw: 'ela trava. Não sei', applied: 'cru', aviso: '' });
  });

  it('subida que falha depois do texto: o texto fica e a falha aparece', async () => {
    core.upload.mockRejectedValueOnce(new Error('413: grande demais'));
    core.relimpar.mockResolvedValueOnce({ text: 'ok', aviso: null, estilo_aplicado: 'prosa' });
    draft();
    const v = voice({ estilo: 'prosa' });
    writeDictation('s1', 'sess', v);
    const out = await runDictation(server, 's1', 'sess', v, { file: wav(), pieces: pieces(Promise.resolve({ raw: 'ok', aviso: '' })) },
      { isActive: () => true });
    expect(readDraft('s1', 'sess')?.text).toBe('ok');
    await expect(out.saved).rejects.toThrow('413: grande demais');
  });

  it('a conferência antes da rede ainda vale e aborta os pedaços', async () => {
    const handle = pieces(Promise.resolve({ raw: 'ok', aviso: '' }));
    const v = voice();
    writeDictation('s1', 'sess', v);
    await expect(runDictation(server, 's1', 'sess', v, { file: wav(), pieces: handle },
      { isActive: () => false, check: () => { throw new Error('interrompido'); } })).rejects.toThrow('interrompido');
    expect(handle.abort).toHaveBeenCalled();
    expect(core.upload).not.toHaveBeenCalled();
  });
});

describe('dictation.client_timing', () => {
  beforeEach(() => {
    memory.clear(); core.transcribe.mockReset(); core.upload.mockReset(); core.diag.mockReset();
    vi.useFakeTimers({ toFake: ['Date'] });
    vi.setSystemTime(10_000);
  });
  afterEach(() => { vi.useRealTimers(); });

  it('mede cada etapa até o texto (arquivo, preparo, conferência, subida, gravação, transcrição, fechamento) e o motivo', async () => {
    core.upload.mockImplementationOnce(async () => { vi.setSystemTime(10_250); return { path: '/up/sess/d.m4a' }; });
    core.transcribe.mockImplementationOnce(async () => { vi.setSystemTime(11_250); return { path: '/up/sess/d.m4a', text: 'olá', raw: 'olá' }; });
    writeDictation('s1', 'sess', voice({ motivo: 'silencio' }));
    await runDictation(server, 's1', 'sess', voice({ motivo: 'silencio' }), { file: new File(['a'], 'd.m4a') }, {
      check: () => vi.setSystemTime(10_050),
      isActive: () => { vi.setSystemTime(11_330); return false; },
      stoppedAt: 9_700, fileAt: 9_900,
    });
    expect(core.diag).toHaveBeenCalledExactlyOnceWith({ evento: 'dictation.client_timing', nivel: 'ok', ms: 1630, motivo: 'silencio',
      detalhe: 'stop_to_file_ms=200 prepare_ms=100 check_ms=50 upload_ms=200 save_ms=0 transcribe_ms=1000 finish_ms=80 total_ms=1630' },
    'https://a.test');
  });

  it('pedaços: mede a espera do último pedaço e a organização, sem esperar a subida, com a marca do caminho', async () => {
    core.upload.mockReturnValueOnce(new Promise(() => {}));
    core.relimpar.mockImplementationOnce(async () => { vi.setSystemTime(11_000); return { text: 'Olá.', aviso: null, estilo_aplicado: 'prosa' }; });
    const result = Promise.resolve().then(() => { vi.setSystemTime(10_600); return { raw: 'olá', aviso: '', count: 2 }; });
    const v = voice({ estilo: 'prosa', motivo: 'silencio' });
    writeDictation('s1', 'sess', v);
    await runDictation(server, 's1', 'sess', v, { file: new File(['a'], 'd.wav'), pieces: { result, abort: vi.fn() } }, {
      isActive: () => { vi.setSystemTime(11_050); return false; }, stoppedAt: 9_700, fileAt: 9_900,
    });
    expect(core.diag).toHaveBeenCalledExactlyOnceWith({ evento: 'dictation.client_timing', nivel: 'ok', ms: 1350, motivo: 'silencio',
      detalhe: 'stop_to_file_ms=200 prepare_ms=100 check_ms=0 upload_ms=0 save_ms=0 transcribe_ms=1000 finish_ms=50 total_ms=1350 '
        + 'path=pieces pieces=2 last_piece_wait_ms=900 organize_ms=400' },
    'https://a.test');
  });

  it('pedaços que desistem: o caminho de hoje mede as etapas dele e leva a marca da volta', async () => {
    core.upload.mockResolvedValueOnce({ path: '/up/sess/d.wav' });
    core.transcribe.mockResolvedValueOnce({ path: '/up/sess/d.wav', text: 'ok', raw: 'ok' });
    const result = Promise.reject(new Error('503: provedor fora'));
    result.catch(() => {});
    const v = voice({ estilo: 'prosa' });
    writeDictation('s1', 'sess', v);
    await runDictation(server, 's1', 'sess', v, { file: new File(['a'], 'd.wav'), pieces: { result, abort: vi.fn() } },
      { isActive: () => false, stoppedAt: 10_000 });
    expect(core.diag).toHaveBeenCalledWith(expect.objectContaining({ evento: 'dictation.client_timing', nivel: 'ok',
      detalhe: expect.stringMatching(/upload_ms=\d+ .* total_ms=\d+ path=pieces_fallback$/) }), 'https://a.test');
  });

  it('a falha diz em que etapa parou: o tempo antes da subida não vira subida', async () => {
    core.upload.mockImplementationOnce(async () => { vi.setSystemTime(10_300); throw new Error('rede caiu'); });
    writeDictation('s1', 'sess', voice());
    await expect(runDictation(server, 's1', 'sess', voice(), { file: new File(['a'], 'd.m4a') }, {
      check: () => vi.setSystemTime(10_100), isActive: () => false, stoppedAt: 10_000,
    })).rejects.toThrow();
    expect(core.diag).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ nivel: 'erro', codigo: 'failed', etapa: 'upload', motivo: 'botao',
      detalhe: 'stop_to_file_ms=0 prepare_ms=0 check_ms=100 upload_ms=200 save_ms=0 transcribe_ms=0 finish_ms=0 total_ms=300' }),
    'https://a.test');
  });

  it('a conferência que recusa antes da rede (app escondido, outra conversa) é aviso, não erro', async () => {
    writeDictation('s1', 'sess', voice());
    await expect(runDictation(server, 's1', 'sess', voice(), { file: new File(['a'], 'd.m4a') }, {
      check: () => { vi.setSystemTime(10_020); throw new Error('outra conversa'); }, isActive: () => false, stoppedAt: 10_000,
    })).rejects.toThrow();
    expect(core.upload).not.toHaveBeenCalled();
    expect(core.diag).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ nivel: 'aviso', codigo: 'interrupted', etapa: 'check',
      detalhe: 'stop_to_file_ms=0 prepare_ms=0 check_ms=20 upload_ms=0 save_ms=0 transcribe_ms=0 finish_ms=0 total_ms=20' }),
    'https://a.test');
  });

  it('texto que voltou cru com aviso sai como aviso, igual ao servidor', async () => {
    core.upload.mockResolvedValueOnce({ path: '/up/sess/d.m4a' });
    core.transcribe.mockResolvedValueOnce({ path: '/up/sess/d.m4a', text: 'cru', raw: 'cru', aviso: 'organização falhou' });
    writeDictation('s1', 'sess', voice());
    await runDictation(server, 's1', 'sess', voice(), { file: new File(['a'], 'd.m4a') }, { isActive: () => false, stoppedAt: 10_000 });
    expect(core.diag).toHaveBeenCalledExactlyOnceWith(expect.objectContaining({ evento: 'dictation.client_timing', nivel: 'aviso' }), 'https://a.test');
  });

  it('a falha leva o status como código; o "de novo", sem parada, não mede', async () => {
    core.upload.mockResolvedValueOnce({ path: '/up/sess/d.m4a' });
    core.transcribe.mockImplementationOnce(async () => { vi.setSystemTime(10_400); throw new Error('502: groq'); });
    writeDictation('s1', 'sess', voice());
    await expect(runDictation(server, 's1', 'sess', voice(), { file: new File(['a'], 'd.m4a') }, { isActive: () => false, stoppedAt: 9_900 }))
      .rejects.toThrow();
    expect(core.diag).toHaveBeenCalledExactlyOnceWith({ evento: 'dictation.client_timing', nivel: 'erro', codigo: '502', ms: 500,
      etapa: 'transcribe', motivo: 'botao',
      detalhe: 'stop_to_file_ms=100 prepare_ms=0 check_ms=0 upload_ms=0 save_ms=0 transcribe_ms=400 finish_ms=0 total_ms=500' },
    'https://a.test');
    core.diag.mockReset();
    core.transcribe.mockResolvedValueOnce({ path: '/up/sess/d.m4a', text: 'ok' });
    await runDictation(server, 's1', 'sess', voice({ audio: null }), { arquivo: 'd.m4a' }, { isActive: () => false });
    expect(core.diag).not.toHaveBeenCalled();
  });
});

describe('dictateUpload', () => {
  beforeEach(() => { memory.clear(); core.transcribe.mockReset(); core.upload.mockReset(); vi.mocked(removeDraftAttachment).mockClear(); });

  it('galeria transcreve pelo nome guardado, sem subir cópia, e o texto entra no rascunho intacto', async () => {
    writeDraft('s1', 'sess', { version: 1, text: 'antes', revision: 2, transcript: '/t/a.jsonl', attachments: [], submission: null });
    core.transcribe.mockResolvedValueOnce({ path: '/up/sess/ditado-9.m4a', text: ' depois ', raw: 'depois cru', aviso: null, estilo_aplicado: 'prosa' });
    await dictateUpload(server, 's1', 'sess', '/t/a.jsonl', 'ditado-9.m4a', 'prosa');
    expect(core.transcribe).toHaveBeenCalledExactlyOnceWith(server, 'sess', 'ditado-9.m4a', { limpar: true, estilo: 'prosa' });
    expect(core.upload).not.toHaveBeenCalled();
    expect(readDictation('s1', 'sess')).toMatchObject({
      status: 'ready', text: 'depois', audio: null, serverPath: '/up/sess/ditado-9.m4a', draftRevision: 2, before: 'antes',
    });
    expect(applyReadyDictation('s1', 'sess', '/t/a.jsonl')?.draft?.text).toBe('antes depois');
    expect(readDraft('s1', 'sess')?.text).toBe('antes depois');
  });

  it('durante o POST o ditado fica pendente para quem montar a conversa', async () => {
    let finish!: (v: unknown) => void;
    core.transcribe.mockImplementationOnce(() => new Promise((resolve) => { finish = resolve; }));
    const run = dictateUpload(server, 's1', 'sess', null, 'a.m4a');
    expect(readDictation('s1', 'sess')?.status).toBe('pending');
    finish({ path: '/up/a.m4a', text: 'ok' });
    await run;
    expect(readDictation('s1', 'sess')?.status).toBe('ready');
  });

  it('recusa na hora quando há ditado pronto ainda não usado', () => {
    writeDictation('s1', 'sess', voice({ status: 'ready', text: 'guardado' }));
    expect(() => dictateUpload(server, 's1', 'sess', null, 'a.m4a')).toThrow();
    expect(core.transcribe).not.toHaveBeenCalled();
  });

  it('recusa enquanto o ditado da conversa (gravado na tela por baixo) está no ar', () => {
    writeDictation('s1', 'sess', voice());
    setDictationInFlight('s1', 'sess', 'v1', true);
    expect(() => dictateUpload(server, 's1', 'sess', null, 'a.m4a')).toThrow();
    expect(readDictation('s1', 'sess')?.id).toBe('v1');
    expect(core.transcribe).not.toHaveBeenCalled();
    setDictationInFlight('s1', 'sess', 'v1', false);
  });

  it('ditado com falha que já subiu é substituído pelo da galeria e a cópia local sai', async () => {
    writeDictation('s1', 'sess', voice({ status: 'failed', issue: '502: groq', serverPath: '/up/sess/d.m4a' }));
    core.transcribe.mockResolvedValueOnce({ path: '/up/sess/b.m4a', text: 'novo' });
    await dictateUpload(server, 's1', 'sess', null, 'b.m4a');
    expect(readDictation('s1', 'sess')).toMatchObject({ status: 'ready', text: 'novo', audio: null });
    expect(removeDraftAttachment).toHaveBeenCalledWith('file:///doc/draft-attachments/1-1.m4a');
  });

  it('recusa quando a gravação com falha só existe no aparelho (não subiu)', () => {
    writeDictation('s1', 'sess', voice({ status: 'failed', issue: 'rede caiu' }));
    expect(() => dictateUpload(server, 's1', 'sess', null, 'b.m4a')).toThrow();
    expect(readDictation('s1', 'sess')).toMatchObject({ id: 'v1', audio: { uri: 'file:///doc/draft-attachments/1-1.m4a' } });
    expect(removeDraftAttachment).not.toHaveBeenCalled();
    expect(core.transcribe).not.toHaveBeenCalled();
  });
});

describe('dictationAudio', () => {
  it('toca a cópia local enquanto existe; sem ela, o arquivo do servidor com o token no cabeçalho', () => {
    expect(dictationAudio(server, 'sess', voice().audio, '/up/sess/d.m4a')).toEqual({ uri: 'file:///doc/draft-attachments/1-1.m4a', name: 'd.m4a' });
    expect(dictationAudio(server, 'sess', null, '/up/sess/x.m4a')).toEqual({
      uri: 'https://a.test/api/sessions/sess/uploads/x.m4a', headers: { Authorization: 'Bearer t' }, name: 'x.m4a',
    });
    expect(dictationAudio(server, 'sess', null, undefined)).toBeNull();
  });
});
