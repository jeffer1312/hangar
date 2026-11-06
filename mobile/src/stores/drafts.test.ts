import { beforeEach, describe, expect, it, vi } from 'vitest';
import { prefs } from './prefs';
import {
  clearDraft, clearRecoverableDraft, deliveredDraft, readDraft, readRecoverableDraft, resolveDraftTranscript, reusableUploadPath, withoutUpload,
  writeDraft, writeRecoverableDraft,
  readDictation, writeDictation, clearDictation, finishDictation, recoverDictation, associateDictationTranscript,
  setDictationInFlight, applyReadyDictation, removeDraftRecording,
} from './drafts';
import type { ConversationDraft, DraftAttachment, DictationDraft } from './drafts';

const { memory } = vi.hoisted(() => ({ memory: new Map<string, string>() }));
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (key: string) => memory.get(key),
    set: (key: string, value: string) => { memory.set(key, value); },
    remove: (key: string) => { memory.delete(key); },
  }),
}));

function draft(overrides: Partial<ConversationDraft> = {}): ConversationDraft {
  return {
    version: 1, text: 'Mensagem atual', revision: 3, transcript: '/sessions/original.jsonl',
    attachments: [], submission: null, ...overrides,
  };
}

describe('drafts', () => {
  beforeEach(() => { memory.clear(); vi.restoreAllMocks(); });

  it('retorna ausência sem criar rascunho', () => {
    expect(readDraft('linux', 'sessao')).toBeNull();
    expect(memory.size).toBe(0);
  });

  it('restaura após remontar o módulo e isola servidor e sessão', async () => {
    writeDraft('linux', 'sessao', draft({ text: 'Linux' }));
    writeDraft('windows', 'sessao', draft({ text: 'Windows', transcript: 'F:\\sessions\\chat.jsonl' }));
    writeDraft('linux', 'outra', draft({ text: 'Outra' }));
    vi.resetModules();
    const reopened = await import('./drafts');
    expect(reopened.readDraft('linux', 'sessao')?.text).toBe('Linux');
    expect(reopened.readDraft('windows', 'sessao')?.transcript).toBe('F:\\sessions\\chat.jsonl');
    expect(reopened.readDraft('linux', 'outra')?.text).toBe('Outra');
    expect(reopened.readDraft('other', 'sessao')).toBeNull();
    expect(memory.has('draft.v1:linux::sessao')).toBe(true);
  });

  it('restaura envio interrompido como incerto preservando texto novo e sua revisão', () => {
    const value = draft({
      text: 'Texto posterior', revision: 4,
      submission: { text: 'Snapshot enviado', draftRevision: 3, status: 'sending' },
    });
    writeDraft('linux', 'sessao', value);
    expect(readDraft('linux', 'sessao')).toEqual({
      ...value, submission: { text: 'Snapshot enviado', draftRevision: 3, status: 'unknown' },
    });
    expect(value.submission?.status).toBe('sending');
  });

  it.each(['unknown', 'rejected'] as const)('preserva snapshot %s sem substituir texto de mesma grafia e revisão nova', (status) => {
    const value = draft({
      text: 'Mesmo texto', revision: 4,
      submission: { text: 'Mesmo texto', draftRevision: 3, status },
    });
    writeDraft('linux', 'sessao', value);
    expect(readDraft('linux', 'sessao')).toEqual(value);
  });

  it('persiste anexo e caminho de upload na identidade original', () => {
    writeDraft('linux', 'sessao', draft({ attachments: [{
      uri: 'file:///app/drafts/photo.jpg', name: 'photo.jpg', mime: 'image/jpeg',
      kind: 'image', uploadedPath: '/uploads/photo.jpg',
    }, { uri: 'file:///app/drafts/nota.txt', name: 'nota.txt', mime: 'text/plain', kind: 'file' }] }));
    expect(readDraft('linux', 'sessao')?.attachments).toEqual([{
      uri: 'file:///app/drafts/photo.jpg', name: 'photo.jpg', mime: 'image/jpeg',
      kind: 'image', uploadedPath: '/uploads/photo.jpg',
    }, { uri: 'file:///app/drafts/nota.txt', name: 'nota.txt', mime: 'text/plain', kind: 'file' }]);
    expect(readDraft('other', 'sessao')).toBeNull();
  });

  it('lê o rascunho gravado com um anexo só como lista', () => {
    const photo = { uri: 'file:///app/drafts/photo.jpg', name: 'photo.jpg', mime: 'image/jpeg', kind: 'image' };
    const { attachments: _none, ...legacy } = draft();
    memory.set('draft.v1:linux::sessao', JSON.stringify({ ...legacy, attachment: photo }));
    expect(readDraft('linux', 'sessao')?.attachments).toEqual([photo]);
    memory.set('draft.v1:linux::sessao', JSON.stringify({ ...legacy, attachment: null }));
    expect(readDraft('linux', 'sessao')?.attachments).toEqual([]);
  });

  it('resolver identidade sem rascunho não cria dados locais', () => {
    expect(resolveDraftTranscript('linux', 'sessao', '/sessions/new.jsonl'))
      .toEqual({ draft: null, recoverable: null });
    expect(readDraft('linux', 'sessao')).toBeNull();
  });

  it('jsonl ainda desconhecido mantém a identidade conhecida e não recria a sessão', () => {
    const value = draft();
    writeDraft('linux', 'sessao', value);
    expect(resolveDraftTranscript('linux', 'sessao', null))
      .toEqual({ draft: value, recoverable: null });
    expect(readDraft('linux', 'sessao')?.transcript).toBe('/sessions/original.jsonl');
  });

  it('primeiro jsonl associa o rascunho provisório sem perder anexo, revisão ou snapshot', () => {
    const value = draft({
      transcript: null, revision: 4,
      attachments: [{ uri: 'file:///app/drafts/a', name: 'a', mime: 'text/plain', kind: 'file' }],
      submission: { text: 'Snapshot anterior', draftRevision: 3, status: 'rejected' },
    });
    writeDraft('linux', 'sessao', value);
    expect(resolveDraftTranscript('linux', 'sessao', null))
      .toEqual({ draft: value, recoverable: null });
    const associated = { ...value, transcript: '/sessions/first.jsonl' };
    expect(resolveDraftTranscript('linux', 'sessao', '/sessions/first.jsonl'))
      .toEqual({ draft: associated, recoverable: null });
    expect(readDraft('linux', 'sessao')).toEqual(associated);
  });

  it('mesmo jsonl mantém rascunho sem exigir nova gravação', () => {
    const value = draft();
    writeDraft('linux', 'sessao', value);
    vi.spyOn(prefs, 'set').mockImplementationOnce(() => { throw new Error('storage unavailable'); });
    expect(resolveDraftTranscript('linux', 'sessao', '/sessions/original.jsonl'))
      .toEqual({ draft: value, recoverable: null });
  });

  it('jsonl diferente conserva o anterior para recuperação e não o restaura na sessão recriada', () => {
    const value = draft({ submission: { text: 'Snapshot anterior', draftRevision: 2, status: 'unknown' } });
    writeDraft('linux', 'sessao', value);
    const raw = memory.get('draft.v1:linux::sessao');
    expect(resolveDraftTranscript('linux', 'sessao', '/sessions/recreated.jsonl'))
      .toEqual({ draft: null, recoverable: value });
    expect(memory.get('draft.v1:linux::sessao')).toBe(raw);
    expect(resolveDraftTranscript('linux', 'sessao', '/sessions/original.jsonl'))
      .toEqual({ draft: value, recoverable: null });
  });

  it('rascunho recuperável tem chave própria por servidor e sessão, sem tocar a principal', () => {
    const current = draft({ text: 'Sessão nova', transcript: '/sessions/recreated.jsonl' });
    const old = draft({ text: 'Sessão morta' });
    writeDraft('linux', 'sessao', current);
    writeRecoverableDraft('linux', 'sessao', old);
    expect(readRecoverableDraft('linux', 'sessao')).toEqual(old);
    expect(readRecoverableDraft('windows', 'sessao')).toBeNull();
    expect(readDraft('linux', 'sessao')).toEqual(current);
    clearRecoverableDraft('linux', 'sessao');
    expect(readRecoverableDraft('linux', 'sessao')).toBeNull();
    expect(readDraft('linux', 'sessao')).toEqual(current);
  });

  it('associação que não foi guardada expõe falha e mantém rascunho provisório recuperável', () => {
    const value = draft({ transcript: null });
    writeDraft('linux', 'sessao', value);
    vi.spyOn(prefs, 'set').mockImplementationOnce(() => { throw new Error('storage unavailable'); });
    expect(() => resolveDraftTranscript('linux', 'sessao', '/sessions/first.jsonl')).toThrow();
    expect(readDraft('linux', 'sessao')).toEqual(value);
  });

  it.each(['', '{"text":"private text"', 'null', '[]', '{}',
    JSON.stringify(draft({ version: 2 as 1 })),
    JSON.stringify(draft({ revision: -1 })),
    JSON.stringify(draft({ revision: 1.5 })),
    JSON.stringify(draft({ transcript: 4 as unknown as string })),
    JSON.stringify(draft({ submission: { text: 'private text', draftRevision: 3, status: 'sent' as 'sending' } })),
    JSON.stringify(draft({ attachments: [{ uri: '', name: 'a', mime: 'text/plain', kind: 'file' }] })),
  ])('expõe erro recuperável sem apagar ou incluir dados inválidos no erro: %s', (raw) => {
    memory.set('draft.v1:linux::sessao', raw);
    let failure: unknown;
    try { readDraft('linux', 'sessao'); } catch (error) { failure = error; }
    expect(failure).toBeInstanceOf(Error);
    expect(String(failure)).not.toContain('private text');
    expect(memory.get('draft.v1:linux::sessao')).toBe(raw);
  });

  it('grava somente os campos do contrato, inclusive dentro do anexo e snapshot', () => {
    const value = { ...draft(), token: 'private-token', attachments: [{
      uri: 'file:///app/drafts/a', name: 'a', mime: 'text/plain', kind: 'file' as const,
      token: 'private-token',
    }], submission: { text: 'Snapshot', draftRevision: 2, status: 'rejected' as const, token: 'private-token' } };
    writeDraft('linux', 'sessao', value);
    expect(memory.get('draft.v1:linux::sessao')).not.toContain('private-token');
    memory.set('draft.v1:linux::sessao', JSON.stringify(value));
    expect(readDraft('linux', 'sessao')).toEqual({
      ...draft(), attachments: [{ uri: 'file:///app/drafts/a', name: 'a', mime: 'text/plain', kind: 'file' }],
      submission: { text: 'Snapshot', draftRevision: 2, status: 'rejected' },
    });
  });

  it('erro de leitura não vira rascunho ausente nem expõe o erro bruto', () => {
    vi.spyOn(prefs, 'getString').mockImplementationOnce(() => { throw new Error('private storage path'); });
    expect(() => readDraft('linux', 'sessao')).toThrow();
    vi.spyOn(prefs, 'getString').mockImplementationOnce(() => { throw new Error('private storage path'); });
    expect(() => readDraft('linux', 'sessao')).not.toThrow('private storage path');
  });

  it('falha de gravação preserva o rascunho anterior e interrompe o chamador', () => {
    writeDraft('linux', 'sessao', draft());
    vi.spyOn(prefs, 'set').mockImplementationOnce(() => { throw new Error('private storage path'); });
    expect(() => writeDraft('linux', 'sessao', draft({ text: 'Novo' }))).toThrow();
    expect(readDraft('linux', 'sessao')?.text).toBe('Mensagem atual');
  });

  it('recusa dados inválidos antes de substituir rascunho válido', () => {
    writeDraft('linux', 'sessao', draft());
    expect(() => writeDraft('linux', 'sessao', draft({ revision: -1 }))).toThrow();
    expect(readDraft('linux', 'sessao')?.revision).toBe(3);
  });

  it('limpa somente a conversa pedida, preservando outros dados locais', () => {
    writeDraft('linux', 'sessao', draft());
    writeDraft('windows', 'sessao', draft({ text: 'Windows' }));
    memory.set('appearance', 'glass');
    clearDraft('linux', 'sessao');
    expect(readDraft('linux', 'sessao')).toBeNull();
    expect(readDraft('windows', 'sessao')?.text).toBe('Windows');
    expect(memory.get('appearance')).toBe('glass');
  });

  it('falha ao limpar conserva o rascunho e interrompe o chamador', () => {
    writeDraft('linux', 'sessao', draft());
    vi.spyOn(prefs, 'remove').mockImplementationOnce(() => { throw new Error('private storage path'); });
    expect(() => clearDraft('linux', 'sessao')).toThrow();
    expect(readDraft('linux', 'sessao')?.text).toBe('Mensagem atual');
  });
});

describe('upload do anexo', () => {
  const target = { serverId: 'linux', name: 'sessao', transcript: '/sessions/original.jsonl' };
  const attachment: DraftAttachment = {
    uri: 'file:///doc/draft-attachments/1-1.png', name: 'foto.png', mime: 'image/png', kind: 'image',
    uploadedPath: '/uploads/foto.png', uploadedFor: target,
  };

  beforeEach(() => { memory.clear(); });

  it('reabrir conserva arquivo, caminho enviado e destino', () => {
    writeDraft('linux', 'sessao', draft({ attachments: [attachment] }));
    const reopened = readDraft('linux', 'sessao')!.attachments[0];
    expect(reopened).toEqual(attachment);
    expect(reusableUploadPath(reopened, target)).toBe('/uploads/foto.png');
  });

  it('reaproveita upload feito antes de a sessão ter transcript', () => {
    expect(reusableUploadPath({ ...attachment, uploadedFor: { ...target, transcript: null } }, target)).toBe('/uploads/foto.png');
  });

  it.each([
    ['outro servidor', { ...target, serverId: 'windows' }],
    ['outra sessão', { ...target, name: 'outra' }],
    ['sessão recriada', { ...target, transcript: '/sessions/nova.jsonl' }],
  ])('%s não herda o caminho', (_caso, other) => {
    expect(reusableUploadPath(attachment, other)).toBeNull();
  });

  it('sem destino gravado não reaproveita, e withoutUpload apaga caminho e destino', () => {
    expect(reusableUploadPath({ ...attachment, uploadedFor: undefined }, target)).toBeNull();
    const bare = withoutUpload(attachment);
    expect(bare).toEqual({ uri: attachment.uri, name: 'foto.png', mime: 'image/png', kind: 'image' });
    expect(reusableUploadPath(bare, target)).toBeNull();
  });

  it('envio incerto que chegou tira o aviso, o texto devolvido e o anexo enviado, e deixa o colado depois', () => {
    const later: DraftAttachment = { uri: 'file:///doc/draft-attachments/2-2.png', name: 'nova.png', mime: 'image/png', kind: 'image' };
    const sent = draft({ text: 'olha isso', revision: 5, attachments: [attachment, later],
      submission: { text: 'olha isso — 📎 imagem: /uploads/foto.png', typed: 'olha isso', draftRevision: 3, status: 'unknown' } });
    expect(deliveredDraft(sent)).toMatchObject({ text: '', revision: 6, attachments: [later], submission: null });
  });

  it('envio incerto que chegou não apaga o que a pessoa digitou depois', () => {
    const sent = draft({ text: 'outra coisa', submission: { text: 'olha isso', typed: 'olha isso', draftRevision: 3, status: 'unknown' } });
    expect(deliveredDraft(sent)).toMatchObject({ text: 'outra coisa', submission: null });
  });
});

describe('ditado conservado na origem', () => {
  const audio: DraftAttachment = { uri: 'file:///doc/draft-attachments/1-1.m4a', name: 'ditado.m4a', mime: 'audio/m4a', kind: 'file' };
  const voice = (patch: Partial<DictationDraft> = {}): DictationDraft => ({
    version: 1, id: 'recording-1', audio, transcript: '/sessions/original.jsonl',
    draftRevision: 3, before: 'Mensagem atual', motivo: 'silencio', status: 'pending',
    text: '', raw: '', issue: '', ...patch,
  });
  beforeEach(() => { memory.clear(); vi.restoreAllMocks(); });

  it('acrescenta gravações, restaura a lista e remove uma sem alterar o texto', async () => {
    writeDraft('linux', 'sessao', draft());
    writeDictation('linux', 'sessao', voice());
    finishDictation('linux', 'sessao', 'recording-1',
      { text: 'Primeiro áudio', raw: '', issue: '', serverPath: '/uploads/primeiro.m4a' }, true);
    writeDictation('linux', 'sessao', voice({ id: 'recording-2', draftRevision: 4 }));
    finishDictation('linux', 'sessao', 'recording-2',
      { text: 'Segundo áudio', raw: '', issue: '', serverPath: '/uploads/segundo.m4a' }, true);
    vi.resetModules();
    const reopened = await import('./drafts');
    expect(reopened.readDraft('linux', 'sessao')?.recordings).toEqual([
      { id: 'recording-1', serverPath: '/uploads/primeiro.m4a' },
      { id: 'recording-2', serverPath: '/uploads/segundo.m4a' },
    ]);
    const before = readDraft('linux', 'sessao')!;
    const after = removeDraftRecording('linux', 'sessao', 'recording-1');
    expect(after?.recordings).toEqual([{ id: 'recording-2', serverPath: '/uploads/segundo.m4a' }]);
    expect(after?.text).toBe('Mensagem atual Primeiro áudio Segundo áudio');
    expect(after?.revision).toBe(before.revision);
    expect(readDraft('linux', 'sessao')).toEqual(after);
  });

  it('tirar o último áudio de um rascunho sem texto apaga o rascunho', () => {
    writeDraft('linux', 'sessao', draft({ text: '', recordings: [{ id: 'recording-1', serverPath: '/uploads/a.m4a' }] }));
    expect(removeDraftRecording('linux', 'sessao', 'recording-1')).toBeNull();
    expect(readDraft('linux', 'sessao')).toBeNull();
    // Sessão recriada não herda um rascunho vazio para recuperar.
    expect(resolveDraftTranscript('linux', 'sessao', '/sessions/recriada.jsonl')).toEqual({ draft: null, recoverable: null });
  });

  it('reabrir conserva áudio sem repetir automaticamente um POST pendente', async () => {
    writeDictation('linux', 'sessao', voice());
    vi.resetModules();
    const reopened = await import('./drafts');
    expect(reopened.readDictation('linux', 'sessao')).toMatchObject({ audio, status: 'failed' });
    expect(reopened.readDictation('windows', 'sessao')).toBeNull();
    expect(reopened.readDictation('linux', 'outra')).toBeNull();
  });

  it('POST vivo neste processo continua pendente; sem ele, reabrir oferece repetir', () => {
    writeDictation('linux', 'sessao', voice());
    setDictationInFlight('linux', 'sessao', 'recording-1', true);
    expect(readDictation('linux', 'sessao')?.status).toBe('pending');
    setDictationInFlight('linux', 'sessao', 'recording-1', false);
    expect(readDictation('linux', 'sessao')?.status).toBe('failed');
  });

  it('ditado pronto com rascunho intacto entra no fim, separado por espaço, e sai do armazenamento', () => {
    writeDraft('linux', 'sessao', draft());
    writeDictation('linux', 'sessao', voice({ status: 'ready', text: 'Ditado limpo', raw: 'cru' }));
    const result = applyReadyDictation('linux', 'sessao', '/sessions/original.jsonl');
    expect(result?.draft).toMatchObject({ text: 'Mensagem atual Ditado limpo', revision: 4, dictationId: 'recording-1' });
    expect(readDictation('linux', 'sessao')).toBeNull();
  });

  it.each([
    ['rascunho editado', draft({ text: 'Outro', revision: 4 }), '/sessions/original.jsonl'],
    ['sessão recriada', draft(), '/sessions/new.jsonl'],
    ['jsonl ainda desconhecido', draft(), null],
  ])('%s não insere sozinho e mantém o ditado pronto', (_reason, current, transcript) => {
    writeDraft('linux', 'sessao', current);
    writeDictation('linux', 'sessao', voice({ status: 'ready', text: 'Ditado' }));
    expect(applyReadyDictation('linux', 'sessao', transcript)).toBeNull();
    expect(readDraft('linux', 'sessao')).toEqual(current);
    expect(readDictation('linux', 'sessao')?.status).toBe('ready');
  });

  it('ditado novo não grava por cima de um POST vivo da mesma conversa', () => {
    writeDictation('linux', 'sessao', voice());
    setDictationInFlight('linux', 'sessao', 'recording-1', true);
    expect(() => writeDictation('linux', 'sessao', voice({ id: 'recording-2' }))).toThrow();
    expect(readDictation('linux', 'sessao')?.id).toBe('recording-1');
    // O próprio POST continua podendo atualizar o seu registro.
    writeDictation('linux', 'sessao', voice({ serverPath: '/up/sess/ditado-1.m4a' }));
    expect(readDictation('linux', 'sessao')?.serverPath).toBe('/up/sess/ditado-1.m4a');
    setDictationInFlight('linux', 'sessao', 'recording-1', false);
    writeDictation('linux', 'sessao', voice({ id: 'recording-2' }));
    expect(readDictation('linux', 'sessao')?.id).toBe('recording-2');
  });

  it('resposta no contexto original insere texto preservando anexo e snapshot', () => {
    const attachment = { uri: 'file:///foto.png', name: 'foto.png', mime: 'image/png', kind: 'image' as const };
    writeDraft('linux', 'sessao', draft({ attachments: [attachment], submission: { text: 'Anterior', draftRevision: 2, status: 'unknown' } }));
    writeDictation('linux', 'sessao', voice());
    const result = finishDictation('linux', 'sessao', 'recording-1', { text: 'Ditado limpo', raw: 'Ditado cru', issue: '' }, true);
    expect(result.draft).toMatchObject({ text: 'Mensagem atual Ditado limpo', revision: 4, attachments: [attachment],
      submission: { text: 'Anterior', draftRevision: 2, status: 'unknown' } });
    expect(readDictation('linux', 'sessao')).toBeNull();
  });

  it.each([
    ['desmontagem', false, draft()],
    ['edição durante transcrição', true, draft({ text: 'Texto novo', revision: 4 })],
    ['recriação com mesmo nome', true, draft({ transcript: '/sessions/new.jsonl', text: 'Sessão nova' })],
  ])('%s conserva resultado recuperável sem inserir nem limpar outro texto', (_reason, active, current) => {
    writeDraft('linux', 'sessao', current);
    writeDraft('windows', 'sessao', draft({ text: 'Windows' }));
    writeDictation('linux', 'sessao', voice());
    const result = finishDictation('linux', 'sessao', 'recording-1', { text: 'Retorno tardio', raw: 'Texto original', issue: '' }, active);
    expect(result.draft).toBeNull();
    expect(readDraft('linux', 'sessao')).toEqual(current);
    expect(readDraft('windows', 'sessao')?.text).toBe('Windows');
    expect(readDictation('linux', 'sessao')).toMatchObject({ text: 'Retorno tardio', raw: 'Texto original', status: 'ready', audio });
  });

  it('primeiro transcript associa ditado iniciado sem jsonl', () => {
    writeDraft('linux', 'sessao', draft());
    writeDictation('linux', 'sessao', voice({ transcript: null }));
    expect(finishDictation('linux', 'sessao', 'recording-1', { text: 'Chegou', raw: '', issue: '' }, true).draft?.text)
      .toBe('Mensagem atual Chegou');
  });

  it('primeira associação deixa de ser curinga após recriação com revisão igual', () => {
    writeDraft('linux', 'sessao', draft({ transcript: null }));
    writeDictation('linux', 'sessao', voice({ transcript: null }));
    associateDictationTranscript('linux', 'sessao', '/sessions/original.jsonl');
    expect(readDictation('linux', 'sessao')?.transcript).toBe('/sessions/original.jsonl');
    writeDraft('linux', 'sessao', draft({ transcript: '/sessions/new.jsonl', text: 'Outra época' }));
    associateDictationTranscript('linux', 'sessao', '/sessions/new.jsonl');
    expect(finishDictation('linux', 'sessao', 'recording-1', { text: 'Antiga', raw: '', issue: '' }, true).draft).toBeNull();
    expect(readDraft('linux', 'sessao')?.text).toBe('Outra época');
    expect(readDictation('linux', 'sessao')?.text).toBe('Antiga');
  });

  it('dados inválidos e tokens não passam para a persistência do áudio', () => {
    writeDictation('linux', 'sessao', { ...voice(), token: 'private-token' } as DictationDraft);
    expect(memory.get('draft.v1.dictation:linux::sessao')).not.toContain('private-token');
    expect(() => writeDictation('linux', 'sessao', voice({ draftRevision: -1 }))).toThrow();
    expect(readDictation('linux', 'sessao')?.draftRevision).toBe(3);
  });

  it('resposta de tentativa anterior não substitui a repetição explícita', () => {
    writeDraft('linux', 'sessao', draft());
    writeDictation('linux', 'sessao', voice({ id: 'retry-2' }));
    expect(finishDictation('linux', 'sessao', 'recording-1', { text: 'Antiga', raw: '', issue: '' }, true).draft).toBeNull();
    expect(readDictation('linux', 'sessao')?.id).toBe('retry-2');
    expect(readDraft('linux', 'sessao')?.text).toBe('Mensagem atual');
  });

  it('falha de escrita mantém áudio/resultado recuperável e não altera texto', () => {
    writeDraft('linux', 'sessao', draft());
    writeDictation('linux', 'sessao', voice());
    const set = prefs.set.bind(prefs);
    vi.spyOn(prefs, 'set').mockImplementation((key, value) => {
      if (key === 'draft.v1:linux::sessao') throw new Error('storage unavailable');
      set(key, value);
    });
    expect(() => finishDictation('linux', 'sessao', 'recording-1', { text: 'Guardado', raw: '', issue: '' }, true)).toThrow();
    expect(readDraft('linux', 'sessao')?.text).toBe('Mensagem atual');
    expect(readDictation('linux', 'sessao')).toMatchObject({ text: 'Guardado', status: 'ready', audio });
  });

  it.each([false, true])('falha ao limpar após inserção (recuperação explícita: %s) não permite duplicar ditado', (explicit) => {
    writeDraft('linux', 'sessao', draft());
    writeDictation('linux', 'sessao', voice({ status: explicit ? 'ready' : 'pending', text: explicit ? 'Ditado' : '' }));
    vi.spyOn(prefs, 'remove').mockImplementation(() => { throw new Error('storage unavailable'); });
    const inserted = explicit ? recoverDictation('linux', 'sessao', 'recording-1')
      : finishDictation('linux', 'sessao', 'recording-1', { text: 'Ditado', raw: '', issue: '' }, true);
    expect(inserted.draft?.text).toBe('Mensagem atual Ditado');
    expect(readDictation('linux', 'sessao')?.status).toBe('applied');
    expect(recoverDictation('linux', 'sessao', 'recording-1').draft).toBeNull();
    expect(readDraft('linux', 'sessao')?.text).toBe('Mensagem atual Ditado');
  });

  it('limpar ditado não apaga anexo, rascunho nem ditado de outro servidor', () => {
    writeDraft('linux', 'sessao', draft());
    writeDictation('linux', 'sessao', voice());
    writeDictation('windows', 'sessao', voice({ id: 'windows' }));
    clearDictation('linux', 'sessao');
    expect(readDictation('linux', 'sessao')).toBeNull();
    expect(readDictation('windows', 'sessao')?.id).toBe('windows');
    expect(readDraft('linux', 'sessao')?.text).toBe('Mensagem atual');
  });

  it('aceita ditado sem cópia local, só com o caminho do servidor', () => {
    writeDictation('linux', 'sessao', voice({ audio: null, serverPath: 'ditado-1.m4a' }));
    finishDictation('linux', 'sessao', 'recording-1',
      { text: 'Pronto', raw: 'cru', issue: '', serverPath: '/up/sess/ditado-1.m4a', applied: 'prosa' }, false);
    expect(readDictation('linux', 'sessao')).toMatchObject({
      audio: null, serverPath: '/up/sess/ditado-1.m4a', applied: 'prosa', status: 'ready', text: 'Pronto',
    });
  });
});
