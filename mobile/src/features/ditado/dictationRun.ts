import { fileAuthHeader, transcribeUploadedForServer, uploadFileForServer, uploadUrlNative, type Server } from '@hangar/core';
import * as m from '../../paraglide/messages';
import { failureCode, recordTiming, stageClock } from './clientTiming';
import { chatStore } from '../../stores/chat';
import { finishDictation, readDictation, readDraft, setDictationInFlight, writeDictation, type DictationDraft, type DraftAttachment } from '../../stores/drafts';
import { removeDraftAttachment } from '../../chat/draftAttachments';

// `file`: gravação nova, sobe antes de transcrever. `arquivo`: nome solto na pasta da conversa, ou o
// `path` absoluto guardado (alcança o áudio de antes de um /clear; o convidado não pode usá-lo).
export type DictationSource = { file: File } | { arquivo: string };

export interface DictationOutcome {
  completed: ReturnType<typeof finishDictation>;
  text: string;
  raw: string;
  aviso: string;
  path: string;
  applied?: string;
}

// `lost`: o ditado sumiu do armazenamento e a repetição fica só na tela; `storageIssue`: guardar a falha falhou.
export class DictationError extends Error {
  constructor(message: string, readonly lost: boolean, readonly storageIssue: string) {
    super(message);
  }
}

// O nome que o servidor deu ao áudio na pasta da conversa: `?arquivo=` e a rota de anexos pedem só ele.
export const uploadName = (path: string) => path.split(/[\\/]/).pop() || path;

// Cópia local enquanto existe; depois, o arquivo na pasta atual da conversa (áudio de antes de um
// /clear não está lá: o player mostra o erro de carregar).
export function dictationAudio(server: Server | undefined, name: string, audio: DraftAttachment | null,
  serverPath?: string | null): { uri: string; headers?: Record<string, string>; name: string } | null {
  if (audio) return { uri: audio.uri, name: audio.name };
  if (!server || !serverPath) return null;
  const file = uploadName(serverPath);
  return { uri: uploadUrlNative(name, file, server), headers: fileAuthHeader(server), name: file };
}

// Outra montagem da conversa relê o ditado guardado quando o draftUpdate muda.
function notify(serverId: string, name: string): void {
  chatStore(serverId, name).use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
}

/**
 * Ditado preso à conversa de origem, fora do ciclo de vida do Composer: trocar de conversa no meio
 * não descarta o resultado. Quem chama já gravou `voice`. `check` roda antes da rede; `isActive`, na
 * chegada, decide se o texto entra no campo aberto ou fica `ready` para a montagem. `stoppedAt` e
 * `fileAt`: a parada da gravação nova (toque, silêncio, teto ou tela escondida, em `voice.motivo`) e o arquivo
 * pronto no gravador, para medir cada etapa.
 */
export async function runDictation(server: Server, serverId: string, name: string, voice: DictationDraft,
  source: DictationSource, opts: { isActive: () => boolean; check?: () => void; stoppedAt?: number; fileAt?: number }): Promise<DictationOutcome> {
  const clock = stageClock();
  setDictationInFlight(serverId, name, voice.id, true);
  notify(serverId, name);
  try {
    opts.check?.();
    let arquivo: string;
    let uploaded = '';
    if ('file' in source) {
      // Sobe e guarda o caminho antes de transcrever: transcrição que cai não custa outra cópia.
      clock.enter('upload');
      ({ path: uploaded } = await uploadFileForServer(server, name, source.file, { audioOnly: true }));
      clock.enter('save');
      const pending = readDictation(serverId, name);
      if (pending?.id === voice.id) writeDictation(serverId, name, { ...pending, serverPath: uploaded });
      arquivo = uploadName(uploaded);
    } else {
      arquivo = source.arquivo;
    }
    clock.enter('transcribe');
    const res = await transcribeUploadedForServer(server, name, arquivo, { limpar: true, estilo: voice.estilo });
    const text = res.text.trim();
    if (!text) throw new Error(m.composer_transcricao_vazia());
    const raw = res.raw?.trim() ?? '';
    const aviso = res.aviso ?? '';
    // O nome solto do "de novo" não é caminho: sem `path` na resposta, fica o guardado.
    const path = res.path || uploaded || voice.serverPath || arquivo;
    const applied = res.estilo_aplicado ?? voice.estilo;
    clock.enter('finish');
    const completed = finishDictation(serverId, name, voice.id,
      { text, raw, issue: aviso, serverPath: path, applied }, opts.isActive());
    if (opts.stoppedAt !== undefined) recordTiming(server, { stoppedAt: opts.stoppedAt, fileAt: opts.fileAt }, clock, voice.motivo, undefined, !!aviso);
    return { completed, text, raw, aviso, path, applied };
  } catch (e) {
    const detail = e instanceof Error ? e.message : m.composer_falha_transcricao();
    if (opts.stoppedAt !== undefined) {
      // Na etapa `check` só há a conferência de quem chama: a recusa dela é prevista.
      const refused = clock.stage === 'check';
      recordTiming(server, { stoppedAt: opts.stoppedAt, fileAt: opts.fileAt }, clock, voice.motivo,
        refused ? { code: 'interrupted', expected: true } : { code: failureCode(detail) });
    }
    const issue = /^(501|503):/.test(detail) ? `${m.composer_ditado_indisponivel()}: ${detail}` : detail;
    let lost = false;
    let storageIssue = '';
    try {
      const latest = readDictation(serverId, name);
      lost = !latest;
      if (latest?.id === voice.id) {
        writeDictation(serverId, name, { ...latest, status: latest.text ? 'ready' : 'failed', issue });
      }
    } catch (storageError) {
      storageIssue = storageError instanceof Error ? storageError.message : m.draft_write_error();
    }
    throw new DictationError(issue, lost, storageIssue);
  } finally {
    setDictationInFlight(serverId, name, voice.id, false);
    notify(serverId, name);
  }
}

/**
 * Galeria: transcreve de novo um áudio que já está no servidor, para o ditado da conversa. Valida e
 * grava antes de devolver a promessa, para a galeria mostrar a recusa em vez de voltar.
 */
export function dictateUpload(server: Server, serverId: string, name: string, transcript: string | null,
  filename: string, estilo?: string): Promise<DictationOutcome> {
  const current = readDictation(serverId, name);
  if (current?.status === 'pending') throw new Error(m.composer_aguarde_transcricao());
  if (current?.status === 'applied') throw new Error(m.composer_ditado_aplicado());
  if (current?.status === 'ready') throw new Error(m.composer_ditado_recuperavel({ text: current.text.slice(0, 80) }));
  // Falha antes de subir: a cópia local é a única gravação, e substituir o ditado a apagaria.
  if (current?.audio && !current.serverPath) throw new Error(m.composer_ditado_so_no_aparelho());
  const draft = readDraft(serverId, name);
  const voice: DictationDraft = {
    version: 1, id: `${Date.now()}-${Math.random().toString(36).slice(2)}`, audio: null, serverPath: filename,
    transcript, draftRevision: draft?.revision ?? 0, before: draft?.text.trim() ?? '', motivo: 'botao', estilo,
    status: 'pending', text: '', raw: '', issue: '',
  };
  writeDictation(serverId, name, voice);
  // O ditado com falha que este substitui já subiu (`serverPath`): a cópia local sobra.
  if (current?.audio) {
    try { removeDraftAttachment(current.audio.uri); } catch { /* só um arquivo órfão na pasta do app */ }
  }
  return runDictation(server, serverId, name, voice, { arquivo: filename }, { isActive: () => false });
}
