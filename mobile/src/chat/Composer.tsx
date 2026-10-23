import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import * as Haptics from 'expo-haptics';
import { AccessibilityInfo, ActivityIndicator, Alert, AppState, Keyboard, Platform, Pressable, ScrollView, Text, View, type NativeSyntheticEvent, type TextInput, type TextInputKeyPressEventData } from 'react-native';
import type { NativeStackNavigationProp } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { broadcast, formataErro, uploadFileForServer, steerSession, podeEnviarSozinho, providerName, sendInputForServer, sideQuestionOf, slashMatches, slashTokenAt, replaceSlashToken, relimparDitado, estilosDitado, canUpdateQueueEdit, queueEditLeft, DELIVERY_STALLED } from '@hangar/core';
import type { CommandInfo, MotivoFim, Provider, QueueEdit, QueuePanel as QueuePanelModel, Server } from '@hangar/core';
import { ComposerActions, ComposerBox, ComposerInput } from './ComposerField';
import { Icon } from '../ui/Icon';
import * as m from '../paraglide/messages';
import { chatStore, filaCount as filaCountOf, submitConversationDraft, isSubmitting } from '../stores/chat';
import { abandonUnknownAttempt, attachInsert, confirmFirstInput, firstInputMessage, readFirstInput, sendFirstInput, useNewConversation, withAttach } from '../stores/newConversation';
import { useSessions } from '../stores/sessions';
import { clearDraft, clearRecoverableDraft, readDraft, readRecoverableDraft, resolveDraftTranscript, removeDraftRecording, reusableUploadPath, withoutUpload, writeDraft, writeRecoverableDraft, clearDictation, readDictation, writeDictation, recoverDictation, applyReadyDictation, associateDictationTranscript, type ConversationDraft, type DraftAttachment, type DictationDraft, type UploadTarget } from '../stores/drafts';
import { useServers } from '../stores/servers';
import { removeDraftAttachment, retainDraftAttachment } from './draftAttachments';
import { useNavigation, useRouter } from 'expo-router';
import { DictationStyleMenu, useDictationStyleLabel } from '../features/ditado/EstiloPill';
import { useDictationOrganization } from '../features/ditado/dictationOrganization';
import { useDitado } from '../features/ditado/useDitado';
import { dictationStyleStore } from '../features/ditado/ditadoEstiloStore';
import { DictationError, dictationAudio, runDictation, uploadName, type DictationSource } from '../features/ditado/dictationRun';
import { AudioChip } from './AudioChip';
import { CommandSheet, useSessionCommands } from './CommandSheet';
import { SessionSettingsButton } from './SessionSettings';
import { SideQuestionSheet } from './SideQuestionSheet';
import { SlashSuggest } from './SlashSuggest';
import { QueuePanel } from './QueuePanel';
import { queueSendMode, type DeliveryFeedback, type QueueSendMode } from './queueDelivery';
import { useDeliveryMode, useQueueDelivery } from './useQueueDelivery';
import { readClipboard, type PickedAttachment } from '../ui/attachmentPicker';
import { AttachSheet } from '../ui/AttachSheet';

import { AttachmentPreview } from '../ui/AttachmentPreview';
import { ContextRefTray } from '../ui/ContextRefTray';
import { CiteReplyModal } from './CiteReplyModal';
import { composeWithContextRefs, refsForMessage, splitContextRefs } from '@hangar/core';
import { contextRefsKey, contextRefsSnapshot, removeSentContextRefs, useContextRefsFor, type DraftContextRef } from '../stores/contextRefs';
import type { PastedFile } from '@mattermost/react-native-paste-input';

interface Props {
  serverId: string;
  name: string;
  draft?: string;
  // Texto devolvido pelo Parar/cancelar: entra antes do que já está no campo, nunca no lugar.
  returned?: { text: string };
  onReturnedAdopted?: () => void;
  settingsRequest?: { which: 'mode' | 'account'; n: number } | null;
  onSettingsRequestConsumed?: (n: number) => void;
  firstInputId?: string;
  firstInputSent?: boolean;
  sessionProvider?: string | null;
  // Claude/Codex sem terminal: a fila é do processo (dá para orientar) e pergunta pendente só sai pelo Parar.
  headless?: boolean;
  onStop?: () => void;
  stopping?: boolean;
  // Painel "Na fila", montado pela tela, que também esconde da conversa as mensagens dele.
  queuePanel: QueuePanelModel;
}

type PendingAttach = PickedAttachment;

// Como o PWA: os caminhos de todos os anexos numa linha só (o envio por terminal recusa quebra de linha).
const attachLine = (list: readonly PendingAttach[]) => list.map((a) => attachInsert(a, a.uploadedPath!)).join(' ');
const allUploaded = (list: readonly PendingAttach[]) => list.length > 0 && list.every((a) => a.uploadedPath);

// Texto que o envio atual produziria (citações, digitado e anexos já enviados), para comparar com o
// envio gravado no rascunho.
function composeForSend(typed: string, refs: readonly DraftContextRef[], attachments: readonly PendingAttach[]): string {
  const withRefs = composeWithContextRefs(refsForMessage(refs, typed), typed.trim());
  return allUploaded(attachments) ? withAttach(withRefs, attachLine(attachments)) : withRefs;
}

// O tamanho é só da prévia: fora do disco, para a comparação com o guardado não ver diferença.
const toDraftAttachments = (list: readonly PendingAttach[]): DraftAttachment[] => list.map(({ size: _size, ...a }) => a);
const mergeAttachments = (a: readonly DraftAttachment[], b: readonly DraftAttachment[]) =>
  [...a, ...b.filter((x) => !a.some((y) => y.uri === x.uri))];
// O caminho que o upload em segundo plano grava não é mudança da pessoa nos anexos.
const sameAttachments = (a: readonly DraftAttachment[], b: readonly DraftAttachment[]) =>
  JSON.stringify(a.map(withoutUpload)) === JSON.stringify(b.map(withoutUpload));
const mergeRecordings = (a: ConversationDraft['recordings'] = [], b: ConversationDraft['recordings'] = []) =>
  [...a, ...b.filter((recording) => !a.some((existing) => existing.id === recording.id))];

// O campo é só um `/nome` sendo digitado (nem argumento nem outro texto): é ele que o comando
// escolhido substitui por inteiro. Espaço no fim não conta: a lista abre com o cursor antes dele.
const soComando = (texto: string) => {
  const t = texto.trimEnd();
  return slashTokenAt(t, t.length)?.whole === true;
};

// Chamado depois que o rascunho largou a cópia: falhar aqui deixa só um arquivo órfão na pasta do app.
function dropCopy(uri: string): void {
  try { removeDraftAttachment(uri); } catch { /* sem perda de dado */ }
}

type DraftLoad = { draft: ConversationDraft | null; recoverable: ConversationDraft | null; dictation: DictationDraft | null; issue: string; blocked: boolean };

function loadDraft(serverId: string, name: string, transcript: string | null): DraftLoad {
  let resolved: ReturnType<typeof resolveDraftTranscript>;
  try {
    resolved = resolveDraftTranscript(serverId, name, transcript);
  } catch (e) {
    const issue = e instanceof Error ? e.message : m.draft_read_error();
    // Formato inválido não tem o que salvar; falha de leitura bloqueia gravar para não apagar o guardado.
    return { draft: null, recoverable: null, dictation: null, issue, blocked: issue !== m.draft_invalid() };
  }
  try {
    const recoverable = resolved.recoverable
      ? keepRecoverable(serverId, name, resolved.recoverable)
      : readRecoverableDraft(serverId, name);
    const dictation = transcript === null ? readDictation(serverId, name)
      : associateDictationTranscript(serverId, name, transcript);
    return { draft: resolved.draft, recoverable, dictation, issue: '', blocked: false };
  } catch (e) {
    // Com o antigo ainda só na chave principal, gravar o texto novo o apagaria.
    return { draft: resolved.draft, recoverable: null, dictation: null, issue: e instanceof Error ? e.message : m.draft_read_error(), blocked: true };
  }
}

const joinDrafts = (before: string, after: string) => (before.trim() && after.trim() ? `${before}\n${after}` : before.trim() ? before : after);

// Guarda o antigo antes de a sessão recriada gravar; um pendente de outra recriação é somado, nunca trocado.
function keepRecoverable(serverId: string, name: string, old: ConversationDraft): ConversationDraft {
  const pending = readRecoverableDraft(serverId, name);
  if (pending && pending.transcript === old.transcript && pending.revision === old.revision) return pending;
  const value = pending ? { ...old, text: joinDrafts(pending.text, old.text), attachments: mergeAttachments(pending.attachments, old.attachments),
    recordings: mergeRecordings(pending.recordings, old.recordings) } : old;
  writeRecoverableDraft(serverId, name, value);
  return value;
}

const VERSION_KEYS = ['cru', 'limpar', 'prosa', 'briefing'];
const versionOptions = () => [{ v: 'cru', label: m.composer_ditado_cru() },
  ...estilosDitado().map((e) => ({ v: e.valor as string, label: e.rotulo }))];

// Mesmo teto da transcrição no core; expirar desarma só o envio, nunca o ditado.
const VOICE_SEND_TIMEOUT_MS = 300_000;
type VoiceSendIntent = { mode: 'send' | 'steer'; draft: ConversationDraft; server: Server; recipients: string;
  generation: number; dictationId: string | null; cancelled: boolean; stopping: boolean };
// Barra do último ditado inserido (no fim do campo). `field` é o campo logo depois da inserção e
// `shown` a versão que está nele: como no PC, o texto ditado intacto mantém as versões à vista.
type DictationBar = { field: string; shown: string; raw: string; applied: string; versions: Record<string, string>; serverPath: string | null };
const barFor = (field: string, text: string, raw: string, applied: string, serverPath: string | null): DictationBar =>
  ({ field, shown: text, raw, applied, versions: raw ? { cru: raw, [applied]: text } : { [applied]: text }, serverPath });
const dictatedInField = (bar: DictationBar, value: string) => {
  const start = bar.field.length - bar.shown.length;
  return value.slice(start, start + bar.shown.length) === bar.shown;
};

export function Composer({ serverId, name, draft, returned, onReturnedAdopted, settingsRequest, onSettingsRequestConsumed, firstInputId, firstInputSent = false, sessionProvider, headless = false, onStop, stopping = false, queuePanel: queueList }: Props) {
  const { theme } = useUnistyles();
  const navigation = useNavigation<NativeStackNavigationProp<Record<string, object | undefined>>>();
  const inputRef = useRef<TextInput>(null);
  const restoreFocus = useRef(false);
  const stopRecordingRef = useRef<() => void>(() => {});
  const voiceSendRef = useRef<VoiceSendIntent | null>(null);
  const voiceSendTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const mountedRef = useRef(true);
  const [error, setError] = useState('');
  const [waitingVoiceSend, setWaitingVoiceSend] = useState(false);
  const deferVoiceSendRef = useRef<(mode: VoiceSendIntent['mode']) => boolean>(() => false);
  const cancelVoiceSend = useCallback((notify = true) => {
    const intent = voiceSendRef.current;
    const pending = intent && !intent.cancelled;
    // Conserva a intenção cancelada para não rearmar o autoenvio nesta gravação.
    if (intent) intent.cancelled = true;
    if (voiceSendTimerRef.current !== null) {
      clearTimeout(voiceSendTimerRef.current);
      voiceSendTimerRef.current = null;
    }
    if (!mountedRef.current) return;
    setWaitingVoiceSend(false);
    if (notify && pending) setError((current) => current || m.composer_voice_send_cancelled());
  }, []);
  useEffect(() => {
    const blur = navigation.addListener('blur', () => {
      restoreFocus.current = true;
      cancelVoiceSend();
      // Outra conversa empilhada por cima com o microfone aberto: para e transcreve para esta.
      stopRecordingRef.current();
    });
    const appeared = navigation.addListener('transitionEnd', ({ data }) => {
      if (data.closing || !navigation.isFocused() || !restoreFocus.current) return;
      restoreFocus.current = false;
      // O foco do leitor volta depois da animação, sem abrir o teclado.
      if (inputRef.current) AccessibilityInfo.sendAccessibilityEvent(inputRef.current, 'focus');
    });
    return () => { blur(); appeared(); };
  }, [navigation, cancelVoiceSend]);
  const chat = chatStore(serverId, name);
  const pending = chat.use((s) => s.pending);
  const events = chat.use((s) => s.events);
  const draftUpdate = chat.use((s) => s.draftUpdate);
  const state = chat.use((s) => s.stateEvent?.state ?? 'idle');
  const deliveryStalled = chat.use((s) => s.stateEvent?.problema === DELIVERY_STALLED);
  const detectedProvider = useSessions((s) => {
    const byServer = s.byServerRecord?.[serverId];
    if (byServer) {
      const hit = byServer.find((x) => x.name === name);
      if (hit?.provider) return hit.provider;
    }
    return (s.rows.find((x) => x.serverId === serverId && x.name === name)?.provider ?? null) as string | null;
  });
  const provider = sessionProvider ?? detectedProvider;
  const pairPeers = useSessions((s) => {
    const byServer = s.byServerRecord?.[serverId];
    return byServer?.find((x) => x.name === name)?.pair_peers ?? s.rows.find((x) => x.name === name)?.pair_peers ?? null;
  });
  const pairPeersKey = pairPeers?.join('\u0000') ?? '';
  const isCodex = provider === 'codex';
  const isClaude = !provider || provider === 'claude';
  const filaCount = filaCountOf({ events, pending }, provider);
  // Um seletor por campo: objeto novo por render faz o zustand renderizar sem parar.
  const queueEdit = chat.use((s) => s.queueEdit) ?? null;
  const queueBusy = chat.use((s) => s.queueBusy) ?? false;
  const queueError = chat.use((s) => s.queueError) ?? null;
  const deliveryFeedback = chat.use((s) => s.deliveryFeedback) ?? null;
  const editing = queueEdit !== null;
  // Citações da bandeja: entram na frente do texto só no envio, e comando de barra não as leva.
  const refsKey = contextRefsKey(serverId, name);
  const refs = useContextRefsFor(refsKey);

  const [sendToPair, setSendToPair] = useState(false);
  useEffect(() => {
    setSendToPair(false);
  }, [pairPeersKey]);

  // O Composer remonta por rota (key): a origem da montagem é o destino de toda gravação dele.
  const origin = useRef({ serverId, name }).current;
  const transcript = useSessions((s) => (s.byServerRecord?.[origin.serverId]?.find((x) => x.name === origin.name)?.jsonl
    ?? s.rows.find((x) => x.serverId === origin.serverId && x.name === origin.name)?.jsonl) || null);
  const transcriptRef = useRef(transcript);
  const [boot] = useState(() => loadDraft(origin.serverId, origin.name, transcript));
  const draftRef = useRef<ConversationDraft | null>(boot.draft);
  const blockedRef = useRef(boot.blocked);
  const [readBlocked, setReadBlocked] = useState(boot.blocked);
  const [draftIssue, setDraftIssue] = useState(boot.issue);
  const [recoverable, setRecoverable] = useState(boot.recoverable);
  const [dictation, setDictation] = useState(boot.dictation);
  const [submission, setSubmission] = useState(boot.draft?.submission ?? null);
  const [recordings, setRecordings] = useState(boot.draft?.recordings ?? []);
  const [pendingAttachments, setPendingAttachmentsState] = useState<PendingAttach[]>(boot.draft?.attachments ?? []);
  // Colar ou escolher vários chega em paralelo: cada um acrescenta à lista mais nova, nunca à do render.
  const attachmentsRef = useRef(pendingAttachments);
  const setPendingAttachments = useCallback((next: PendingAttach[]) => {
    attachmentsRef.current = next;
    setPendingAttachmentsState(next);
  }, []);
  // Rascunho guardado é mais novo que o texto de handoff/cancelamento que a rota ainda carrega.
  const adoptedDraftRef = useRef(draft);
  const [text, setText] = useState(() => boot.draft?.text ?? (firstInputSent ? '' : draft ?? ''));
  const textRef = useRef(text);
  useEffect(() => {
    textRef.current = text;
  }, [text]);

  // Só texto novo avança a revisão: é ela que decide se o ACK pode limpar o campo.
  const persistDraft = useCallback((patch: { text?: string; attachments?: readonly PendingAttach[]; recordings?: ConversationDraft['recordings'] }): boolean => {
    if (blockedRef.current) return false;
    // Em edição o campo tem o texto da fila, não o rascunho: o guardado continua o de antes da edição.
    if (patch.text !== undefined && patch.attachments === undefined && chat.use.getState().queueEdit) return true;
    let current: ConversationDraft | null;
    try { current = readDraft(origin.serverId, origin.name) ?? draftRef.current; } catch (e) {
      // Formato inválido não tem o que salvar: a edição nova grava por cima, como no boot.
      if (!(e instanceof Error && e.message === m.draft_invalid())) {
        setDraftIssue(e instanceof Error ? e.message : m.draft_read_error());
        return false;
      }
      current = draftRef.current;
    }
    if (current?.transcript && transcriptRef.current && current.transcript !== transcriptRef.current) current = draftRef.current;
    const base: ConversationDraft = current
      ?? { version: 1, text: '', revision: 0, transcript: transcriptRef.current, attachments: [], submission: null };
    const value: ConversationDraft = {
      ...base,
      transcript: base.transcript ?? transcriptRef.current,
      ...(patch.text !== undefined ? { text: patch.text, revision: base.revision + 1 } : {}),
      ...(patch.attachments !== undefined ? { attachments: toDraftAttachments(patch.attachments) } : {}),
      ...(patch.recordings !== undefined ? { recordings: patch.recordings } : {}),
    };
    try {
      if (!value.text && !value.attachments.length && !value.recordings?.length && !value.submission) clearDraft(origin.serverId, origin.name);
      else writeDraft(origin.serverId, origin.name, value);
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_write_error());
      return false;
    }
    if (patch.attachments !== undefined && voiceSendRef.current
      && !sameAttachments(value.attachments, voiceSendRef.current.draft.attachments)) cancelVoiceSend();
    draftRef.current = value;
    if (patch.recordings !== undefined) setRecordings(patch.recordings);
    setSubmission(value.submission);
    setDraftIssue('');
    return true;
  }, [origin, chat, cancelVoiceSend]);
  const persistText = useCallback((next: string) => persistDraft({ text: next }), [persistDraft]);

  // Cada anexo sobe assim que entra no campo; o Enviar só espera o que ainda está no ar. A chave é a
  // uri: anexo removido sai do mapa e o resultado que chegar depois não volta para a lista.
  const uploadsRef = useRef(new Map<string, Promise<string>>());
  const recordUpload = useCallback((uri: string, path: string, target: UploadTarget): boolean => {
    const list = attachmentsRef.current;
    if (!list.some((a) => a.uri === uri && a.uploadedPath !== path)) return true;
    const next = list.map((a) => (a.uri === uri ? { ...a, uploadedPath: path, uploadedFor: target } : a));
    if (!persistDraft({ attachments: next })) return false;
    setPendingAttachments(next);
    return true;
  }, [persistDraft, setPendingAttachments]);
  const startUpload = useCallback((attach: PendingAttach): Promise<string> => {
    const running = uploadsRef.current.get(attach.uri);
    if (running) return running;
    const target = { serverId: origin.serverId, name: origin.name, transcript: transcriptRef.current };
    const upload: Promise<string> = (async () => {
      const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
      if (!server) throw new Error(m.chat_servidor_removido());
      const blob = await (await fetch(attach.uri)).blob();
      const path = (await uploadFileForServer(server, origin.name, new File([blob], attach.name, { type: attach.mime }))).path;
      // Desmontado, a lista em memória é velha: gravá-la apagaria o que outra montagem pôs no rascunho.
      if (mountedRef.current && uploadsRef.current.get(attach.uri) === upload) recordUpload(attach.uri, path, target);
      return path;
    })();
    uploadsRef.current.set(attach.uri, upload);
    // Falhou, sai do mapa: o próximo pedido sobe de novo em vez de repetir o erro.
    upload.catch(() => { if (uploadsRef.current.get(attach.uri) === upload) uploadsRef.current.delete(attach.uri); });
    return upload;
  }, [origin, recordUpload]);

  // ACK do primeiro input depois do handoff: some o texto entregue, nunca uma edição nova.
  useEffect(() => {
    const handoff = adoptedDraftRef.current;
    if (!firstInputSent || !handoff || textRef.current.trim() !== handoff.trim()) return;
    adoptedDraftRef.current = undefined;
    if (persistText('')) setText('');
  }, [firstInputSent, persistText]);

  useEffect(() => {
    if ((draftRef.current?.text ?? '') !== text) persistText(text);
  }, [text, persistText]);

  useEffect(() => {
    if (blockedRef.current) return;
    try {
      setDictation(readDictation(origin.serverId, origin.name));
      const latest = readDraft(origin.serverId, origin.name);
      if (!latest) {
        setPendingAttachments([]);
        setRecordings([]);
        return;
      }
      if (latest.transcript && transcriptRef.current && latest.transcript !== transcriptRef.current) return;
      const unchanged = textRef.current === (draftRef.current?.text ?? '');
      draftRef.current = latest;
      setRecordings(latest.recordings ?? []);
      setSubmission(latest.submission);
      setPendingAttachments(latest.attachments.map((a) => ({ ...a, size: attachmentsRef.current.find((c) => c.uri === a.uri)?.size })));
      if (unchanged) {
        textRef.current = latest.text;
        setText(latest.text);
      }
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_read_error());
    }
  }, [draftUpdate, origin, setPendingAttachments]);

  const handleRecoverSubmission = useCallback(() => {
    if (isSubmitting(origin.serverId, origin.name) || !submission
      || (submission.status !== 'rejected' && submission.status !== 'unknown')) return;
    // Os anexos continuam no rascunho com o caminho enviado: devolver só o texto evita citá-los duas vezes.
    // Um anexo novo, mesmo já enviado em segundo plano, não está no texto recusado.
    const uploaded = pendingAttachments.filter((a) => a.uploadedPath && submission.text.includes(attachInsert(a, a.uploadedPath)));
    const insert = uploaded.length ? attachLine(uploaded) : null;
    const withoutAttachment = insert && submission.text.endsWith(insert)
      ? submission.text.slice(0, -insert.length).replace(/ — $/, '') : submission.text;
    // As citações não voltam ao campo: continuam na bandeja, que só esvazia quando o envio dá certo.
    const sent = splitContextRefs(withoutAttachment)?.typed ?? withoutAttachment;
    const next = textRef.current.trim() === sent.trim()
      ? textRef.current : joinDrafts(textRef.current, sent);
    try {
      const latest = readDraft(origin.serverId, origin.name);
      if (!latest?.submission || (latest.submission.status !== 'rejected' && latest.submission.status !== 'unknown')) return;
      const value = { ...latest, text: next, revision: latest.revision + 1, submission: null };
      writeDraft(origin.serverId, origin.name, value);
      draftRef.current = value;
      textRef.current = next;
      setText(next);
      setSubmission(null);
      setDraftIssue('');
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_write_error());
    }
  }, [submission, origin, pendingAttachments]);

  // null → caminho é a primeira confirmação (Codex iniciando); caminho → outro é sessão recriada.
  useEffect(() => {
    transcriptRef.current = transcript;
    const known = draftRef.current?.transcript;
    if (transcript === null || blockedRef.current) return;
    try {
      setDictation(associateDictationTranscript(origin.serverId, origin.name, transcript));
      const target = recordingTargetRef.current;
      if (target?.snapshot.transcript === null) {
        target.snapshot = { ...target.snapshot, transcript };
      }
      if (known === undefined || known === transcript) return;
      const { draft: kept, recoverable: old } = resolveDraftTranscript(origin.serverId, origin.name, transcript);
      if (old) {
        // Gravação que falhou deixou o campo à frente do guardado: conservar o que está na tela.
        const latest = draftRef.current?.text === textRef.current ? old : { ...old, text: textRef.current };
        const kept = keepRecoverable(origin.serverId, origin.name, latest);
        draftRef.current = null;
        setRecordings([]);
        setSubmission(null);
        setRecoverable(kept);
        // Upload no ar é da sessão que morreu: o anexo recuperado sobe de novo para a recriada.
        uploadsRef.current.clear();
        setPendingAttachments([]);
        setText('');
      } else {
        draftRef.current = kept ?? (draftRef.current && { ...draftRef.current, transcript });
      }
    } catch (e) {
      // Sem onde conservar o antigo, gravar o texto novo o apagaria.
      blockedRef.current = true;
      setReadBlocked(true);
      setDraftIssue(e instanceof Error ? e.message : m.draft_read_error());
    }
  }, [transcript, origin, setPendingAttachments]);

  useEffect(() => {
    const target = { serverId: origin.serverId, name: origin.name, transcript: transcriptRef.current };
    for (const a of pendingAttachments) {
      // A falha fica para o Enviar, que tenta de novo e mostra o motivo.
      if (!reusableUploadPath(a, target)) startUpload(a).catch(() => {});
    }
  }, [pendingAttachments, origin, startUpload]);

  const handleRecoverDraft = useCallback(() => {
    if (!recoverable) return;
    const next = joinDrafts(textRef.current, recoverable.text);
    // O upload antigo foi para a sessão que morreu: a recriada recebe o arquivo, nunca o caminho.
    const adopted = recoverable.attachments.length
      ? mergeAttachments(attachmentsRef.current, recoverable.attachments.map(withoutUpload)) : undefined;
    if (!persistDraft({ text: next, ...(adopted ? { attachments: adopted } : {}),
      recordings: mergeRecordings(draftRef.current?.recordings, recoverable.recordings) })) return;
    try {
      clearRecoverableDraft(origin.serverId, origin.name);
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error());
    }
    if (adopted) setPendingAttachments(adopted);
    setRecoverable(null);
    setText(next);
    setSelection({ start: next.length, end: next.length });
  }, [recoverable, persistDraft, origin, setPendingAttachments]);

  const handleDiscardDraft = useCallback(() => {
    try {
      clearRecoverableDraft(origin.serverId, origin.name);
      // Nada gravado desde a recriação: a chave principal ainda é o antigo e o reofereceria.
      if (!draftRef.current) clearDraft(origin.serverId, origin.name);
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error());
      return;
    }
    for (const a of recoverable?.attachments ?? []) {
      if (!attachmentsRef.current.some((c) => c.uri === a.uri)) dropCopy(a.uri);
    }
    setRecoverable(null);
  }, [origin, recoverable]);

  const handleRereadDraft = useCallback(() => {
    const load = loadDraft(origin.serverId, origin.name, transcriptRef.current);
    setDraftIssue(load.issue);
    if (load.blocked) return;
    // Texto igual ao último guardado não foi digitado no bloqueio: se for de sessão morta, já está no recuperável.
    const typed = textRef.current === (draftRef.current?.text ?? '') ? '' : textRef.current;
    blockedRef.current = false;
    setReadBlocked(false);
    draftRef.current = load.draft;
    setRecordings(load.draft?.recordings ?? []);
    setSubmission(load.draft?.submission ?? null);
    setPendingAttachments(load.draft?.attachments ?? []);
    setRecoverable(load.recoverable);
    setDictation(load.dictation);
    // O que a pessoa digitou enquanto a leitura falhava fica depois do guardado.
    const next = joinDrafts(load.draft?.text ?? '', typed);
    persistText(next);
    setText(next);
  }, [origin, persistText, setPendingAttachments]);
  const [sending, setSending] = useState(false);
  const sendingRef = useRef(false);
  const [uploading, setUploading] = useState(false);
  const steeringRef = useRef(false);
  const [steering, setSteering] = useState(false);
  // Aviso do painel da fila (a mensagem em edição saiu dela); some sozinho.
  const [queueNotice, setQueueNotice] = useState<string | null>(null);
  const [transcribing, setTranscribing] = useState(false);
  const transcribingRef = useRef(false);
  const activeRef = useRef(AppState.currentState === 'active');
  const activityGenerationRef = useRef(0);
  const recordingTargetRef = useRef<{ server: Server; snapshot: ConversationDraft; generation: number; estilo?: string } | null>(null);
  const micStartingRef = useRef(false);
  const [micStarting, setMicStarting] = useState(false);
  const [undo, setUndo] = useState<{ before: string; raw: string; revision: number } | null>(null);
  const [bar, setBarState] = useState<DictationBar | null>(null);
  const barRef = useRef<DictationBar | null>(null);
  const setBar = useCallback((next: DictationBar | null) => { barRef.current = next; setBarState(next); }, []);
  const [revising, setRevising] = useState<string | null>(null);
  const [failed, setFailed] = useState<{ file: File; motivo: MotivoFim; uri: string } | null>(null);
  const [autoN, setAutoN] = useState<number | null>(null);
  const [attachMenuOpen, setAttachMenuOpen] = useState(false);
  const [focado, setFocado] = useState(false);
  const [styleMenuOpen, setStyleMenuOpen] = useState(false);
  const styleServer = useServers((s) => s.servers.find((x) => x.id === origin.serverId) ?? null);
  const supportsDelivery = provider === 'claude' || provider === 'codex';
  // Lê o Jev mesmo parado e sem fila: o toque longo no Enviar escolhe o modo a qualquer momento.
  const delivery = useQueueDelivery(styleServer ?? undefined, supportsDelivery);
  // O envio decide o modo depois do upload: lê o modo e o Jev desta hora, não os do render que começou o envio.
  const deliveryRef = useRef(delivery);
  deliveryRef.current = delivery;
  const modeMenu = useMemo(() => supportsDelivery && !editing ? { delivery: { mode: delivery.mode, status: delivery.status, help: delivery.help },
    onPick: useDeliveryMode.getState().setMode } : undefined, [supportsDelivery, editing, delivery.mode, delivery.status, delivery.help]);
  const deliveryTiming = useCallback((message: string) => {
    const live = chat.use.getState();
    const { mode, status } = deliveryRef.current;
    return queueSendMode(mode, status === 'ready', live.stateEvent?.state ?? 'idle', provider,
      headless, message, live.events, live.askOpen ? live.askPayload : null);
  }, [chat, provider, headless]);
  // Jev salvo e indisponível vai pela fila; o aviso do painel só aparece com fila, então o envio avisa.
  const jevFallback = useCallback((message: string): DeliveryFeedback | null => {
    const { mode, status, help } = deliveryRef.current;
    if (mode !== 'jev' || status === 'ready' || status === 'loading') return null;
    const live = chat.use.getState();
    return queueSendMode('jev', true, live.stateEvent?.state ?? 'idle', provider, headless, message, live.events, null)
      ? { text: m.mobile_queue_jev_fallback({ reason: help.replace(/[\s.…]+$/, '') }), warning: true } : null;
  }, [chat, provider, headless]);
  const dictationStyle = useDictationStyleLabel(styleServer);
  // Organização desligada na máquina da sessão: todo estilo volta cru, então a barra não oferece versões.
  const organizationOff = useDictationOrganization(styleServer) === 'off';
  const [commandSheetOpen, setCommandSheetOpen] = useState(false);
  // Comandos e rodapé abrem a mesma folha; o número sobe a cada pedido.
  const [selectorRequest, setSelectorRequest] = useState<{ which: 'model' | 'effort' | 'mode' | 'account'; n: number } | null>(null);
  const lastSettingsRequest = useRef<number | null>(null);
  // null = pergunta lateral fechada; string = a pergunta que veio com o `/btw` (pode ser vazia).
  const [sideQuestion, setSideQuestion] = useState<string | null>(null);
  // Só é definido quando o app MOVE o cursor (ditado, undo, draft); o onSelectionChange devolve o
  // controle ao campo logo em seguida — preso, ele impediria a pessoa de mexer no cursor.
  const [selection, setSelection] = useState<{ start: number; end: number } | undefined>();
  // Seleção que o campo informou por último: decide qual palavra é o `/nome` das sugestões. null =
  // ainda não informou; vale o fim do texto.
  const [caret, setCaret] = useState<{ start: number; end: number } | null>(null);
  const undoTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const autoTimerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const autoAlvoRef = useRef<number>(0);
  const autoTextoRef = useRef<string>('');

  const limparUndo = useCallback(() => {
    setUndo(null);
    if (undoTimerRef.current) {
      clearTimeout(undoTimerRef.current);
      undoTimerRef.current = null;
    }
  }, []);

  const cancelarAuto = useCallback(() => {
    if (autoTimerRef.current) {
      clearInterval(autoTimerRef.current);
      autoTimerRef.current = null;
    }
    if (mountedRef.current) setAutoN(null);
    autoAlvoRef.current = 0;
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    activeRef.current = AppState.currentState === 'active';
    const sub = AppState.addEventListener('change', (next) => {
      activeRef.current = next === 'active';
      if (next !== 'active') {
        activityGenerationRef.current++;
        cancelVoiceSend();
        cancelarAuto();
      }
    });
    return () => {
      mountedRef.current = false;
      cancelVoiceSend();
      cancelarAuto();
      sub.remove();
    };
  }, [cancelarAuto, cancelVoiceSend]);

  const handleChangeText = useCallback(
    (v: string) => {
      cancelVoiceSend();
      textRef.current = v;
      persistText(v);
      setText(v);
      if (undo) limparUndo();
      if (autoN !== null) cancelarAuto();
    },
    [undo, autoN, limparUndo, cancelarAuto, persistText, cancelVoiceSend],
  );

  // draft devolvido pelo cancelar do picker (Task 3): adota quando muda
  useEffect(() => {
    if (draft === adoptedDraftRef.current) return;
    adoptedDraftRef.current = draft;
    if (draft !== undefined && draft !== text) {
      setText(draft);
      setSelection({ start: draft.length, end: draft.length });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft]);
  // Adota por identidade (o mesmo texto devolvido duas vezes entra as duas); o ref segura a
  // remontagem dupla do StrictMode antes de o pai limpar.
  const adoptedReturnedRef = useRef<{ text: string } | undefined>(undefined);
  useEffect(() => {
    if (!returned || returned === adoptedReturnedRef.current) return;
    adoptedReturnedRef.current = returned;
    const typed = textRef.current.trim() ? textRef.current : '';
    const next = typed ? `${returned.text}\n\n${typed}` : returned.text;
    textRef.current = next;
    setText(next);
    setSelection({ start: returned.text.length, end: returned.text.length });
    onReturnedAdopted?.();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [returned]);
  // Repetir um envio recusado com anexo já enviado gera o mesmo texto: o snapshot não bloqueia.
  const submissionBlocksSend = !!submission
    && (isSubmitting(serverId, name) || submission.text.trim() !== composeForSend(text, refs, pendingAttachments));
  const hasContent = text.trim().length > 0 || pendingAttachments.length > 0 || refs.length > 0;

  // A primeira mensagem já pode ter chegado antes de esta tela montar: nunca criar outro eco.
  const sendText = useCallback(async (value: string, revision?: number, explicit = false, timing: QueueSendMode | null = deliveryTiming(value) ?? null,
    notice: DeliveryFeedback | null = null): Promise<void> => {
    const pair = !!(sendToPair && pairPeers?.length);
    const mode = pair ? undefined : timing ?? undefined;
    // O aviso descreve um envio que aconteceu: os caminhos que não chamam o send não o mostram.
    const send = async () => {
      await (mode ? chat.send(value, revision, mode) : chat.send(value, revision));
      if (notice && !pair) chat.showDeliveryFeedback(notice);
    };
    if (!firstInputId) return send();
    const snapshot = readFirstInput(serverId, name);
    // Tentativa já confirmada e apagada enquanto o campo ainda mostrava o texto do handoff: não reenviar.
    const handoff = adoptedDraftRef.current;
    if (!snapshot && handoff !== undefined && value.trim() === handoff.trim()) {
      adoptedDraftRef.current = undefined;
      if (persistText('')) setText('');
      return;
    }
    if (snapshot?.id !== firstInputId) {
      return send();
    }
    if (snapshot.phase === 'send_unknown') {
      if (!explicit || readDraft(serverId, name)?.submission) {
        throw new Error(useNewConversation.getState().issues[serverId]?.message ?? m.nova_conversa_envio_incerto());
      }
      if (!abandonUnknownAttempt(serverId, snapshot.id)) {
        throw new Error(useNewConversation.getState().issues[serverId]?.message ?? m.nova_conversa_envio_incerto());
      }
      return send();
    }
    if (value.trim() !== firstInputMessage(snapshot).trim()) {
      return send();
    }
    if (snapshot.phase === 'created') await sendFirstInput(serverId, snapshot.id);
    const current = readFirstInput(serverId, name);
    if (!current || current.id !== snapshot.id) return;
    if (current.phase === 'sent') {
      confirmFirstInput(snapshot.id);
      return;
    }
    const issue = useNewConversation.getState().issues[serverId];
    throw new Error(issue?.message ?? (current.phase === 'created'
      ? m.composer_falha_envio() : m.nova_conversa_envio_incerto()));
  }, [serverId, name, firstInputId, draft, chat, persistText, deliveryTiming, sendToPair, pairPeers]);

  const handleSend = useCallback(async (voiceReady = false) => {
    if (voiceReady && (chat.use.getState().queueEdit || submissionBlocksSend || sendingRef.current || sending || uploading
      || blockedRef.current || (!textRef.current.trim() && !pendingAttachments.length)
      || !activeRef.current || !navigation.isFocused())) {
      setError((current) => current || m.composer_voice_send_cancelled());
      return;
    }
    // Em edição o botão é o Atualizar: troca o texto da mensagem na fila, sem enviar nada.
    if (queueEdit) {
      if (canUpdateQueueEdit(text, queueEdit.refs)) await chat.submitQueueEdit(text);
      return;
    }
    if (!voiceReady && deferVoiceSendRef.current('send')) return;
    const trimmed = textRef.current.trim();
    const hasAttach = pendingAttachments.length > 0;
    const sentRefs = refsForMessage(refs, trimmed);
    if (!trimmed && !hasAttach && !sentRefs.length) return;
    if (submissionBlocksSend) return;
    if (sendingRef.current || sending || uploading) return;
    // `/btw` não é mensagem nem entra na fila: abre a pergunta lateral. Só o Claude tem o comando.
    const btw = !hasAttach && !sentRefs.length && isClaude ? sideQuestionOf(trimmed) : null;
    if (btw !== null) {
      if (!persistText('')) return;
      textRef.current = '';
      setText('');
      setSideQuestion(btw);
      return;
    }
    if (blockedRef.current || !persistText(textRef.current)) return;
    const sentRevision = draftRef.current!.revision;
    sendingRef.current = true;
    limparUndo();
    cancelarAuto();
    // Como no app do Claude: enviou, o teclado desce e a conversa aparece inteira.
    Keyboard.dismiss();
    setSending(true);
    setError('');
    const stop = () => {
      setSending(false);
      sendingRef.current = false;
    };
    const body = composeWithContextRefs(sentRefs, trimmed);
    let finalText = body;
    // Remover anexo fica desligado durante o envio; colar no meio dele acrescenta um que espera a próxima vez.
    const sentAttachments = attachmentsRef.current;
    if (sentAttachments.length) {
      const target = { serverId: origin.serverId, name: origin.name, transcript: transcriptRef.current };
      const inserts: string[] = [];
      setUploading(true);
      try {
        for (const attach of sentAttachments) {
          let path = reusableUploadPath(attach, target);
          if (!path) {
            if (!useServers.getState().servers.some((s) => s.id === origin.serverId)) {
              setError(m.chat_servidor_removido());
              stop();
              return;
            }
            // Reaproveita o upload no ar e, se ele falhar, tenta uma vez mais; falha anterior já saiu do mapa.
            const inFlight = uploadsRef.current.get(attach.uri);
            try {
              path = await startUpload(attach).catch((e: unknown) => {
                if (!inFlight) throw e;
                return startUpload(attach);
              });
            } catch (e) {
              // O backend diz o motivo (tamanho, formato); o texto e os anexos continuam no rascunho, e
              // os que já subiram guardam o caminho para não subir de novo.
              setError(e instanceof Error && e.message ? `${m.board_falha_upload()}: ${e.message}` : m.board_falha_upload());
              stop();
              return;
            }
            if (!recordUpload(attach.uri, path, target)) {
              stop();
              return;
            }
          }
          inserts.push(attachInsert(attach, path));
        }
      } finally {
        setUploading(false);
      }
      finalText = withAttach(body, inserts.join(' '));
    }
    if (!finalText.trim()) {
      stop();
      return;
    }
    // Depois do upload: o turno pode ter acabado nele, e Direto ou Jev numa sessão parada não fazem sentido.
    let timing: QueueSendMode | undefined = deliveryTiming(trimmed);
    if (sentAttachments.length && typeof timing === 'object' && timing.kind === 'jev') timing = { ...timing, task: '' };
    const fallbackNotice = timing ? null : jevFallback(trimmed);
    // Como o texto, os anexos enviados saem do campo já; só a falha os devolve, com o caminho do upload.
    const uploadedSent = attachmentsRef.current.filter((a) => sentAttachments.some((x) => x.uri === a.uri));
    const unsent = attachmentsRef.current.filter((a) => !uploadedSent.includes(a));
    const attachmentsLeft = uploadedSent.length > 0 && persistDraft({ attachments: unsent });
    if (attachmentsLeft) setPendingAttachments(unsent);
    let groupPendingId: string | null = null;
    try {
      const snapshot = firstInputId ? readFirstInput(serverId, name) : null;
      const firstDraft = !!snapshot && snapshot.id === firstInputId && firstInputMessage(snapshot).trim() === finalText.trim();
      const sendToPairNow = !firstDraft && sendToPair && !!pairPeers?.length && !finalText.trimStart().startsWith('/');
      if (sendToPairNow && pairPeers?.length) {
        const server = useServers.getState().servers.find((s) => s.id === serverId);
        if (!server) throw new Error(m.chat_servidor_removido());
        const recipients = [name, ...pairPeers];
        await submitConversationDraft(serverId, name, finalText, async () => {
          groupPendingId = `pending-group-${Date.now()}`;
          chat.use.setState((current) => ({ pending: [...current.pending, { id: groupPendingId!, text: finalText }] }));
          const results = await broadcast(recipients, finalText, server);
          const failedRecipients = recipients.filter((recipient) => !results[recipient]?.ok);
          if (failedRecipients.length) {
            const delivered = recipients.filter((recipient) => results[recipient]?.ok);
            const detail = failedRecipients.map((recipient) => formataErro(results[recipient]?.error)).find(Boolean);
            throw new Error(
              `${delivered.length ? m.chat_chegou_mas({ n: delivered.join(', ') }) : ''}${m.chat_nao_chegou_em()}${failedRecipients.join(', ')} (${detail ?? m.board_falha_envio()})`,
            );
          }
        }, sentRevision);
      } else {
        await sendText(finalText, sentRevision, true, timing ?? null, fallbackNotice);
      }
      removeSentContextRefs(refsKey, sentRefs);
      setBar(null);
      // Só saem os enviados: colar durante o envio acrescenta um que ainda espera a vez.
      const rest = attachmentsRef.current.filter((a) => !sentAttachments.some((x) => x.uri === a.uri));
      if (sentAttachments.length && persistDraft({ attachments: rest })) {
        for (const a of sentAttachments) {
          uploadsRef.current.delete(a.uri);
          dropCopy(a.uri);
        }
        setPendingAttachments(rest);
        // Outra tela desta conversa aberta durante o envio ainda mostra o anexo: o aviso vem depois da limpeza.
        chat.use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
      }
    } catch (e) {
      if (groupPendingId) {
        chat.use.setState((current) => ({ pending: current.pending.filter((item) => item.id !== groupPendingId) }));
      }
      if (attachmentsLeft) {
        const back = mergeAttachments(uploadedSent, attachmentsRef.current);
        if (persistDraft({ attachments: back })) setPendingAttachments(back);
      }
      const msg = e instanceof Error ? e.message : m.composer_falha_envio();
      setError(msg);
    } finally {
      setSending(false);
      sendingRef.current = false;
    }
  }, [text, sending, uploading, chat, sendText, limparUndo, cancelarAuto, pendingAttachments, setPendingAttachments, serverId, name, firstInputId, pairPeers, sendToPair, persistText, persistDraft, origin, submissionBlocksSend, isClaude, setBar, queueEdit, navigation, refs, refsKey, deliveryTiming, jevFallback, startUpload, recordUpload]);

  // auto-envio: contagem de 3s
  const iniciarAuto = useCallback(
    (textoParaEnviar: string) => {
      cancelarAuto();
      const generation = activityGenerationRef.current;
      const revision = draftRef.current?.revision;
      autoTextoRef.current = textoParaEnviar;
      autoAlvoRef.current = Date.now() + 3000;
      setAutoN(3);
      autoTimerRef.current = setInterval(() => {
        if (!mountedRef.current || !activeRef.current || generation !== activityGenerationRef.current
          || textRef.current !== textoParaEnviar || draftRef.current?.revision !== revision) {
          cancelarAuto();
          return;
        }
        const rest = autoAlvoRef.current - Date.now();
        if (rest <= 0) {
          cancelarAuto();
          const typed = autoTextoRef.current.trim();
          // Mesmo caminho do botão Enviar: as citações da bandeja vão na frente e saem dela só se
          // o envio der certo. Comando de barra não leva citação.
          const sentRefs = refsForMessage(contextRefsSnapshot(refsKey), typed);
          const toSend = composeWithContextRefs(sentRefs, typed);
          if (!toSend || sendingRef.current) return;
          sendingRef.current = true;
          limparUndo();
          setSending(true);
          void sendText(toSend, revision)
            .then(() => { removeSentContextRefs(refsKey, sentRefs); setError(''); setBar(null); })
            .catch((e: unknown) => {
              const msg = e instanceof Error ? e.message : m.composer_falha_envio();
              setError(msg);
            })
            .finally(() => { sendingRef.current = false; setSending(false); });
          return;
        }
        setAutoN(Math.ceil(rest / 1000));
      }, 250);
    },
    [cancelarAuto, sendText, limparUndo, setBar, refsKey],
  );

  const offerUndo = useCallback((before: string, raw: string, cleaned: string, revision: number) => {
    limparUndo();
    if (!raw.trim() || raw.trim() === cleaned.trim()) return;
    setUndo({ before, raw: raw.trim(), revision });
    undoTimerRef.current = setTimeout(limparUndo, 10_000);
  }, [limparUndo]);

  const runTranscription = useCallback(async (voice: DictationDraft, source: DictationSource, server: Server, generation: number, allowAuto: boolean) => {
    if (transcribingRef.current) return;
    const intent = voiceSendRef.current;
    if (intent && !intent.dictationId) intent.dictationId = voice.id;
    transcribingRef.current = true;
    if (mountedRef.current) {
      setTranscribing(true);
      setError((current) => voiceSendRef.current?.cancelled ? current : '');
      setFailed(null);
    }
    try {
      writeDictation(origin.serverId, origin.name, voice);
    } catch (e) {
      // Sem o áudio guardado, a repetição fica só nesta tela.
      cancelVoiceSend();
      if (mountedRef.current) {
        if ('file' in source && voice.audio) setFailed({ file: source.file, motivo: voice.motivo, uri: voice.audio.uri });
        setError(e instanceof Error ? e.message : m.draft_write_error());
        setTranscribing(false);
      }
      transcribingRef.current = false;
      return;
    }
    if (mountedRef.current) setDictation(voice);
    try {
      const { completed, text: trimmed, raw, aviso, path, applied } = await runDictation(server, origin.serverId, origin.name, voice, source, {
        check: () => {
          const sessions = useSessions.getState();
          const live = sessions.byServerRecord?.[origin.serverId]?.find((s) => s.name === origin.name)
            ?? sessions.rows.find((s) => s.serverId === origin.serverId && s.name === origin.name);
          const currentTranscript = live ? live.jsonl || null : transcriptRef.current;
          // Pelo caminho guardado o servidor acha o áudio mesmo depois de um /clear; o resultado fica `ready`.
          if ('file' in source && voice.transcript !== null && currentTranscript !== voice.transcript) {
            throw new Error(m.composer_ditado_anterior());
          }
          // Só o app em segundo plano adia o POST (o sistema pode matá-lo); trocar de conversa não.
          if (AppState.currentState !== 'active' || generation !== activityGenerationRef.current) {
            throw new Error(m.composer_ditado_interrompido());
          }
        },
        isActive: () => mountedRef.current && activeRef.current && !blockedRef.current && navigation.isFocused()
          && generation === activityGenerationRef.current
          && textRef.current.trim() === voice.before
          && (voice.transcript === null || voice.transcript === transcriptRef.current),
      });
      if (aviso || !completed.draft) cancelVoiceSend(!aviso);
      if (mountedRef.current) {
        setDictation(completed.dictation);
        if (completed.draft) {
          draftRef.current = completed.draft;
          setRecordings(completed.draft.recordings ?? []);
          textRef.current = completed.draft.text;
          setText(completed.draft.text);
          setSelection({ start: completed.draft.text.length, end: completed.draft.text.length });
          offerUndo(voice.before, raw, trimmed, completed.draft.revision);
          setBar(barFor(completed.draft.text, trimmed, raw, applied ?? 'cru', path));
          if (aviso) setError(aviso);
          if (allowAuto && !voiceSendRef.current && podeEnviarSozinho({
            motivo: voice.motivo, texto: trimmed, aviso: aviso || null,
            rascunhoAntes: !!voice.before.trim() || completed.draft.attachments.length > 0 || !!completed.draft.submission,
          })) iniciarAuto(completed.draft.text);
        }
      }
      if (completed.draft && !completed.dictation && voice.audio) dropCopy(voice.audio.uri);
    } catch (e) {
      cancelVoiceSend();
      if (mountedRef.current) {
        if (e instanceof DictationError) {
          if (e.lost && 'file' in source && voice.audio) setFailed({ file: source.file, motivo: voice.motivo, uri: voice.audio.uri });
          if (e.storageIssue) setDraftIssue(e.storageIssue);
        }
        try {
          setDictation(readDictation(origin.serverId, origin.name));
        } catch (readError) {
          setDraftIssue(readError instanceof Error ? readError.message : m.draft_read_error());
        }
        setError(e instanceof Error ? e.message : m.composer_falha_transcricao());
      }
    } finally {
      transcribingRef.current = false;
      if (mountedRef.current) setTranscribing(false);
    }
  }, [origin, offerUndo, iniciarAuto, navigation, setBar, cancelVoiceSend]);

  const handleTranscribe = useCallback(
    async (file: File, motivo: MotivoFim, uri: string, allowAuto = true) => {
      const target = recordingTargetRef.current;
      if (!target || transcribingRef.current) return;
      const generation = activityGenerationRef.current;
      // Bloqueia outro início enquanto a cópia sai do cache para o diretório do app.
      transcribingRef.current = true;
      if (mountedRef.current) setTranscribing(true);
      let audio: DraftAttachment;
      try {
        audio = await retainDraftAttachment({ uri, name: file.name, mime: file.type, kind: 'file' });
      } catch (e) {
        cancelVoiceSend();
        if (mountedRef.current) {
          setFailed({ file, motivo, uri });
          setError(e instanceof Error ? e.message : m.draft_write_error());
          setTranscribing(false);
        }
        transcribingRef.current = false;
        return;
      }
      transcribingRef.current = false;
      const voice: DictationDraft = {
        version: 1, id: `${Date.now()}-${Math.random().toString(36).slice(2)}`, audio,
        transcript: target.snapshot.transcript, draftRevision: target.snapshot.revision,
        before: target.snapshot.text.trim(), motivo, estilo: target.estilo,
        status: 'pending', text: '', raw: '', issue: '',
      };
      await runTranscription(voice, { file }, target.server, generation, allowAuto && generation === target.generation);
    },
    [runTranscription, cancelVoiceSend],
  );

  const { gravando, rms, iniciar, parar } = useDitado({
    server: styleServer,
    onFim: handleTranscribe,
    onErroParada: (e) => {
      cancelVoiceSend();
      if (mountedRef.current) setError(e.message === 'ditado_parada_falhou' ? m.composer_falha_gravacao() : e.message || m.composer_falha_gravacao());
    },
  });
  stopRecordingRef.current = () => { if (gravando) void parar('escondeu'); };

  const handleCancelRecording = useCallback(() => {
    // A parada pedida por Enviar ainda deve guardar o áudio.
    if (!voiceSendRef.current) recordingTargetRef.current = null;
    cancelVoiceSend();
    void parar('botao');
  }, [cancelVoiceSend, parar]);

  const handleMicPress = useCallback(async () => {
    if (transcribingRef.current || micStartingRef.current) return;
    if (gravando) {
      void parar('botao');
      return;
    }
    if (dictation || !mountedRef.current || !activeRef.current) return;
    // Outra montagem pode ter começado a transcrever depois do último render desta.
    try {
      if (readDictation(origin.serverId, origin.name)?.status === 'pending') {
        setError(m.composer_aguarde_transcricao());
        return;
      }
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_read_error());
      return;
    }
    if (autoN !== null) cancelarAuto();
    micStartingRef.current = true;
    setMicStarting(true);
    try {
      const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
      if (!server) throw new Error(m.chat_servidor_removido());
      if (blockedRef.current || !persistText(textRef.current)) return;
      const snapshot = draftRef.current!;
      // Mesmo vazio precisa de revisão em disco para comparar o retorno da voz.
      writeDraft(origin.serverId, origin.name, snapshot);
      const style = dictationStyleStore(server).getState();
      recordingTargetRef.current = { server: { ...server }, snapshot,
        generation: activityGenerationRef.current, estilo: style.pronto ? style.valor : undefined };
      voiceSendRef.current = null;
      setBar(null);
      await iniciar();
      // O diálogo de permissão pode resolver antes de chegar o AppState active.
      if (mountedRef.current && recordingTargetRef.current) {
        recordingTargetRef.current.generation = activityGenerationRef.current;
      }
    } catch (e) {
      const msg = e instanceof Error ? e.message : '';
      if (msg === 'permission_denied') {
        setError(m.composer_sem_acesso_mic());
      } else {
        setError(e instanceof Error ? e.message : m.composer_falha_gravacao());
      }
    } finally {
      micStartingRef.current = false;
      if (mountedRef.current) setMicStarting(false);
    }
  }, [gravando, iniciar, parar, autoN, cancelarAuto, dictation, origin, persistText, setBar]);

  const handleUndo = useCallback(() => {
    if (!undo) return;
    cancelarAuto();
    if (draftRef.current?.revision !== undo.revision) { limparUndo(); return; }
    const { before, raw } = undo;
    const restored = before ? `${before} ${raw}` : raw;
    if (!persistText(restored)) return;
    textRef.current = restored;
    setText(restored);
    limparUndo();
    setSelection({ start: restored.length, end: restored.length });
  }, [undo, limparUndo, cancelarAuto, persistText]);

  // Como no PC: o cru já está aqui; os estilos saem do /ditado/relimpar e ficam guardados na barra.
  // Campo editado fora do texto ditado recusa a troca, para não perder o que foi digitado.
  const handleVersion = useCallback(async (version: string) => {
    const current = barRef.current;
    if (!current || revising !== null) return;
    // A contagem levaria o texto da versão anterior.
    cancelarAuto();
    if (textRef.current !== current.field) { setError(m.native_dictation_draft_changed()); return; }
    let next = current.versions[version];
    let applied = version;
    if (next === undefined) {
      const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
      if (!server) { setError(m.chat_servidor_removido()); return; }
      setRevising(version);
      setError('');
      try {
        const r = await relimparDitado(current.raw, version, { ...server }, { session: origin.name });
        next = r.text.trim();
        if (!next) { setError(m.composer_transcricao_vazia()); return; }
        // A limpeza pode desistir e devolver o cru: marca o que o servidor aplicou, não o botão.
        applied = r.estilo_aplicado && VERSION_KEYS.includes(r.estilo_aplicado) ? r.estilo_aplicado : version;
        if (r.aviso) setError(r.aviso);
      } catch (e) {
        if (mountedRef.current) setError(e instanceof Error ? e.message : m.composer_falha_transcricao());
        return;
      } finally {
        if (mountedRef.current) setRevising(null);
      }
    }
    if (!mountedRef.current || barRef.current !== current) return;
    if (textRef.current !== current.field) { setError(m.native_dictation_draft_changed()); return; }
    const field = current.field.slice(0, current.field.length - current.shown.length) + next;
    if (!persistText(field)) return;
    textRef.current = field;
    setText(field);
    setSelection({ start: field.length, end: field.length });
    limparUndo();
    setBar({ ...current, field, shown: next, applied, versions: { ...current.versions, [applied]: next } });
  }, [revising, origin, persistText, limparUndo, setBar, cancelarAuto]);

  const handleRetry = useCallback(async () => {
    if (transcribingRef.current) return;
    cancelVoiceSend();
    if (failed) { await handleTranscribe(failed.file, failed.motivo, failed.uri, false); return; }
    if (!dictation || dictation.status !== 'failed') return;
    cancelarAuto();
    const generation = activityGenerationRef.current;
    transcribingRef.current = true;
    setTranscribing(true);
    try {
      const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
      if (!server) throw new Error(m.chat_servidor_removido());
      const sameTranscript = dictation.transcript === transcriptRef.current;
      let source: DictationSource;
      // O arquivo já enviado não sobe de novo. Nome solto enquanto a pasta da conversa é a mesma
      // (convidado só pode esse); o caminho absoluto só depois de um /clear, e aí o `isActive` dá
      // falso pelo jsonl e o resultado volta `ready`, com "Recuperar".
      if (dictation.serverPath) {
        source = { arquivo: sameTranscript ? uploadName(dictation.serverPath) : dictation.serverPath };
      } else if (dictation.audio) {
        if (dictation.transcript !== null && !sameTranscript) {
          setError(m.composer_ditado_anterior());
          return;
        }
        const blob = await (await fetch(dictation.audio.uri)).blob();
        source = { file: new File([blob], dictation.audio.name, { type: dictation.audio.mime }) };
      } else return;
      transcribingRef.current = false;
      // Repetir é explícito, mas nunca rearma o autoenvio cancelado.
      await runTranscription({ ...dictation, id: `${Date.now()}-${Math.random().toString(36).slice(2)}`,
        status: 'pending', issue: '' }, source, { ...server }, generation, false);
    } catch (e) {
      if (mountedRef.current) setError(e instanceof Error ? e.message : m.composer_falha_transcricao());
    } finally {
      transcribingRef.current = false;
      if (mountedRef.current) setTranscribing(false);
    }
  }, [failed, handleTranscribe, dictation, origin, runTranscription, cancelarAuto, cancelVoiceSend]);

  // Recuperar e a inserção automática levam o texto ao campo do mesmo jeito.
  const adoptDictation = useCallback((voice: DictationDraft, before: string,
    result: { draft: ConversationDraft | null; dictation: DictationDraft | null }) => {
    setDictation(result.dictation);
    if (!result.draft) return;
    draftRef.current = result.draft;
    setRecordings(result.draft.recordings ?? []);
    textRef.current = result.draft.text;
    setText(result.draft.text);
    setSelection({ start: result.draft.text.length, end: result.draft.text.length });
    offerUndo(before, voice.raw, voice.text, result.draft.revision);
    setBar(barFor(result.draft.text, voice.text, voice.raw, voice.applied ?? voice.estilo ?? 'cru', voice.serverPath ?? null));
    if (!result.dictation && voice.audio) dropCopy(voice.audio.uri);
    setError((current) => voice.issue || (voiceSendRef.current?.cancelled ? current : ''));
  }, [offerUndo, setBar]);

  const handleRecoverDictation = useCallback(() => {
    if (!dictation?.text || dictation.status !== 'ready' || transcribingRef.current) return;
    cancelarAuto();
    const before = textRef.current.trim();
    if (!persistText(textRef.current)) return;
    try {
      adoptDictation(dictation, before, recoverDictation(origin.serverId, origin.name, dictation.id));
    } catch (e) { setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error()); }
  }, [dictation, cancelarAuto, persistText, adoptDictation, origin]);

  const handleDiscardDictation = useCallback(() => {
    if (!dictation || transcribingRef.current) return;
    // O ditado sai antes do áudio no rascunho: falhar no meio deixa o chip, que se tira à parte.
    try { clearDictation(origin.serverId, origin.name); } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error());
      return;
    }
    if (dictation.audio) dropCopy(dictation.audio.uri);
    setDictation(null);
    setError('');
    if (dictation.status !== 'applied') return;
    try {
      const latest = removeDraftRecording(origin.serverId, origin.name, dictation.id);
      draftRef.current = latest;
      setRecordings(latest?.recordings ?? []);
    } catch (e) { setDraftIssue(e instanceof Error ? e.message : m.draft_write_error()); }
  }, [dictation, origin]);

  // Ditado que voltou fora da tela só entra no rascunho intacto com a conversa ativa e em foco.
  // `transcribing` nas dependências: o aviso do fim do POST chega antes de esta tela soltar a trava.
  useEffect(() => {
    if (!mountedRef.current || !activeRef.current || !navigation.isFocused() || blockedRef.current || transcribingRef.current) return;
    if (textRef.current !== (draftRef.current?.text ?? '')) return;
    try {
      const before = textRef.current.trim();
      const applied = applyReadyDictation(origin.serverId, origin.name, transcriptRef.current);
      if (applied) {
        if (applied.voice.issue) cancelVoiceSend(false);
        adoptDictation(applied.voice, before, applied);
      }
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error());
    }
  }, [draftUpdate, transcript, transcribing, origin, adoptDictation, cancelVoiceSend, navigation]);

  const handleSteer = useCallback(async () => {
    if (steeringRef.current) return;
    steeringRef.current = true;
    setSteering(true);
    setError('');
    try {
      const server = useServers.getState().servers.find((s) => s.id === serverId);
      if (!server) { setError(m.chat_servidor_removido()); return; }
      chat.applySteer(await steerSession(name, undefined, server));
    } catch (e) {
      const status = (e as { status?: number } | null)?.status;
      setError(status === 409 ? m.composer_fila_erro() : e instanceof Error ? e.message : m.composer_fila_erro());
    } finally {
      steeringRef.current = false;
      setSteering(false);
    }
  }, [serverId, name, chat]);

  const isKimi = provider === 'kimi';
  // Só o Kimi promove a fila inteira por ctrl-s; orientar texto do campo é outra ação.
  const showSteer = isKimi && state === 'working' && (filaCount > 0 || steering);

  const originServer = useServers((s) => s.servers.find((x) => x.id === origin.serverId));
  const dictationPlayer = dictation ? dictationAudio(originServer, origin.name, dictation.audio, dictation.serverPath) : null;
  // As versões acompanham o trecho intacto; os áudios continuam disponíveis após editar o texto.
  const barIntact = !!bar && (revising !== null || dictatedInField(bar, text));
  const barVersions = barIntact && !!bar?.raw && !organizationOff;
  const dictationMessage = dictation ? dictation.status === 'applied' ? m.composer_ditado_aplicado()
    : dictation.text ? m.composer_ditado_recuperavel({ text: dictation.text.slice(0, 80) })
    : dictation.issue || m.composer_ditado_interrompido() : '';
  const handleRemoveRecording = useCallback((id: string) => {
    if (sendingRef.current || blockedRef.current) return;
    try {
      const latest = removeDraftRecording(origin.serverId, origin.name, id);
      draftRef.current = latest;
      setRecordings(latest?.recordings ?? []);
      chat.use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
    } catch (e) { setDraftIssue(e instanceof Error ? e.message : m.draft_write_error()); }
  }, [origin, chat]);
  // Transcrição desta tela ou de uma montagem anterior da mesma conversa ainda no ar.
  const busyVoice = transcribing || dictation?.status === 'pending';
  const voiceActive = micStarting || gravando || busyVoice;
  const voiceRecipients = JSON.stringify([sendToPair, [...(pairPeers ?? [])].sort()]);
  // Parar a gravação não aguarda onFim: só a aplicação do mesmo ditado libera o envio.
  deferVoiceSendRef.current = (mode) => {
    if (micStartingRef.current) {
      setError((current) => current || m.composer_voice_starting());
      return true;
    }
    if (!voiceActive && !transcribingRef.current) return false;
    if (voiceSendRef.current && !voiceSendRef.current.cancelled) return true;
    cancelarAuto();
    if (sendingRef.current || isSubmitting(origin.serverId, origin.name) || uploading || blockedRef.current || submissionBlocksSend || editing
      || !mountedRef.current || !activeRef.current || !navigation.isFocused()) {
      if (mountedRef.current) setError((current) => current || m.composer_voice_send_cancelled());
      return true;
    }
    try {
      const snapshot = readDraft(origin.serverId, origin.name);
      const voice = readDictation(origin.serverId, origin.name);
      const target = recordingTargetRef.current;
      const pendingVoice = voice?.status === 'pending' ? voice : null;
      const revision = pendingVoice?.draftRevision ?? target?.snapshot.revision;
      const before = pendingVoice?.before ?? target?.snapshot.text.trim();
      const voiceTranscript = pendingVoice ? pendingVoice.transcript : target?.snapshot.transcript;
      if (!snapshot || snapshot.revision !== revision || snapshot.text.trim() !== before
        || (voiceTranscript !== null && voiceTranscript !== transcriptRef.current)) {
        setError(m.native_dictation_draft_changed());
        return true;
      }
      const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
      if (!server) throw new Error(m.chat_servidor_removido());
      const intent: VoiceSendIntent = { mode, draft: snapshot, server: { ...server }, recipients: voiceRecipients,
        generation: activityGenerationRef.current, dictationId: pendingVoice?.id ?? null, cancelled: false, stopping: gravando };
      voiceSendRef.current = intent;
      setWaitingVoiceSend(true);
      voiceSendTimerRef.current = setTimeout(() => {
        if (voiceSendRef.current === intent) cancelVoiceSend();
      }, VOICE_SEND_TIMEOUT_MS);
      if (gravando) void parar('botao').then(() => {
        intent.stopping = false;
        // onFim marca a transcrição antes de copiar o áudio; a parada sozinha não garante resultado.
        if (voiceSendRef.current === intent && !intent.cancelled && !transcribingRef.current && !intent.dictationId) cancelVoiceSend();
      }).catch((e: unknown) => {
        cancelVoiceSend(false);
        if (mountedRef.current) setError(e instanceof Error ? e.message : m.composer_falha_gravacao());
      });
    } catch (e) {
      cancelVoiceSend();
      setError(e instanceof Error ? e.message : m.composer_falha_envio());
    }
    return true;
  };
  const canSend = (hasContent || voiceActive) && !micStarting && !waitingVoiceSend && !sending && !uploading && !readBlocked && !submissionBlocksSend;
  const showTextSteer = state === 'working' && (isCodex || (isClaude && headless))
    && (!!text.trim() || voiceActive) && !text.trimStart().startsWith('/') && !editing && !(sendToPair && pairPeers?.length);
  const canSteerText = showTextSteer && canSend && !pendingAttachments.length;
  const handleSteerText = useCallback(async (voiceReady = false) => {
    if ((!voiceReady && !canSteerText) || sendingRef.current || isSubmitting(origin.serverId, origin.name)
      || micStartingRef.current || attachmentsRef.current.length || chat.use.getState().queueEdit
      || chat.use.getState().stateEvent?.state !== 'working' || !(isCodex || (isClaude && headless))
      || (sendToPair && pairPeers?.length) || !activeRef.current || !navigation.isFocused()) {
      if (voiceReady) setError((current) => current || m.composer_voice_send_cancelled());
      else if (micStartingRef.current) setError((current) => current || m.composer_voice_starting());
      return;
    }
    if (!voiceReady && deferVoiceSendRef.current('steer')) return;
    if (transcribingRef.current || voiceActive) {
      if (voiceReady) setError((current) => current || m.composer_voice_send_cancelled());
      return;
    }
    const value = textRef.current;
    if (!value.trim() || value.trimStart().startsWith('/') || blockedRef.current || !persistText(value)) {
      if (voiceReady) setError((current) => current || m.composer_voice_send_cancelled());
      return;
    }
    const revision = draftRef.current!.revision;
    sendingRef.current = true;
    setSending(true);
    setError('');
    limparUndo();
    cancelarAuto();
    try {
      await chat.send(value, revision, 'steer');
      setBar(null);
    } catch (e) {
      setError(e instanceof Error ? e.message : m.composer_falha_envio());
    } finally {
      sendingRef.current = false;
      setSending(false);
    }
  }, [canSteerText, origin, chat, persistText, limparUndo, cancelarAuto, setBar, isCodex, isClaude, headless, sendToPair, pairPeers, voiceActive, navigation]);

  useEffect(() => {
    const intent = voiceSendRef.current;
    if (!intent || intent.cancelled) return;
    try {
      const latest = readDraft(origin.serverId, origin.name);
      const voice = readDictation(origin.serverId, origin.name);
      const applied = !!intent.dictationId && latest?.dictationId === intent.dictationId;
      const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
      if (!mountedRef.current || !activeRef.current || !navigation.isFocused() || editing
        || intent.generation !== activityGenerationRef.current || intent.recipients !== voiceRecipients
        || !server || server.baseUrl !== intent.server.baseUrl || server.token !== intent.server.token
        || !latest || latest.transcript !== intent.draft.transcript || transcript !== intent.draft.transcript
        || latest.revision !== intent.draft.revision + (applied ? 1 : 0)
        || !sameAttachments(latest.attachments, intent.draft.attachments)
        || (!applied && latest.text !== intent.draft.text)
        || (voice?.id === intent.dictationId && (voice.status === 'failed' || !!voice.issue))) {
        cancelVoiceSend();
        return;
      }
      if (!applied) {
        if (!voiceActive && !transcribingRef.current && !intent.stopping) cancelVoiceSend();
        return;
      }
      if (voiceActive || transcribingRef.current || text !== latest.text) return;
      // Consumir antes de chamar o envio: um segundo aviso do mesmo ditado não pode repetir.
      cancelVoiceSend(false);
      if (intent.mode === 'steer') void handleSteerText(true);
      else void handleSend(true);
    } catch (e) {
      cancelVoiceSend();
      setDraftIssue(e instanceof Error ? e.message : m.draft_read_error());
    }
  }, [draftUpdate, text, voiceActive, waitingVoiceSend, transcript, voiceRecipients, editing, originServer,
    origin, navigation, handleSend, handleSteerText, cancelVoiceSend]);
  // Sem terminal, pergunta pendente só tem saída pelo Parar (não há pane para mandar Esc).
  const canInterrupt = state === 'working' || (headless && state === 'awaiting_input');
  // Parar pede o campo vazio; com texto, Orientar (quando disponível) fica ao lado de Enviar.
  const showStop = !!onStop && canInterrupt && !hasContent && !voiceActive && !editing;
  const showSend = editing || !showStop || hasContent || sending || uploading || filaCount > 0;
  // Em edição o botão é o Atualizar: só o texto conta, e uma ação da fila por vez.
  const canSubmit = queueEdit ? canUpdateQueueEdit(text, queueEdit.refs) && !queueBusy : canSend;
  const queueFeedback = queueNotice ?? queueError;
  // A edição da fila troca só o texto: anexo e ditado esperam a volta do rascunho.
  const attachBlocked = uploading || sending || gravando || editing;
  const micBlocked = micStarting || transcribing || sending || (!!dictation && !gravando) || editing;

  const confirmStop = useCallback(() => {
    Alert.alert(m.composer_interromper_claude(), m.composer_interromper_msg(), [
      { text: m.comum_cancelar(), style: 'cancel' },
      { text: m.composer_interromper(), style: 'destructive', onPress: () => {
        void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium);
        onStop?.();
      } },
    ]);
  }, [onStop]);

  // ── Comandos: lista da folha e sugestões em linha enquanto a palavra sob o cursor é `/nome` ──
  // Cursor movido pelo app (`selection`) vale até o campo devolver o dele.
  const sel = selection ?? caret;
  const caretPos = !sel ? text.length : sel.start === sel.end ? Math.min(sel.end, text.length) : -1;
  const slashToken = useMemo(() => slashTokenAt(text, caretPos), [text, caretPos]);
  const slashQuery = slashToken?.query ?? null;
  const { commands, error: commandsError, retry: retryCommands } = useSessionCommands(serverId, name, provider, commandSheetOpen || slashQuery !== null);
  const suggestions = useMemo(() => slashMatches(commands ?? [], slashQuery), [commands, slashQuery]);

  const replaceText = useCallback((next: string, cursor = next.length) => {
    textRef.current = next;
    persistText(next);
    setText(next);
    setSelection({ start: cursor, end: cursor });
  }, [persistText]);

  // Toda saída da edição devolve o campo aqui (o store não alcança o campo): o texto que a saída deixou em
  // `queueEditExit` ou, sem ele, o rascunho de antes dela.
  const lastEditRef = useRef<QueueEdit | null>(null);
  useEffect(() => {
    const prev = lastEditRef.current;
    lastEditRef.current = queueEdit;
    if (!prev || queueEdit) return;
    const exit = chat.use.getState().queueEditExit;
    if (exit !== null) chat.use.setState({ queueEditExit: null });
    const next = exit ?? prev.savedDraft;
    replaceText(next, next.length);
  }, [queueEdit, replaceText, chat]);

  // A mensagem saiu da fila durante a edição (entregue, orientada ou apagada noutro aparelho).
  useEffect(() => {
    if (!queueEdit) return;
    const left = queueEditLeft(queueEdit, queueList, textRef.current);
    if (!left) return;
    chat.dropQueueEdit(left.composerText);
    setQueueNotice(m.queue_entry_gone());
  }, [queueList, queueEdit, chat]);

  // Sair da conversa no meio da edição: o rascunho guardado é o de antes dela e volta na próxima abertura.
  useEffect(() => () => {
    if (chat.use.getState().queueEdit) chat.dropQueueEdit();
  }, [chat]);

  useEffect(() => {
    if (!queueNotice) return;
    const timer = setTimeout(() => setQueueNotice(null), 5000);
    return () => clearTimeout(timer);
  }, [queueNotice]);

  const handleQueueEdit = useCallback((id: string) => {
    const next = chat.startQueueEdit(id, textRef.current);
    replaceText(next, next.length);
  }, [chat, replaceText]);

  // Preenche `/nome ` para a pessoa digitar o argumento. Texto já escrito (que não é o próprio
  // comando sendo digitado) fica como argumento, em vez de sumir.
  const fillCommand = useCallback((cmdName: string) => {
    const kept = soComando(textRef.current) ? '' : textRef.current.trim();
    replaceText(kept ? `/${cmdName} ${kept}` : `/${cmdName} `);
  }, [replaceText]);

  // Comando sem argumento sai na hora, como o PWA: sem eco na conversa nem rascunho.
  const runCommand = useCallback(async (cmd: string) => {
    if (soComando(textRef.current)) replaceText('');
    const btw = isClaude ? sideQuestionOf(cmd) : null;
    if (btw !== null) { setSideQuestion(btw); return; }
    const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
    if (!server) { setError(m.chat_servidor_removido()); return; }
    setError('');
    try {
      await sendInputForServer(server, origin.name, cmd);
    } catch (e) {
      setError(e instanceof Error ? e.message : m.composer_falha_envio());
    }
  }, [replaceText, isClaude, origin]);

  const openSelector = useCallback((which: 'model' | 'effort' | 'mode' | 'account') => {
    setSelectorRequest((cur) => ({ which, n: (cur?.n ?? 0) + 1 }));
  }, []);
  useEffect(() => {
    if (!settingsRequest || lastSettingsRequest.current === settingsRequest.n) return;
    lastSettingsRequest.current = settingsRequest.n;
    openSelector(settingsRequest.which);
    // A tela larga o pedido para ele não voltar ao remontar o Composer.
    onSettingsRequestConsumed?.(settingsRequest.n);
  }, [settingsRequest, onSettingsRequestConsumed, openSelector]);

  // Sugestão tocada: model/effort abrem o seletor; com argumento, destrutivo ou no Codex preenche
  // para revisar; o resto envia direto.
  const handleSuggestPick = useCallback((c: CommandInfo) => {
    if ((!isCodex || c.source === 'builtin') && (c.name === 'model' || c.name === 'effort')) {
      replaceText('');
      openSelector(c.name);
      return;
    }
    if (isCodex || c.argumentHint || c.destructive) { fillCommand(c.name); return; }
    void runCommand('/' + c.name);
  }, [isCodex, replaceText, openSelector, fillCommand, runCommand]);

  // No meio do texto a sugestão só completa o nome: o resto da mensagem fica, e nada é enviado.
  const pickSlash = useCallback((c: CommandInfo) => {
    if (!slashToken || slashToken.whole) { handleSuggestPick(c); return; }
    const next = replaceSlashToken(textRef.current, slashToken, c.name);
    replaceText(next.text, next.cursor);
  }, [slashToken, handleSuggestPick, replaceText]);

  const handleKeyPress = useCallback(
    (ev: NativeSyntheticEvent<TextInputKeyPressEventData>) => {
      // Enter envia só no web (no celular é quebra de linha). O shiftKey vem do evento do DOM que o
      // react-native-web repassa em nativeEvent — o tipo do RN não o declara.
      const nat: TextInputKeyPressEventData = ev.nativeEvent;
      const shift = 'shiftKey' in nat && Boolean((nat as { shiftKey?: boolean }).shiftKey);
      if (nat.key === 'Enter' && !shift && Platform.OS === 'web') {
        // Sem isto o Enter TAMBÉM insere a quebra de linha, que corre contra o setText('') do envio.
        ev.preventDefault();
        void handleSend();
      }
    },
    [handleSend],
  );

  // A seleção vira cópia do app antes de entrar no rascunho: a original pode sumir do cache do picker.
  const adoptPicked = useCallback(async (picked: PendingAttach) => {
    let kept: DraftAttachment;
    try {
      kept = await retainDraftAttachment(withoutUpload(picked));
    } catch (e) {
      setError(e instanceof Error && e.message ? e.message : m.draft_write_error());
      return;
    }
    const next = [...attachmentsRef.current, { ...kept, size: picked.size }];
    if (!persistDraft({ attachments: next })) {
      dropCopy(kept.uri);
      return;
    }
    setPendingAttachments(next);
  }, [persistDraft, setPendingAttachments]);

  const handlePicked = useCallback((picked: PickedAttachment) => {
    setError('');
    void adoptPicked(picked);
  }, [adoptPicked]);

  // Colar imagens ou arquivos no campo os acrescenta aos anexos, como o paste do PWA.
  const handlePaste = useCallback((files: PastedFile[], erro: string | null) => {
    if (erro) { setError(formataErro(erro) ?? erro); return; }
    for (const f of files) {
      const mime = f.type || 'application/octet-stream';
      handlePicked({ uri: f.uri, name: f.fileName, mime, kind: mime.startsWith('image/') ? 'image' : 'file', size: f.fileSize });
    }
  }, [handlePicked]);

  // O menu de segurar do campo no iOS não oferece Colar: o "+" cola pela área de transferência.
  const handleClipboardPaste = useCallback(async () => {
    try {
      const pasted = await readClipboard();
      if (pasted && typeof pasted !== 'string') { handlePicked(pasted); return; }
      if (!pasted) { setError(m.composer_colar_vazio()); return; }
      const current = textRef.current;
      const start = Math.min(caret?.start ?? current.length, current.length);
      const at = { start, end: Math.min(Math.max(caret?.end ?? current.length, start), current.length) };
      const next = current.slice(0, at.start) + pasted + current.slice(at.end);
      handleChangeText(next);
      const end = at.start + pasted.length;
      setSelection({ start: end, end });
    } catch (e) {
      setError(e instanceof Error && e.message ? e.message : m.composer_colar_falhou());
    }
  }, [handlePicked, handleChangeText, caret]);

  const router = useRouter();

  const handleRemoveAttach = useCallback((uri: string) => {
    const next = attachmentsRef.current.filter((a) => a.uri !== uri);
    if (next.length === attachmentsRef.current.length || !persistDraft({ attachments: next })) return;
    uploadsRef.current.delete(uri);
    dropCopy(uri);
    setPendingAttachments(next);
  }, [persistDraft, setPendingAttachments]);

  useEffect(() => {
    return () => {
      if (undoTimerRef.current) clearTimeout(undoTimerRef.current);
      if (autoTimerRef.current) clearInterval(autoTimerRef.current);
    };
  }, []);

  return (
    <>
    <ComposerBox focused={focado}>
        {/* Faixa de cima da caixa: o que espera a vez no Hangar, as ações por mensagem e o modo de envio. */}
        <QueuePanel panel={queueList} editingId={queueEdit?.entryId ?? null} busy={queueBusy}
          onEdit={handleQueueEdit} onCancelEdit={chat.cancelQueueEdit}
          delivery={supportsDelivery ? delivery : undefined} onModeChange={useDeliveryMode.getState().setMode}
          onSteer={chat.steerQueued} onRemove={chat.removeQueued} onMove={chat.moveQueued}
          reopen={deliveryStalled ? { serverId, name } : undefined} />
        {/* Fila da TUI do Kimi esperando o turno: diz quantas e dá a saída de mandar agora. */}
        {showSteer ? (
          <Pressable
            onPress={handleSteer}
            disabled={steering}
            hitSlop={7}
            accessibilityState={{ disabled: steering, busy: steering }}
            style={({ pressed }) => [styles.steerBtn, { backgroundColor: pressed ? theme.tokens.bg.hover : theme.tokens.fillSubtle }]}
            accessibilityLabel={m.composer_fila_aria()}
            accessibilityHint={m.composer_fila_titulo()}
            accessibilityRole="button"
          >
            <Icon name="Hourglass" size={13} color={theme.tokens.accent.base} />
            {!steering ? (
              <Text style={[styles.steerCount, { color: theme.tokens.text.secondary }]}>
                {`${m.composer_fila_contagem({ n: filaCount })} ·`}
              </Text>
            ) : null}
            <Text style={[styles.steerText, { color: theme.tokens.accent.base }]}>
              {steering ? m.askq_enviando() : m.composer_fila_acao()}
            </Text>
          </Pressable>
        ) : null}

        {queueFeedback ? <Text style={[styles.hint, { color: theme.tokens.text.secondary }]} accessibilityLiveRegion="polite">{queueFeedback}</Text> : null}
        {deliveryFeedback ? <Text style={[styles.hint, { color: deliveryFeedback.warning ? theme.tokens.status.warning : theme.tokens.text.secondary }]}
          accessibilityRole={deliveryFeedback.warning ? 'alert' : undefined} accessibilityLiveRegion="polite">{deliveryFeedback.text}</Text> : null}

        {/* Como o T3: a linha diz que o campo tem a mensagem da fila, e Cancelar devolve o rascunho. */}
        {editing ? (
          <View style={styles.undoRow}>
            <Icon name="Pencil" size={13} color={theme.tokens.accent.base} />
            <Text style={[styles.hint, styles.recoverText, { color: theme.tokens.text.secondary }]} accessibilityLiveRegion="polite">
              {m.queue_panel_editing()}
            </Text>
            <Pressable onPress={chat.cancelQueueEdit} disabled={queueBusy} hitSlop={7} style={styles.editCancel} accessibilityRole="button"
              accessibilityState={{ disabled: queueBusy }}>
              <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.comum_cancelar()}</Text>
            </Pressable>
          </View>
        ) : null}

        {/* Mandar pro grupo liga e desliga no +; aqui só fica à vista enquanto está ligado, porque
            muda para quem a mensagem vai. */}
        {pairPeers?.length && sendToPair ? (
          <Pressable
            onPress={() => setSendToPair((current) => !current)}
            hitSlop={7}
            style={[styles.pairChip, { backgroundColor: sendToPair ? theme.tokens.accent.dim : theme.tokens.fillSubtle }]}
            accessibilityRole="switch"
            accessibilityState={{ checked: sendToPair }}
            accessibilityLabel={m.composer_mandar_grupo()}
            accessibilityHint={sendToPair ? m.composer_mandando_grupo() : m.composer_mandar_tambem({ n: pairPeers.join(', ') })}
          >
            <Icon name="ArrowLeftRight" size={14} color={sendToPair ? theme.tokens.accent.base : theme.tokens.text.secondary} />
            <Text style={[styles.pairChipLabel, { color: sendToPair ? theme.tokens.accent.base : theme.tokens.text.secondary }]} numberOfLines={1}>
              {sendToPair ? (pairPeers.length === 1 ? m.composer_pros_dois() : m.composer_pro_grupo()) : m.composer_mandar_tambem({ n: pairPeers.join(', ') })}
            </Text>
          </Pressable>
        ) : null}

        {refs.length && !editing ? (
          <ContextRefTray refs={refs} draftKey={refsKey} disabled={sending} />
        ) : null}
        {/* "Finalizar" leva ao campo depois que o modal sai da tela: focado antes, o iOS ignora. No
            Android o teclado só abre quando a janela do modal já devolveu o foco; antes disso o campo
            fica focado sem teclado, e o segundo pedido o abre. */}
        <CiteReplyModal draftKey={refsKey} onFinish={() => {
          setTimeout(() => inputRef.current?.focus(), 350);
          setTimeout(() => { if (!Keyboard.isVisible()) inputRef.current?.focus(); }, 900);
        }} />

        {/* A edição troca só o texto: os anexos do rascunho esperam a volta dele. */}
        {pendingAttachments.length === 1 && !editing ? (
          <AttachmentPreview attachment={pendingAttachments[0]} onRemove={() => handleRemoveAttach(pendingAttachments[0].uri)} disabled={sending} />
        ) : pendingAttachments.length > 1 && !editing ? (
          <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.attachStrip}>
            {pendingAttachments.map((a) => (
              <AttachmentPreview key={a.uri} attachment={a} onRemove={() => handleRemoveAttach(a.uri)} disabled={sending} style={styles.attachTile} />
            ))}
          </ScrollView>
        ) : null}


        <SlashSuggest matches={suggestions} onPick={pickSlash}
          error={slashQuery !== null ? commandsError : ''} onRetry={retryCommands} />

        {/* Campo em linha própria: dividindo a linha com os botões ele ficava só com a sobra. */}
          <ComposerInput
            ref={inputRef}
            accessible
            accessibilityLabel={m.composer_mensagem()}
            value={text}
            onChangeText={handleChangeText}
            placeholder={busyVoice ? m.composer_transcrevendo_audio()
              : provider ? m.composer_mensagem_para({ nome: providerName(provider as Provider) }) : m.composer_mensagem()}
            onKeyPress={handleKeyPress}
            selection={selection}
            onSelectionChange={(e) => {
              setSelection(undefined);
              setCaret(e.nativeEvent.selection);
            }}
            onFocus={() => setFocado(true)}
            onBlur={() => setFocado(false)}
            onPaste={handlePaste}
            recording={gravando}
            rms={rms}
            onCancelRecording={handleCancelRecording}
          />

        {/* Linha do app de PC: + e microfone sem moldura à esquerda; o chip da sessão e o Enviar
            fecham a linha. Com o Orientar, o chip encolhe o nome do modelo mas nada sai, como no PWA. */}
        <ComposerActions
          attach={{ onPress: () => setAttachMenuOpen(true), disabled: attachBlocked, busy: uploading }}
          mic={{ onPress: () => void handleMicPress(), disabled: micBlocked, busy: busyVoice, recording: gravando,
            styleLabel: dictationStyle, onStyle: () => setStyleMenuOpen(true) }}
          settings={<SessionSettingsButton serverId={serverId} name={name} provider={provider} headless={headless} openRequest={selectorRequest} />}
          afterSettings={<>

          {/* Parar: só o quadrado vermelho, sem círculo em volta, como no PC. */}
          {showStop ? (
            <Pressable
              onPress={confirmStop}
              disabled={stopping}
              hitSlop={5}
              style={({ pressed }) => [styles.roundBtn, pressed && styles.iconBtnPressed, stopping && styles.iconBtnDisabled]}
              accessibilityLabel={m.composer_parar()}
              accessibilityRole="button"
              accessibilityState={{ disabled: stopping, busy: stopping }}
            >
              {stopping ? (
                <ActivityIndicator size="small" color={theme.tokens.status.error} />
              ) : (
                <View style={[styles.stopMark, { backgroundColor: theme.tokens.status.error }]} />
              )}
            </Pressable>
          ) : null}

          {showTextSteer ? (
            <Pressable
              onPress={() => void handleSteerText()}
              disabled={!canSteerText}
              hitSlop={7}
              accessibilityRole="button"
              accessibilityLabel={m.composer_steer_current_text()}
              accessibilityHint={m.codex_orientar_ajuda()}
              accessibilityState={{ disabled: !canSteerText, busy: waitingVoiceSend || sending }}
              style={({ pressed }) => [styles.steerBtn, { backgroundColor: pressed ? theme.tokens.bg.hover : theme.tokens.fillSubtle }, !canSteerText && styles.iconBtnDisabled]}
            >
              <Text style={[styles.steerText, { color: theme.tokens.accent.base }]}>{m.codex_orientar()}</Text>
            </Pressable>
          ) : null}

          </>}
          send={{ visible: showSend, onPress: () => void handleSend(), disabled: !canSubmit,
            busy: editing ? queueBusy : waitingVoiceSend || sending || uploading, editing, count: filaCount,
            label: editing ? m.queue_panel_update_aria() : m.composer_enviar_mensagem(),
            hint: filaCount > 0 ? m.composer_fila_contagem({ n: filaCount }) : undefined }}
          modeMenu={modeMenu}
        />

        {bar && barVersions ? (
          <View style={styles.versionRow}>
            {barVersions ? (
              <>
                <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.composer_ditado_versao()}</Text>
                {versionOptions().map((o) => {
                  const selected = bar.applied === o.v;
                  return (
                    <Pressable key={o.v} onPress={() => void handleVersion(o.v)} disabled={revising !== null || busyVoice || gravando}
                      hitSlop={7} accessibilityRole="button" accessibilityLabel={o.label}
                      accessibilityState={{ selected, disabled: revising !== null || busyVoice || gravando, busy: revising === o.v }}
                      style={[styles.versionBtn, selected && { backgroundColor: theme.tokens.fillSubtle }]}>
                      <Text style={[styles.undoText, { color: selected ? theme.tokens.text.primary : theme.tokens.text.secondary }]}>
                        {revising === o.v ? m.composer_ditado_trocando() : o.label}
                      </Text>
                    </Pressable>
                  );
                })}
              </>
            ) : null}
          </View>
        ) : null}

        {recordings.length > 0 && !editing ? (
          <View style={styles.recordings}>
            {recordings.filter((recording) => !dictationPlayer || recording.serverPath !== dictation?.serverPath).map((recording) => {
              const player = dictationAudio(originServer, origin.name, null, recording.serverPath);
              return player ? <AudioChip key={recording.id} {...player} disabled={sending || gravando || busyVoice}
                onRemove={() => handleRemoveRecording(recording.id)} /> : null;
            })}
          </View>
        ) : null}

        {/* Recuperar ditado, envio ou rascunho mexe no rascunho guardado: espera a edição da fila acabar. */}
        {dictation && !busyVoice && !editing ? (
          <View style={styles.dictationRecovery}>
            <Text style={[styles.dictationMessage, { color: theme.tokens.text.secondary }]}
              accessibilityRole={dictation.status === 'failed' ? 'alert' : undefined}
              accessibilityLiveRegion={dictation.status === 'failed' ? 'assertive' : 'polite'}>
              {dictationMessage}
            </Text>
            <View style={styles.errorRow}>
            {dictationPlayer ? <AudioChip key={dictationPlayer.uri} {...dictationPlayer} onRemove={handleDiscardDictation} /> : null}
            {dictation.status === 'applied' ? null : dictation.text ? (
              <Pressable onPress={handleRecoverDictation} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
                <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.composer_draft_recover()}</Text>
              </Pressable>
            ) : (
              <Pressable onPress={handleRetry} style={[styles.retryBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
                <Text style={[styles.retryText, { color: theme.tokens.accent.base }]}>{m.composer_transcrever_de_novo()}</Text>
              </Pressable>
            )}
            <Pressable onPress={handleDiscardDictation} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
              <Text style={[styles.undoText, { color: theme.tokens.text.secondary }]}>{m.composer_draft_discard()}</Text>
            </Pressable>
            </View>
          </View>
        ) : null}

        {autoN !== null ? (
          <Pressable onPress={cancelarAuto} hitSlop={7} style={[styles.autoChip, { backgroundColor: theme.tokens.fillSubtle }]} accessibilityRole="button">
            <Text style={[styles.autoText, { color: theme.tokens.text.primary }]}>{m.composer_enviando_cancelar({ n: autoN })}</Text>
          </Pressable>
        ) : null}

        {/* Com as versões à vista, o "Cru" já desfaz a limpeza. */}
        {undo && !barVersions ? (
          <View style={styles.undoRow}>
            <Text style={[styles.hint, { color: theme.tokens.text.muted }]}>{m.composer_ditado_limpo()}</Text>
            <Pressable onPress={handleUndo} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
              <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.composer_desfazer_limpeza()}</Text>
            </Pressable>
          </View>
        ) : null}

        {error && (!dictation || busyVoice || editing || error !== dictationMessage) ? (
          <View style={styles.errorRow}>
            <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert" accessibilityLiveRegion="assertive">{error}</Text>
            {failed ? (
              <Pressable onPress={handleRetry} style={[styles.retryBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
                <Text style={[styles.retryText, { color: theme.tokens.accent.base }]}>{m.composer_transcrever_de_novo()}</Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}

        {draftIssue ? (
          <View style={styles.errorRow}>
            <Text style={[styles.error, { color: theme.tokens.status.error }]} accessibilityRole="alert" accessibilityLiveRegion="assertive">{draftIssue}</Text>
            {readBlocked ? (
              <Pressable onPress={handleRereadDraft} style={[styles.retryBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
                <Text style={[styles.retryText, { color: theme.tokens.accent.base }]}>{m.composer_draft_read_again()}</Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}

        {/* Em voo a mensagem já aparece no painel "Na fila": aqui só o resultado que pede ação. */}
        {submission && submission.status !== 'sending' && !sending && !isSubmitting(origin.serverId, origin.name) && !editing ? (
          <View style={styles.undoRow}>
            <Text style={[styles.hint, styles.recoverText, { color: theme.tokens.text.muted }]} accessibilityLiveRegion="polite">
              {submission.status === 'rejected' ? m.composer_submission_rejected() : m.chat_envio_incerto()}
            </Text>
            <Pressable onPress={handleRecoverSubmission} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
              <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.composer_draft_recover()}</Text>
            </Pressable>
            {submission.status === 'unknown' ? (
              <Pressable onPress={chat.retry} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
                <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.composer_submission_check()}</Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}

        {/* Recuperável sem texto nem anexo não tem o que devolver. */}
        {recoverable && (recoverable.text.trim() || recoverable.attachments.length) && !editing ? (
          <View style={styles.undoRow}>
            <Text style={[styles.hint, styles.recoverText, { color: theme.tokens.text.muted }]} numberOfLines={2}>
              {m.composer_draft_previous({ text: recoverable.text.trim().slice(0, 80) })}
            </Text>
            <Pressable onPress={handleRecoverDraft} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
              <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.composer_draft_recover()}</Text>
            </Pressable>
            <Pressable onPress={handleDiscardDraft} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
              <Text style={[styles.undoText, { color: theme.tokens.text.secondary }]}>{m.composer_draft_discard()}</Text>
            </Pressable>
          </View>
        ) : null}

        <CommandSheet
          open={commandSheetOpen}
          onClose={() => setCommandSheetOpen(false)}
          commands={commands}
          error={commandsError}
          onRetry={retryCommands}
          fillOnly={isCodex}
          onCommand={(cmd) => void runCommand(cmd)}
          onFill={fillCommand}
          onOpenModelEffort={openSelector}
        />

        {/* Monta só aberta: o histórico da pergunta lateral é lido a cada abertura. */}
        {sideQuestion !== null ? (
          <SideQuestionSheet open serverId={serverId} name={name} question={sideQuestion} onClose={() => setSideQuestion(null)} />
        ) : null}

        <DictationStyleMenu open={styleMenuOpen} onClose={() => setStyleMenuOpen(false)} server={styleServer} />

        <AttachSheet
          open={attachMenuOpen}
          onClose={() => setAttachMenuOpen(false)}
          onPick={handlePicked}
          multiple
          onError={setError}
          onPaste={() => { void handleClipboardPaste(); }}
          onSessionAttachments={() => router.push(`/s/${origin.serverId}/${origin.name}/attachments` as never)}
          onCommands={() => setCommandSheetOpen(true)}
          dictationStyle={{ label: dictationStyle, onPress: () => setStyleMenuOpen(true) }}
          sendToGroup={pairPeers?.length
            ? { label: m.composer_mandar_tambem({ n: pairPeers.join(', ') }), on: sendToPair, onPress: () => setSendToPair((c) => !c) }
            : undefined}
        />
    </ComposerBox>
    </>
  );
}

const styles = StyleSheet.create((theme) => ({
  // Chips acima da caixa (fila, par): a mesma silhueta do chip da sessão; hitSlop leva a 44 pt.
  steerBtn: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
    height: 30,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
  },
  steerText: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.semibold,
  },
  steerCount: {
    fontSize: theme.base.text.xs,
  },
  pairChip: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
    height: 30,
    maxWidth: 240,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
  },
  pairChipLabel: {
    flexShrink: 1,
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.semibold,
  },
  iconBtnPressed: {
    backgroundColor: theme.tokens.bg.hover,
  },
  iconBtnDisabled: {
    opacity: 0.5,
  },
  roundBtn: {
    width: 34,
    height: 34,
    borderRadius: theme.base.radius.full,
    alignItems: 'center',
    justifyContent: 'center',
  },
  stopMark: {
    width: 13,
    height: 13,
    borderRadius: theme.base.radius.xs,
  },
  autoChip: {
    minWidth: 44,
    height: 30,
    justifyContent: 'center',
    alignSelf: 'flex-start',
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
  },
  autoText: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.semibold,
  },
  undoRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
  },
  recoverText: {
    flex: 1,
  },
  undoBtn: {
    minWidth: 44,
    minHeight: 44,
    justifyContent: 'center',
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
  },
  undoText: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.semibold,
  },
  editCancel: {
    minHeight: 30,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[1],
  },
  error: {
    flexShrink: 1,
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[1],
  },
  recordings: { gap: theme.base.space[2], alignItems: 'flex-start' },
  dictationRecovery: { gap: theme.base.space[2] },
  dictationMessage: { fontSize: theme.base.text.sm, alignSelf: 'stretch' },
  errorRow: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[2],
    flexWrap: 'wrap',
  },
  retryBtn: {
    minWidth: 44,
    minHeight: 44,
    justifyContent: 'center',
    borderWidth: 1,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
  },
  retryText: {
    fontSize: theme.base.text.xs,
    fontWeight: theme.base.weight.semibold,
  },
  attachStrip: {
    gap: theme.base.space[2],
  },
  attachTile: {
    width: 220,
  },
  versionRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: theme.base.space[1],
  },
  versionBtn: {
    minHeight: 30,
    borderRadius: theme.base.radius.full,
    paddingHorizontal: theme.base.space[2],
    justifyContent: 'center',
  },
  hint: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[1],
  },
}));
