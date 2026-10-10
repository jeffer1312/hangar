import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import * as Haptics from 'expo-haptics';
import { AccessibilityInfo, ActivityIndicator, Alert, AppState, Platform, Pressable, Text, View, type NativeSyntheticEvent, type TextInput, type TextInputKeyPressEventData } from 'react-native';
import type { NativeStackNavigationProp } from 'expo-router';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { broadcast, formataErro, hexParaRgb, uploadFileForServer, steerSession, podeEnviarSozinho, providerName, sendInputForServer, sideQuestionOf, slashMatches, slashTokenAt, replaceSlashToken, relimparDitado, estilosDitado } from '@hangar/core';
import type { CommandInfo, MotivoFim, Provider, Server } from '@hangar/core';
import { Glass } from '../ui/Glass';
import { Icon } from '../ui/Icon';
import { MultilineInput } from '../ui/MultilineInput';
import { RecordingWave } from '../ui/RecordingWave';
import * as m from '../paraglide/messages';
import { chatStore, filaCount as filaCountOf, submitConversationDraft, isSubmitting } from '../stores/chat';
import { abandonUnknownAttempt, attachInsert, confirmFirstInput, firstInputMessage, readFirstInput, sendFirstInput, useNewConversation, withAttach } from '../stores/newConversation';
import { useSessions } from '../stores/sessions';
import { clearDraft, clearRecoverableDraft, readDraft, readRecoverableDraft, resolveDraftTranscript, reusableUploadPath, withoutUpload, writeDraft, writeRecoverableDraft, clearDictation, readDictation, writeDictation, recoverDictation, applyReadyDictation, associateDictationTranscript, type ConversationDraft, type DraftAttachment, type DictationDraft } from '../stores/drafts';
import { useServers } from '../stores/servers';
import { removeDraftAttachment, retainDraftAttachment } from './draftAttachments';
import { useNavigation, useRouter } from 'expo-router';
import { DictationStyleMenu, useDictationStyleLabel } from '../features/ditado/EstiloPill';
import { useDitado } from '../features/ditado/useDitado';
import { useDitadoEstiloStore } from '../features/ditado/ditadoEstiloStore';
import { DictationError, dictationAudio, runDictation, uploadName, type DictationSource } from '../features/ditado/dictationRun';
import { AudioChip } from './AudioChip';
import { CommandSheet, useSessionCommands } from './CommandSheet';
import { SessionSettingsButton } from './SessionSettings';
import { SideQuestionSheet } from './SideQuestionSheet';
import { SlashSuggest } from './SlashSuggest';
import type { PickedAttachment } from '../ui/attachmentPicker';
import { AttachSheet } from '../ui/AttachSheet';
import { AttachmentPreview } from '../ui/AttachmentPreview';
import type { PastedFile } from '@mattermost/react-native-paste-input';

interface Props {
  serverId: string;
  name: string;
  draft?: string;
  // Texto devolvido pelo Parar/cancelar: entra antes do que já está no campo, nunca no lugar.
  returned?: { text: string };
  onReturnedAdopted?: () => void;
  firstInputId?: string;
  firstInputSent?: boolean;
  sessionProvider?: string | null;
  // Claude/Codex sem terminal: a fila é do processo (dá para orientar) e pergunta pendente só sai pelo Parar.
  headless?: boolean;
  onStop?: () => void;
  stopping?: boolean;
}

type PendingAttach = PickedAttachment;
// O campo é só um `/nome` sendo digitado (nem argumento nem outro texto): é ele que o comando
// escolhido substitui por inteiro. Espaço no fim não conta: a lista abre com o cursor antes dele.
const soComando = (texto: string) => {
  const t = texto.trimEnd();
  return slashTokenAt(t, t.length)?.whole === true;
};

// Número do selo da fila: crescer com o texto ampliado o cortava dentro do botão; a dica acessível já diz a contagem.
const GLYPH_MAX_SCALE = 1.4;

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
  const value = pending ? { ...old, text: joinDrafts(pending.text, old.text), attachment: old.attachment ?? pending.attachment } : old;
  writeRecoverableDraft(serverId, name, value);
  return value;
}

const VERSION_KEYS = ['cru', 'limpar', 'prosa', 'briefing'];
const versionOptions = () => [{ v: 'cru', label: m.composer_ditado_cru() },
  ...estilosDitado().map((e) => ({ v: e.valor as string, label: e.rotulo }))];

// Barra do último ditado inserido (no fim do campo). `field` é o campo logo depois da inserção e
// `shown` a versão que está nele: como no PC, o texto ditado intacto mantém as versões à vista.
type DictationBar = { field: string; shown: string; raw: string; applied: string; versions: Record<string, string>; serverPath: string | null };
const barFor = (field: string, text: string, raw: string, applied: string, serverPath: string | null): DictationBar =>
  ({ field, shown: text, raw, applied, versions: raw ? { cru: raw, [applied]: text } : { [applied]: text }, serverPath });
const dictatedInField = (bar: DictationBar, value: string) => {
  const start = bar.field.length - bar.shown.length;
  return value.slice(start, start + bar.shown.length) === bar.shown;
};

export function Composer({ serverId, name, draft, returned, onReturnedAdopted, firstInputId, firstInputSent = false, sessionProvider, headless = false, onStop, stopping = false }: Props) {
  const { theme } = useUnistyles();
  const navigation = useNavigation<NativeStackNavigationProp<Record<string, object | undefined>>>();
  const inputRef = useRef<TextInput>(null);
  const restoreFocus = useRef(false);
  const stopRecordingRef = useRef<() => void>(() => {});
  useEffect(() => {
    const blur = navigation.addListener('blur', () => {
      restoreFocus.current = true;
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
  }, [navigation]);
  const chat = chatStore(serverId, name);
  const pending = chat.use((s) => s.pending);
  const events = chat.use((s) => s.events);
  const draftUpdate = chat.use((s) => s.draftUpdate);
  const state = chat.use((s) => s.stateEvent?.state ?? 'idle');
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
  const filaCount = filaCountOf({ events, pending }, provider, headless);

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
  const [pendingAttach, setPendingAttach] = useState<PendingAttach | null>(boot.draft?.attachment ?? null);
  // Rascunho guardado é mais novo que o texto de handoff/cancelamento que a rota ainda carrega.
  const adoptedDraftRef = useRef(draft);
  const [text, setText] = useState(() => boot.draft?.text ?? (firstInputSent ? '' : draft ?? ''));
  const textRef = useRef(text);
  useEffect(() => {
    textRef.current = text;
  }, [text]);

  // Só texto novo avança a revisão: é ela que decide se o ACK pode limpar o campo.
  const persistDraft = useCallback((patch: { text?: string; attachment?: DraftAttachment | null }): boolean => {
    if (blockedRef.current) return false;
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
      ?? { version: 1, text: '', revision: 0, transcript: transcriptRef.current, attachment: null, submission: null };
    const value: ConversationDraft = {
      ...base,
      transcript: base.transcript ?? transcriptRef.current,
      ...(patch.text !== undefined ? { text: patch.text, revision: base.revision + 1 } : {}),
      ...(patch.attachment !== undefined ? { attachment: patch.attachment } : {}),
    };
    try {
      if (!value.text && !value.attachment && !value.submission) clearDraft(origin.serverId, origin.name);
      else writeDraft(origin.serverId, origin.name, value);
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_write_error());
      return false;
    }
    draftRef.current = value;
    setSubmission(value.submission);
    setDraftIssue('');
    return true;
  }, [origin]);
  const persistText = useCallback((next: string) => persistDraft({ text: next }), [persistDraft]);

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
        setPendingAttach(null);
        return;
      }
      if (latest.transcript && transcriptRef.current && latest.transcript !== transcriptRef.current) return;
      const unchanged = textRef.current === (draftRef.current?.text ?? '');
      draftRef.current = latest;
      setSubmission(latest.submission);
      setPendingAttach((cur) => latest.attachment
        ? (cur?.uri === latest.attachment.uri ? { ...latest.attachment, size: cur.size } : latest.attachment)
        : null);
      if (unchanged) {
        textRef.current = latest.text;
        setText(latest.text);
      }
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_read_error());
    }
  }, [draftUpdate, origin]);

  const handleRecoverSubmission = useCallback(() => {
    if (isSubmitting(origin.serverId, origin.name) || !submission
      || (submission.status !== 'rejected' && submission.status !== 'unknown')) return;
    // O anexo continua no rascunho com o caminho enviado: devolver só o texto evita citar o arquivo duas vezes.
    const insert = pendingAttach?.uploadedPath ? attachInsert(pendingAttach, pendingAttach.uploadedPath) : null;
    const sent = insert && submission.text.endsWith(insert)
      ? submission.text.slice(0, -insert.length).replace(/ — $/, '') : submission.text;
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
  }, [submission, origin, pendingAttach]);

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
        setSubmission(null);
        setRecoverable(kept);
        setPendingAttach(null);
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
  }, [transcript, origin]);

  const handleRecoverDraft = useCallback(() => {
    if (!recoverable) return;
    if (pendingAttach && recoverable.attachment) {
      setError(m.composer_draft_recover_attach_busy());
      return;
    }
    const next = joinDrafts(textRef.current, recoverable.text);
    // O upload antigo foi para a sessão que morreu: a recriada recebe o arquivo, nunca o caminho.
    const adopted = !pendingAttach && recoverable.attachment ? withoutUpload(recoverable.attachment) : undefined;
    if (!persistDraft({ text: next, ...(adopted ? { attachment: adopted } : {}) })) return;
    try {
      clearRecoverableDraft(origin.serverId, origin.name);
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error());
    }
    if (adopted) setPendingAttach(adopted);
    setRecoverable(null);
    setText(next);
    setSelection({ start: next.length, end: next.length });
  }, [recoverable, persistDraft, origin, pendingAttach]);

  const handleDiscardDraft = useCallback(() => {
    try {
      clearRecoverableDraft(origin.serverId, origin.name);
      // Nada gravado desde a recriação: a chave principal ainda é o antigo e o reofereceria.
      if (!draftRef.current) clearDraft(origin.serverId, origin.name);
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error());
      return;
    }
    if (recoverable?.attachment && recoverable.attachment.uri !== pendingAttach?.uri) dropCopy(recoverable.attachment.uri);
    setRecoverable(null);
  }, [origin, recoverable, pendingAttach]);

  const handleRereadDraft = useCallback(() => {
    const load = loadDraft(origin.serverId, origin.name, transcriptRef.current);
    setDraftIssue(load.issue);
    if (load.blocked) return;
    // Texto igual ao último guardado não foi digitado no bloqueio: se for de sessão morta, já está no recuperável.
    const typed = textRef.current === (draftRef.current?.text ?? '') ? '' : textRef.current;
    blockedRef.current = false;
    setReadBlocked(false);
    draftRef.current = load.draft;
    setSubmission(load.draft?.submission ?? null);
    setPendingAttach(load.draft?.attachment ?? null);
    setRecoverable(load.recoverable);
    setDictation(load.dictation);
    // O que a pessoa digitou enquanto a leitura falhava fica depois do guardado.
    const next = joinDrafts(load.draft?.text ?? '', typed);
    persistText(next);
    setText(next);
  }, [origin, persistText]);
  const [sending, setSending] = useState(false);
  const sendingRef = useRef(false);
  const [uploading, setUploading] = useState(false);
  const [error, setError] = useState('');
  const steeringRef = useRef(false);
  const [steering, setSteering] = useState(false);
  const [steerFeedback, setSteerFeedback] = useState('');
  const [transcribing, setTranscribing] = useState(false);
  const transcribingRef = useRef(false);
  const mountedRef = useRef(true);
  const activeRef = useRef(AppState.currentState === 'active');
  const activityGenerationRef = useRef(0);
  const recordingTargetRef = useRef<{ server: Server; snapshot: ConversationDraft; generation: number; estilo?: string } | null>(null);
  const micStartingRef = useRef(false);
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
  const dictationStyle = useDictationStyleLabel();
  const [commandSheetOpen, setCommandSheetOpen] = useState(false);
  // `/model` e `/effort` abrem a folha do chip da sessão; o número sobe a cada pedido.
  const [selectorRequest, setSelectorRequest] = useState<{ which: 'model' | 'effort'; n: number } | null>(null);
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
        cancelarAuto();
      }
    });
    return () => {
      mountedRef.current = false;
      cancelarAuto();
      sub.remove();
    };
  }, [cancelarAuto]);

  const handleChangeText = useCallback(
    (v: string) => {
      textRef.current = v;
      persistText(v);
      setText(v);
      if (undo) limparUndo();
      if (autoN !== null) cancelarAuto();
    },
    [undo, autoN, limparUndo, cancelarAuto, persistText],
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
  const composedText = pendingAttach?.uploadedPath
    ? withAttach(text.trim(), attachInsert(pendingAttach, pendingAttach.uploadedPath)) : text.trim();
  const submissionBlocksSend = !!submission && (isSubmitting(serverId, name) || submission.text.trim() !== composedText);
  const hasContent = text.trim().length > 0 || pendingAttach !== null;

  // A primeira mensagem já pode ter chegado antes de esta tela montar: nunca criar outro eco.
  const sendText = useCallback(async (value: string, revision?: number, explicit = false): Promise<void> => {
    if (!firstInputId) return chat.send(value, revision);
    const snapshot = readFirstInput(serverId, name);
    // Tentativa já confirmada e apagada enquanto o campo ainda mostrava o texto do handoff: não reenviar.
    const handoff = adoptedDraftRef.current;
    if (!snapshot && handoff !== undefined && value.trim() === handoff.trim()) {
      adoptedDraftRef.current = undefined;
      if (persistText('')) setText('');
      return;
    }
    if (snapshot?.id !== firstInputId) {
      return chat.send(value, revision);
    }
    if (snapshot.phase === 'send_unknown') {
      if (!explicit || readDraft(serverId, name)?.submission) {
        throw new Error(useNewConversation.getState().issues[serverId]?.message ?? m.nova_conversa_envio_incerto());
      }
      if (!abandonUnknownAttempt(serverId, snapshot.id)) {
        throw new Error(useNewConversation.getState().issues[serverId]?.message ?? m.nova_conversa_envio_incerto());
      }
      return chat.send(value, revision);
    }
    if (value.trim() !== firstInputMessage(snapshot).trim()) {
      return chat.send(value, revision);
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
  }, [serverId, name, firstInputId, draft, chat, persistText]);

  const handleSend = useCallback(async () => {
    const trimmed = text.trim();
    const hasAttach = pendingAttach !== null;
    if (!trimmed && !hasAttach) return;
    if (submissionBlocksSend) return;
    if (sendingRef.current || sending || uploading) return;
    // `/btw` não é mensagem nem entra na fila: abre a pergunta lateral. Só o Claude tem o comando.
    const btw = !hasAttach && isClaude ? sideQuestionOf(trimmed) : null;
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
    setSending(true);
    setError('');
    const stop = () => {
      setSending(false);
      sendingRef.current = false;
    };
    let finalText = trimmed;
    const attach = pendingAttach;
    if (attach) {
      const target = { serverId: origin.serverId, name: origin.name, transcript: transcriptRef.current };
      let path = reusableUploadPath(attach, target);
      if (!path) {
        const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
        if (!server) {
          setError(m.chat_servidor_removido());
          stop();
          return;
        }
        setUploading(true);
        try {
          const blob = await (await fetch(attach.uri)).blob();
          path = (await uploadFileForServer(server, origin.name, new File([blob], attach.name, { type: attach.mime }))).path;
        } catch (e) {
          // O backend diz o motivo (tamanho, formato); o texto e o anexo continuam no rascunho.
          setError(e instanceof Error && e.message ? `${m.board_falha_upload()}: ${e.message}` : m.board_falha_upload());
          stop();
          return;
        } finally {
          setUploading(false);
        }
        const uploaded = { ...attach, uploadedPath: path, uploadedFor: target };
        if (!persistDraft({ attachment: uploaded })) {
          stop();
          return;
        }
        setPendingAttach(uploaded);
      }
      finalText = withAttach(trimmed, attachInsert(attach, path));
    }
    if (!finalText.trim()) {
      stop();
      return;
    }
    let groupPendingId: string | null = null;
    try {
      const snapshot = firstInputId ? readFirstInput(serverId, name) : null;
      const firstDraft = !!snapshot && snapshot.id === firstInputId && firstInputMessage(snapshot).trim() === finalText.trim();
      const sendToPairNow = !firstDraft && sendToPair && !!pairPeers?.length && !finalText.trimStart().startsWith('/');
      if (sendToPairNow && pairPeers?.length) {
        const recipients = [name, ...pairPeers];
        await submitConversationDraft(serverId, name, finalText, async () => {
          groupPendingId = `pending-group-${Date.now()}`;
          chat.use.setState((current) => ({ pending: [...current.pending, { id: groupPendingId!, text: finalText }] }));
          const results = await broadcast(recipients, finalText);
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
        await sendText(finalText, sentRevision, true);
      }
      setBar(null);
      if (attach && persistDraft({ attachment: null })) {
        dropCopy(attach.uri);
        setPendingAttach(null);
        // Outra tela desta conversa aberta durante o envio ainda mostra o anexo: o aviso vem depois da limpeza.
        chat.use.setState((s) => ({ draftUpdate: s.draftUpdate + 1 }));
      }
    } catch (e) {
      if (groupPendingId) {
        chat.use.setState((current) => ({ pending: current.pending.filter((item) => item.id !== groupPendingId) }));
      }
      const msg = e instanceof Error ? e.message : m.composer_falha_envio();
      setError(msg);
    } finally {
      setSending(false);
      sendingRef.current = false;
    }
  }, [text, sending, uploading, chat, sendText, limparUndo, cancelarAuto, pendingAttach, serverId, name, firstInputId, pairPeers, sendToPair, persistText, persistDraft, origin, submissionBlocksSend, isClaude, setBar]);

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
          const toSend = autoTextoRef.current.trim();
          if (!toSend) return;
          limparUndo();
          void sendText(toSend, revision)
            .then(() => { setError(''); setBar(null); })
            .catch((e: unknown) => {
              const msg = e instanceof Error ? e.message : m.composer_falha_envio();
              setError(msg);
            });
          return;
        }
        setAutoN(Math.ceil(rest / 1000));
      }, 250);
    },
    [cancelarAuto, sendText, limparUndo, setBar],
  );

  const offerUndo = useCallback((before: string, raw: string, cleaned: string, revision: number) => {
    limparUndo();
    if (!raw.trim() || raw.trim() === cleaned.trim()) return;
    setUndo({ before, raw: raw.trim(), revision });
    undoTimerRef.current = setTimeout(limparUndo, 10_000);
  }, [limparUndo]);

  const runTranscription = useCallback(async (voice: DictationDraft, source: DictationSource, server: Server, generation: number, allowAuto: boolean) => {
    if (transcribingRef.current) return;
    transcribingRef.current = true;
    if (mountedRef.current) {
      setTranscribing(true);
      setError('');
      setFailed(null);
    }
    try {
      writeDictation(origin.serverId, origin.name, voice);
    } catch (e) {
      // Sem o áudio guardado, a repetição fica só nesta tela.
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
      if (mountedRef.current) {
        setDictation(completed.dictation);
        if (completed.draft) {
          draftRef.current = completed.draft;
          textRef.current = completed.draft.text;
          setText(completed.draft.text);
          setSelection({ start: completed.draft.text.length, end: completed.draft.text.length });
          offerUndo(voice.before, raw, trimmed, completed.draft.revision);
          setBar(barFor(completed.draft.text, trimmed, raw, applied ?? 'cru', path));
          if (aviso) setError(aviso);
          if (allowAuto && podeEnviarSozinho({
            motivo: voice.motivo, texto: trimmed, aviso: aviso || null,
            rascunhoAntes: !!voice.before.trim() || !!completed.draft.attachment || !!completed.draft.submission,
          })) iniciarAuto(completed.draft.text);
        }
      }
      if (completed.draft && !completed.dictation && voice.audio) dropCopy(voice.audio.uri);
    } catch (e) {
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
  }, [origin, offerUndo, iniciarAuto, navigation, setBar]);

  const handleTranscribe = useCallback(
    async (file: File, motivo: MotivoFim, uri: string, allowAuto = true) => {
      const target = recordingTargetRef.current;
      if (!target || transcribingRef.current) return;
      // Bloqueia outro início enquanto a cópia sai do cache para o diretório do app.
      transcribingRef.current = true;
      if (mountedRef.current) setTranscribing(true);
      let audio: DraftAttachment;
      try {
        audio = await retainDraftAttachment({ uri, name: file.name, mime: file.type, kind: 'file' });
      } catch (e) {
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
      await runTranscription(voice, { file }, target.server, target.generation, allowAuto);
    },
    [runTranscription],
  );

  const { gravando, rms, iniciar, parar } = useDitado({
    onFim: handleTranscribe,
    onErroParada: (e) => {
      if (mountedRef.current) setError(e.message === 'ditado_parada_falhou' ? m.composer_falha_gravacao() : e.message || m.composer_falha_gravacao());
    },
  });
  stopRecordingRef.current = () => { if (gravando) void parar('escondeu'); };

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
    try {
      const server = useServers.getState().servers.find((s) => s.id === origin.serverId);
      if (!server) throw new Error(m.chat_servidor_removido());
      if (blockedRef.current || !persistText(textRef.current)) return;
      const snapshot = draftRef.current!;
      // Mesmo vazio precisa de revisão em disco para comparar o retorno da voz.
      writeDraft(origin.serverId, origin.name, snapshot);
      const style = useDitadoEstiloStore.getState();
      recordingTargetRef.current = { server: { ...server }, snapshot,
        generation: activityGenerationRef.current, estilo: style.pronto ? style.valor : undefined };
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
    } finally { micStartingRef.current = false; }
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
        const r = await relimparDitado(current.raw, version, { ...server });
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
  }, [failed, handleTranscribe, dictation, origin, runTranscription, cancelarAuto]);

  // Recuperar e a inserção automática levam o texto ao campo do mesmo jeito.
  const adoptDictation = useCallback((voice: DictationDraft, before: string,
    result: { draft: ConversationDraft | null; dictation: DictationDraft | null }) => {
    setDictation(result.dictation);
    if (!result.draft) return;
    draftRef.current = result.draft;
    textRef.current = result.draft.text;
    setText(result.draft.text);
    setSelection({ start: result.draft.text.length, end: result.draft.text.length });
    offerUndo(before, voice.raw, voice.text, result.draft.revision);
    setBar(barFor(result.draft.text, voice.text, voice.raw, voice.applied ?? voice.estilo ?? 'cru', voice.serverPath ?? null));
    if (!result.dictation && voice.audio) dropCopy(voice.audio.uri);
    setError(voice.issue);
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
    try {
      clearDictation(origin.serverId, origin.name);
      if (dictation.audio) dropCopy(dictation.audio.uri);
      setDictation(null);
      setError('');
    } catch (e) { setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error()); }
  }, [dictation, origin]);

  // Ditado que voltou com a conversa fora da tela entra sozinho no fim do rascunho intacto.
  // `transcribing` nas dependências: o aviso do fim do POST chega antes de esta tela soltar a trava.
  useEffect(() => {
    if (blockedRef.current || transcribingRef.current) return;
    if (textRef.current !== (draftRef.current?.text ?? '')) return;
    try {
      const before = textRef.current.trim();
      const applied = applyReadyDictation(origin.serverId, origin.name, transcriptRef.current);
      if (applied) adoptDictation(applied.voice, before, applied);
    } catch (e) {
      setDraftIssue(e instanceof Error ? e.message : m.draft_clear_error());
    }
  }, [draftUpdate, transcript, transcribing, origin, adoptDictation]);

  const handleSteer = useCallback(async () => {
    if (steeringRef.current) return;
    steeringRef.current = true;
    setSteering(true);
    setSteerFeedback('');
    setError('');
    try {
      const result = await steerSession(name);
      chat.applySteer(result);
      if ((isCodex || headless) && chat.use.getState().stateEvent?.state === 'working') {
        const sent = result.promoted || (result.confirmed ?? 0) > 0;
        setSteerFeedback(!sent ? m.codex_orientar_sem_envio() : isCodex ? m.codex_orientar_recebido() : m.headless_orientar_recebido());
      }
    } catch (e) {
      const status = (e as { status?: number } | null)?.status;
      setError(status === 409 ? m.composer_fila_erro() : e instanceof Error ? e.message : m.composer_fila_erro());
    } finally {
      steeringRef.current = false;
      setSteering(false);
    }
  }, [name, isCodex, headless, chat]);

  const isKimi = provider === 'kimi';
  // Fila que dá para mandar agora: Kimi (ctrl-s), Codex (turn/steer), Claude sem terminal (stdin)
  // e Claude com terminal (ctrl+x ctrl+s da fila do Claude Code).
  const queuePromotable = isKimi || isCodex || headless || isClaude;
  const showSteer = queuePromotable && state === 'working' && (filaCount > 0 || steering);
  useEffect(() => {
    if (state !== 'working') setSteerFeedback('');
  }, [state]);

  const originServer = useServers((s) => s.servers.find((x) => x.id === origin.serverId));
  const dictationPlayer = dictation ? dictationAudio(originServer, origin.name, dictation.audio, dictation.serverPath) : null;
  // Como no PC: versões e player só enquanto o texto ditado está intacto no campo.
  const barIntact = !!bar && (revising !== null || dictatedInField(bar, text));
  // O aviso do ditado (limpeza do registro falhou) já traz o mesmo áudio: um player só.
  const barPlayer = bar && barIntact && !gravando && !dictationPlayer ? dictationAudio(originServer, origin.name, null, bar.serverPath) : null;
  const barVersions = barIntact && !!bar?.raw;
  // Gravando ou transcrevendo, o texto do campo ainda vai mudar: enviar agora mandaria a metade.
  // Transcrição desta tela ou de uma montagem anterior da mesma conversa ainda no ar.
  const busyVoice = transcribing || dictation?.status === 'pending';
  const canSend = hasContent && !sending && !uploading && !readBlocked && !submissionBlocksSend && !gravando && !busyVoice;
  // Sem terminal, pergunta pendente só tem saída pelo Parar (não há pane para mandar Esc).
  const canInterrupt = state === 'working' || (headless && state === 'awaiting_input');
  // Campo vazio: o Parar ocupa o lugar do Enviar (como no PC). Com texto ou fila os dois ficam,
  // porque a mensagem entra na fila.
  // Com texto a linha é Orientar + Enviar, como no PWA; parar pede o campo vazio.
  const showStop = !!onStop && canInterrupt && !hasContent;
  const showSend = !showStop || hasContent || sending || uploading || filaCount > 0;

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
  const { commands, error: commandsError, retry: retryCommands } = useSessionCommands(name, provider, commandSheetOpen || slashQuery !== null);
  const suggestions = useMemo(() => slashMatches(commands ?? [], slashQuery), [commands, slashQuery]);

  const replaceText = useCallback((next: string, cursor = next.length) => {
    textRef.current = next;
    persistText(next);
    setText(next);
    setSelection({ start: cursor, end: cursor });
  }, [persistText]);

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

  const openSelector = useCallback((which: 'model' | 'effort') => {
    setSelectorRequest((cur) => ({ which, n: (cur?.n ?? 0) + 1 }));
  }, []);

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
    if (!persistDraft({ attachment: kept })) {
      dropCopy(kept.uri);
      return;
    }
    if (pendingAttach && pendingAttach.uri !== kept.uri) dropCopy(pendingAttach.uri);
    setPendingAttach({ ...kept, size: picked.size });
  }, [persistDraft, pendingAttach]);

  const handlePicked = useCallback((picked: PickedAttachment) => {
    setError('');
    void adoptPicked(picked);
  }, [adoptPicked]);

  // Colar imagem ou arquivo no campo vira o anexo, como o paste do PWA. O composer leva um anexo só.
  const handlePaste = useCallback((files: PastedFile[], erro: string | null) => {
    if (erro) { setError(formataErro(erro) ?? erro); return; }
    const f = files[0];
    if (!f) return;
    const mime = f.type || 'application/octet-stream';
    handlePicked({ uri: f.uri, name: f.fileName, mime, kind: mime.startsWith('image/') ? 'image' : 'file', size: f.fileSize });
    if (files.length > 1) setError(m.composer_colou_so_um());
  }, [handlePicked]);

  const router = useRouter();

  const handleRemoveAttach = useCallback(() => {
    if (!pendingAttach || !persistDraft({ attachment: null })) return;
    dropCopy(pendingAttach.uri);
    setPendingAttach(null);
  }, [pendingAttach, persistDraft]);

  useEffect(() => {
    return () => {
      if (undoTimerRef.current) clearTimeout(undoTimerRef.current);
      if (autoTimerRef.current) clearInterval(autoTimerRef.current);
    };
  }, []);

  // Foco tinge a borda com o acento, como no PWA: o campo não tem cursor de contorno próprio.
  const acento = hexParaRgb(theme.tokens.accent.base);
  const bordaFoco = focado && acento ? { borderColor: `rgba(${acento.join(',')},0.45)` } : null;

  return (
    <Glass variant="chrome" style={[styles.glass, bordaFoco]}>
        {/* Fila esperando o turno: diz quantas e dá a saída de mandar agora (como o chip do PWA). */}
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
              {steering ? m.askq_enviando() : isCodex || headless ? m.codex_orientar() : m.composer_fila_acao()}
            </Text>
          </Pressable>
        ) : null}

        {steerFeedback && state === 'working' ? <Text style={[styles.hint, { color: theme.tokens.text.secondary }]} accessibilityLiveRegion="polite">{steerFeedback}</Text> : null}

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

        {pendingAttach ? (
          <AttachmentPreview attachment={pendingAttach} onRemove={handleRemoveAttach} disabled={sending} />
        ) : null}


        <SlashSuggest matches={suggestions} onPick={pickSlash}
          error={slashQuery !== null ? commandsError : ''} onRetry={retryCommands} />

        {/* Campo em linha própria: dividindo a linha com os botões ele ficava só com a sobra. */}
        <View style={styles.inputWrap}>
          <MultilineInput
            ref={inputRef}
            accessible
            accessibilityLabel={m.composer_mensagem()}
            value={text}
            onChangeText={handleChangeText}
            placeholder={busyVoice ? m.composer_transcrevendo_audio()
              : provider ? m.composer_mensagem_para({ nome: providerName(provider as Provider) }) : m.composer_mensagem()}
            maxHeight={120}
            onKeyPress={handleKeyPress}
            selection={selection}
            onSelectionChange={(e) => {
              setSelection(undefined);
              setCaret(e.nativeEvent.selection);
            }}
            onFocus={() => setFocado(true)}
            onBlur={() => setFocado(false)}
            onPaste={handlePaste}
            style={gravando ? styles.inputEscondido : undefined}
            editable={!gravando}
            accessibilityElementsHidden={gravando}
            importantForAccessibility={gravando ? 'no-hide-descendants' : 'auto'}
          />
          {/* Gravando, a onda ocupa o lugar do campo (montado por baixo, com o texto intacto): a caixa
              não cresce nem muda de forma por causa do ditado. */}
          {gravando ? (
            <View style={styles.ondaNoCampo}>
              {/* Sem alvo, o fim da gravação não transcreve: é o Cancelar do PC, que joga o áudio fora. */}
              <RecordingWave rms={rms} onCancel={() => { recordingTargetRef.current = null; void parar('botao'); }} />
            </View>
          ) : null}
        </View>

        {/* Linha do app de PC: + e microfone sem moldura à esquerda; o chip da sessão e o Enviar
            fecham a linha. Com o Orientar, o chip encolhe o nome do modelo mas nada sai, como no PWA. */}
        <View style={styles.row}>
          <Pressable
            onPress={() => setAttachMenuOpen(true)}
            disabled={uploading || sending || gravando}
            hitSlop={5}
            accessibilityState={{ disabled: uploading || sending || gravando, busy: uploading }}
            style={({ pressed }) => [styles.iconBtn, pressed && styles.iconBtnPressed, (uploading || sending || gravando) && styles.iconBtnDisabled]}
            accessibilityLabel={m.composer_adicionar_ao_chat()}
            accessibilityRole="button"
          >
            {uploading ? (
              <ActivityIndicator size="small" color={theme.tokens.text.secondary} />
            ) : (
              <Icon name="Plus" size={22} color={theme.tokens.text.secondary} />
            )}
          </Pressable>

          <Pressable
              onPress={handleMicPress}
              disabled={transcribing || sending || (!!dictation && !gravando)}
              hitSlop={5}
              accessibilityState={{ disabled: transcribing || sending || (!!dictation && !gravando), busy: busyVoice }}
              style={({ pressed }) => [
                styles.iconBtn,
                pressed && styles.iconBtnPressed,
                (transcribing || sending || (!!dictation && !gravando)) && styles.iconBtnDisabled,
              ]}
              accessibilityLabel={gravando ? m.composer_parar_gravacao() : m.composer_gravar_audio()}
              accessibilityRole="button"
              // O estilo do ditado mora no + e no toque longo; o leitor de tela recebe a mesma ação
              // nomeada, e a dica diz qual estilo vale antes de falar.
              onLongPress={gravando ? undefined : () => setStyleMenuOpen(true)}
              accessibilityHint={m.composer_mic_style_hint({ estilo: dictationStyle })}
              accessibilityActions={gravando ? undefined : [{ name: 'longpress', label: m.ditado_estilo_titulo() }]}
              onAccessibilityAction={(e) => {
                if (e.nativeEvent.actionName === 'longpress' && !gravando) setStyleMenuOpen(true);
              }}
            >
              {/* Gravando, o vermelho é o aviso: o quadrado diz "toque para parar". */}
              {gravando
                ? <Icon name="Square" size={16} color={theme.tokens.status.error} />
                : <Icon name="Mic" size={20} color={theme.tokens.text.secondary} />}
          </Pressable>

          <View style={styles.spacer} />

          <SessionSettingsButton serverId={serverId} name={name} provider={provider} headless={headless} openRequest={selectorRequest} />


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

          {/* Vazio, o círculo fica apagado em vez de sumir: o claro cheio só acende com o que mandar. */}
          {showSend ? (
            <Pressable
              onPress={() => { void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light); void handleSend(); }}
              disabled={!canSend}
              hitSlop={5}
              accessibilityState={{ disabled: !canSend, busy: sending || uploading }}
              style={({ pressed }) => [
                styles.roundBtn,
                { backgroundColor: canSend ? theme.tokens.text.primary : theme.tokens.border.default },
                pressed && canSend && styles.sendBtnPressed,
              ]}
              accessibilityLabel={m.composer_enviar_mensagem()}
              accessibilityHint={filaCount > 0 ? m.composer_fila_contagem({ n: filaCount }) : undefined}
              accessibilityRole="button"
            >
              <Icon name="ArrowUp" size={18} color={canSend ? theme.tokens.bg.base : theme.tokens.text.muted} />
              {/* Fila (pending local + queued-* do SSE): selo no canto, dentro do botão para o Android não cortar. */}
              {filaCount > 0 ? (
                <View style={[styles.queueBadge, { backgroundColor: theme.tokens.status.warning }]} pointerEvents="none">
                  <Text style={[styles.queueBadgeText, { color: theme.tokens.text.inverse }]} maxFontSizeMultiplier={GLYPH_MAX_SCALE}>
                    {filaCount > 9 ? '9+' : filaCount}
                  </Text>
                </View>
              ) : null}
            </Pressable>
          ) : null}
        </View>

        {bar && (barVersions || barPlayer) ? (
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
            {barPlayer ? <AudioChip key={barPlayer.uri} uri={barPlayer.uri} headers={barPlayer.headers} name={barPlayer.name} /> : null}
          </View>
        ) : null}

        {dictation && !busyVoice ? (
          <View style={styles.errorRow}>
            <Text style={[styles.hint, styles.recoverText, { color: theme.tokens.text.muted }]} accessibilityLiveRegion="polite">
              {dictation.status === 'applied' ? m.composer_ditado_aplicado()
                : dictation.text ? m.composer_ditado_recuperavel({ text: dictation.text.slice(0, 80) })
                : dictation.issue || m.composer_ditado_interrompido()}
            </Text>
            {dictationPlayer ? <AudioChip key={dictationPlayer.uri} uri={dictationPlayer.uri} headers={dictationPlayer.headers} name={dictationPlayer.name} /> : null}
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

        {error ? (
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

        {submission ? (
          <View style={styles.undoRow}>
            <Text style={[styles.hint, styles.recoverText, { color: theme.tokens.text.muted }]} accessibilityLiveRegion="polite">
              {submission.status === 'rejected' ? m.composer_submission_rejected()
                : submission.status === 'sending' || sending ? m.composer_submission_sending() : m.chat_envio_incerto()}
            </Text>
            {submission.status === 'rejected' || (submission.status === 'unknown' && !sending) ? (
              <Pressable onPress={handleRecoverSubmission} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
                <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.composer_draft_recover()}</Text>
              </Pressable>
            ) : null}
            {submission.status === 'unknown' ? (
              <Pressable onPress={chat.retry} style={[styles.undoBtn, { borderColor: theme.tokens.border.subtle }]} accessibilityRole="button">
                <Text style={[styles.undoText, { color: theme.tokens.accent.base }]}>{m.composer_submission_check()}</Text>
              </Pressable>
            ) : null}
          </View>
        ) : null}

        {recoverable ? (
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
          <SideQuestionSheet open name={name} question={sideQuestion} onClose={() => setSideQuestion(null)} />
        ) : null}

        <DictationStyleMenu open={styleMenuOpen} onClose={() => setStyleMenuOpen(false)} />

        <AttachSheet
          open={attachMenuOpen}
          onClose={() => setAttachMenuOpen(false)}
          onPick={handlePicked}
          onError={setError}
          onSessionAttachments={() => router.push(`/s/${origin.serverId}/${origin.name}/attachments` as never)}
          onCommands={() => setCommandSheetOpen(true)}
          dictationStyle={{ label: dictationStyle, onPress: () => setStyleMenuOpen(true) }}
          sendToGroup={pairPeers?.length
            ? { label: m.composer_mandar_tambem({ n: pairPeers.join(', ') }), on: sendToPair, onPress: () => setSendToPair((c) => !c) }
            : undefined}
        />
    </Glass>
  );
}

const styles = StyleSheet.create((theme) => ({
  // Caixa flutuante do app de PC; a linha de status vem logo embaixo, por isso a margem curta.
  glass: {
    marginHorizontal: theme.base.space[2],
    borderRadius: 22,
    paddingHorizontal: 12,
    paddingTop: 12,
    paddingBottom: 8,
    gap: theme.base.space[1],
  },
  // Chips acima da caixa (fila, par): a mesma silhueta do chip da sessão; hitSlop leva a 44 pt.
  steerBtn: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    height: 30,
    borderRadius: 15,
    paddingHorizontal: 10,
  },
  steerText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  steerCount: {
    fontSize: theme.base.text.xs,
  },
  pairChip: {
    alignSelf: 'flex-start',
    flexDirection: 'row',
    alignItems: 'center',
    gap: 5,
    height: 30,
    maxWidth: 240,
    borderRadius: 15,
    paddingHorizontal: 10,
  },
  pairChipLabel: {
    flexShrink: 1,
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  row: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: 6,
  },
  spacer: {
    flex: 1,
    minWidth: 0,
  },
  // O campo é o próprio vidro, sem caixa dentro da caixa.
  inputWrap: {
    minHeight: 40,
    justifyContent: 'center',
    paddingHorizontal: theme.base.space[1],
  },
  inputEscondido: { opacity: 0 },
  ondaNoCampo: { ...StyleSheet.absoluteFillObject, justifyContent: 'center', paddingHorizontal: theme.base.space[1] },
  // 34 pt + hitSlop 5 = 44 pt de toque; o ícone fica sem fundo, como no PC.
  iconBtn: {
    width: 34,
    height: 34,
    borderRadius: 17,
    alignItems: 'center',
    justifyContent: 'center',
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
    borderRadius: 3,
  },
  queueBadge: {
    position: 'absolute',
    top: 0,
    right: 0,
    minWidth: 15,
    height: 15,
    paddingHorizontal: 3,
    borderRadius: 8,
    alignItems: 'center',
    justifyContent: 'center',
  },
  queueBadgeText: {
    fontSize: 11,
    fontWeight: '700',
    lineHeight: 13,
  },
  sendBtnPressed: {
    opacity: 0.8,
  },
  autoChip: {
    minWidth: 44,
    height: 30,
    justifyContent: 'center',
    alignSelf: 'flex-start',
    borderRadius: 15,
    paddingHorizontal: 10,
  },
  autoText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
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
    paddingVertical: 4,
  },
  undoText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  error: {
    flexShrink: 1,
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[1],
  },
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
    paddingVertical: 4,
  },
  retryText: {
    fontSize: theme.base.text.xs,
    fontWeight: '600',
  },
  versionRow: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    alignItems: 'center',
    gap: theme.base.space[1],
  },
  versionBtn: {
    minHeight: 30,
    borderRadius: 15,
    paddingHorizontal: 10,
    justifyContent: 'center',
  },
  hint: {
    fontSize: theme.base.text.xs,
    paddingHorizontal: theme.base.space[1],
  },
}));
