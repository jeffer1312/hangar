import {
  fileAuthHeader, registrarDiag, relimparDitado, transcribeUploadedForServer, uploadFileForServer, uploadUrlNative, type Server,
} from '@hangar/core';
import * as m from '../../paraglide/messages';
import { chatStore } from '../../stores/chat';
import {
  attachDictationPath, finishDictation, keepDictationAudio, readDictation, readDraft, setDictationInFlight, writeDictation,
  type DictationDraft,
  type DraftAttachment,
} from '../../stores/drafts';
import { removeDraftAttachment } from '../../chat/draftAttachments';
import { useDictationOrganizationStore } from './dictationOrganization';
import { dictationStyleStore } from './ditadoEstiloStore';
import type { PiecesHandle } from './livePieces';

// `file`: gravação nova, sobe antes de transcrever; com `pieces`, o texto vem dos pedaços transcritos
// durante a fala e o arquivo sobe em paralelo. `arquivo`: nome solto na pasta da conversa, ou o
// `path` absoluto guardado (alcança o áudio de antes de um /clear; o convidado não pode usá-lo).
export type DictationSource = { file: File; pieces?: PiecesHandle | null } | { arquivo: string };

export interface DictationOutcome {
  completed: ReturnType<typeof finishDictation>;
  text: string;
  raw: string;
  aviso: string;
  path: string;
  applied?: string;
  /** Ditado em pedaços: a subida do arquivo inteiro, que termina depois do texto. Rejeita com a falha dela. */
  saved?: Promise<void>;
}

function dropLocalCopy(uri: string): void {
  try { removeDraftAttachment(uri); } catch { /* só um arquivo órfão na pasta do app */ }
}

const issueOf = (e: unknown) => {
  const detail = e instanceof Error ? e.message : m.composer_falha_transcricao();
  return /^(501|503):/.test(detail) ? `${m.composer_ditado_indisponivel()}: ${detail}` : detail;
};

// O cru dos pedaços organizado uma vez, como o `/transcribe` faria com o arquivo inteiro. O relimpar exige
// estilo: sem o da pílula, o salvo no servidor, e sem leitura dele o padrão do servidor.
async function organizePieces(server: Server, name: string, voice: DictationDraft, pieces: PiecesHandle) {
  const transcribed = await pieces.result;
  const raw = transcribed.raw.trim();
  if (!raw) throw new Error('pieces_empty');
  const style = dictationStyleStore(server).getState();
  const estilo = voice.estilo ?? (style.pronto ? style.valor : 'prosa');
  const organized = useDictationOrganizationStore.getState().byServer[server.id] === 'off'
    ? { text: raw, aviso: null, estilo_aplicado: 'cru' }
    : await relimparDitado(raw, estilo, server, { session: name });
  const text = organized.text.trim();
  if (!text) throw new Error('pieces_organized_empty');
  // Como o `with_provider` do servidor: o aviso da transcrição antes do da organização.
  const aviso = [transcribed.aviso, organized.aviso].filter(Boolean).join(' · ');
  return { text, raw, aviso, applied: organized.estilo_aplicado ?? estilo };
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
 * chegada, decide se o texto entra no campo aberto ou fica `ready` para a montagem.
 */
export async function runDictation(server: Server, serverId: string, name: string, voice: DictationDraft,
  source: DictationSource, opts: { isActive: () => boolean; check?: () => void }): Promise<DictationOutcome> {
  setDictationInFlight(serverId, name, voice.id, true);
  notify(serverId, name);
  const pieces = 'file' in source ? source.pieces : null;
  try {
    try {
      opts.check?.();
    } catch (e) {
      pieces?.abort();
      throw e;
    }
    let arquivo: string;
    let uploaded = '';
    if ('file' in source) {
      // Sobe e guarda o caminho antes de transcrever: transcrição que cai não custa outra cópia.
      const upload = uploadFileForServer(server, name, source.file, { audioOnly: true }).then(({ path }) => {
        const pending = readDictation(serverId, name);
        if (pending?.id === voice.id) writeDictation(serverId, name, { ...pending, serverPath: path });
        return path;
      });
      if (pieces) {
        // A falha da subida é de quem a espera: o texto dos pedaços não espera por ela.
        upload.catch(() => {});
        const organized = await organizePieces(server, name, voice, pieces).catch((e: unknown) => {
          pieces.abort();
          registrarDiag({ evento: 'dictation.pieces_fallback', nivel: 'aviso', codigo: 'pieces_failed',
            detalhe: e instanceof Error ? e.message : undefined });
          return null;
        });
        if (organized) {
          const { text, raw, aviso, applied } = organized;
          const completed = finishDictation(serverId, name, voice.id, { text, raw, issue: aviso, applied }, opts.isActive());
          const saved = upload.then(
            (path) => {
              if (!attachDictationPath(serverId, name, voice.id, path)) {
                registrarDiag({ evento: 'dictation.audio_unattached', nivel: 'aviso', codigo: 'dictation_already_left' });
              }
              // A cópia local sai quando nenhum ditado guardado a toca mais; o áudio está no servidor.
              if (voice.audio && readDictation(serverId, name)?.id !== voice.id) dropLocalCopy(voice.audio.uri);
              notify(serverId, name);
            },
            (e: unknown) => {
              const issue = issueOf(e);
              registrarDiag({ evento: 'dictation.upload_failed', nivel: 'erro', codigo: 'pieces_upload_failed', detalhe: issue });
              // Sem o arquivo no servidor, a cópia local é o único áudio: fica com o ditado enquanto ele existir.
              let kept = true;
              try {
                kept = keepDictationAudio(serverId, name, { ...voice, text, raw, applied, issue: '' }, issue);
              } catch (storageError) {
                // Sem conseguir guardar, a cópia fica no disco: apagar seria perder o áudio.
                registrarDiag({ evento: 'dictation.audio_unattached', nivel: 'erro', codigo: 'dictation_audio_not_stored',
                  detalhe: storageError instanceof Error ? storageError.message : undefined });
              }
              if (!kept) {
                registrarDiag({ evento: 'dictation.audio_unattached', nivel: 'aviso', codigo: 'dictation_already_left' });
                if (voice.audio) dropLocalCopy(voice.audio.uri);
              }
              notify(serverId, name);
              throw new Error(issue);
            },
          );
          // Quem chama escuta a mesma promessa; isto só evita a rejeição solta se ninguém escutar.
          saved.catch(() => {});
          return { completed, text, raw, aviso, path: '', applied, saved };
        }
      }
      uploaded = await upload;
      arquivo = uploadName(uploaded);
    } else {
      arquivo = source.arquivo;
    }
    const res = await transcribeUploadedForServer(server, name, arquivo, { limpar: true, estilo: voice.estilo });
    const text = res.text.trim();
    if (!text) throw new Error(m.composer_transcricao_vazia());
    const raw = res.raw?.trim() ?? '';
    const aviso = res.aviso ?? '';
    // O nome solto do "de novo" não é caminho: sem `path` na resposta, fica o guardado.
    const path = res.path || uploaded || voice.serverPath || arquivo;
    const applied = res.estilo_aplicado ?? voice.estilo;
    const completed = finishDictation(serverId, name, voice.id,
      { text, raw, issue: aviso, serverPath: path, applied }, opts.isActive());
    return { completed, text, raw, aviso, path, applied };
  } catch (e) {
    const issue = issueOf(e);
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
