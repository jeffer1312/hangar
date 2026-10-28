import { create } from 'zustand';
import {
  basename, createSessionForServer, fetchSessionsForServer, getCreationProgress, getHistory,
  sanitizeSessionName, sendInputForServer, transitionFirstConversation, uniqueSessionName, uploadFileForServer,
} from '@hangar/core';
import type {
  ChatEvent, CreateSessionBody, FirstConversationAttempt, FirstConversationEvent, Server, SessionInfo,
} from '@hangar/core';
import { prefs } from './prefs';
import { useServers } from './servers';
import { clearDraft, readDraft, withoutUpload, writeDraft, type DraftAttachment } from './drafts';
import { submitConversationDraft } from './chat';
import * as m from '../paraglide/messages';

export type NewConversationIssue =
  | { kind: 'rejected'; message: string }
  | { kind: 'unknown'; message: string; events?: ChatEvent[] }
  | { kind: 'in_progress'; message: string }
  | { kind: 'not_found'; message: string }
  | { kind: 'conflict'; message: string }
  | { kind: 'candidate'; message: string; session: SessionInfo }
  | { kind: 'recover_failed'; message: string }
  | { kind: 'local'; message: string };

export type NewConversationInput = {
  // Sem `name`, o store gera o nome estável; com ele, o nome escolhido fica congelado na tentativa.
  body: Omit<CreateSessionBody, 'name' | 'cwd'> & { cwd: string; name?: string };
  // Nome da reserva pré-aquecida (`body.prewarm`): sem nome digitado, é o que a adoção espera.
  reserveName?: string;
  text: string;
  // Escolhido antes de a sessão existir: o upload é por sessão, então ele sobe depois que ela nasce.
  attachment?: DraftAttachment | null;
};

export const attachInsert = (attach: DraftAttachment, path: string) =>
  `📎 ${attach.kind === 'image' ? m.board_imagem() : m.board_arquivo()}: ${path}`;
export const withAttach = (text: string, insert: string) => (text ? `${text} — ${insert}` : insert);

// Campo extra da tentativa: as transições do core espalham o objeto e o preservam.
export function attemptAttachment(attempt: FirstConversationAttempt): DraftAttachment | null {
  const a = (attempt as { attachment?: Partial<DraftAttachment> | null }).attachment;
  return a && typeof a.uri === 'string' && typeof a.name === 'string' && typeof a.mime === 'string'
    && (a.kind === 'image' || a.kind === 'file') ? a as DraftAttachment : null;
}

// O caminho do upload mora no rascunho da sessão, onde o composer da conversa também o grava.
function uploadedPath(attempt: FirstConversationAttempt, attach: DraftAttachment): string | null {
  if (!attempt.sessionName) return null;
  let kept: DraftAttachment | null | undefined;
  try { kept = readDraft(attempt.serverId, attempt.sessionName)?.attachments.find((a) => a.uri === attach.uri); } catch { return null; }
  const done = kept?.uploadedFor;
  return kept?.uri === attach.uri && kept.uploadedPath && done?.serverId === attempt.serverId
    && done.name === attempt.sessionName ? kept.uploadedPath : null;
}

// A primeira mensagem como ela sai: o texto digitado e, com o anexo já enviado, a citação dele.
export function firstInputMessage(attempt: FirstConversationAttempt): string {
  const attach = attemptAttachment(attempt);
  const path = attach && uploadedPath(attempt, attach);
  return attach && path ? withAttach(attempt.text.trim(), attachInsert(attach, path)) : attempt.text;
}

type State = {
  attempts: Record<string, FirstConversationAttempt>;
  issues: Record<string, NewConversationIssue | null>;
  busy: Record<string, boolean>;
};

const KEY = (serverId: string) => `create.attempt.v1:${serverId}`;
const PHASES = new Set(['draft', 'creating', 'create_unknown', 'created', 'sending', 'send_unknown', 'sent']);
// Promessas em voo por servidor: dois toques (ou remontagem) no meio do GET/POST recebem a mesma.
const inFlight = new Map<string, Promise<void>>();
const refusedNames = new Map<string, Set<string>>();

export const useNewConversation = create<State>(() => ({ attempts: {}, issues: {}, busy: {} }));

function setIssue(serverId: string, issue: NewConversationIssue | null) {
  useNewConversation.setState((s) => ({ issues: { ...s.issues, [serverId]: issue } }));
}

function isAttempt(value: unknown, serverId: string): value is FirstConversationAttempt {
  if (value === null || typeof value !== 'object') return false;
  const a = value as Record<string, unknown>;
  const body = a.body as Record<string, unknown> | null;
  return typeof a.id === 'string' && a.serverId === serverId && typeof a.text === 'string'
    && typeof a.phase === 'string' && PHASES.has(a.phase)
    && (a.sessionName === null || typeof a.sessionName === 'string')
    && body !== null && typeof body === 'object'
    && typeof body.name === 'string' && typeof body.cwd === 'string';
}

// Grava antes de publicar na memória: o que a tela mostra sempre existe no disco.
function persist(attempt: FirstConversationAttempt): void {
  prefs.set(KEY(attempt.serverId), JSON.stringify(attempt));
  // O ACK da criação fica guardado mesmo se a transferência ao rascunho falhar.
  if (attempt.sessionName && !readDraft(attempt.serverId, attempt.sessionName)) {
    writeDraft(attempt.serverId, attempt.sessionName, {
      version: 1, text: attempt.phase === 'sent' ? '' : attempt.text, revision: 1,
      transcript: null, attachments: attempt.phase === 'sent' ? [] : [attemptAttachment(attempt)].filter((a) => a !== null), submission: null,
    });
  }
  useNewConversation.setState((s) => ({ attempts: { ...s.attempts, [attempt.serverId]: attempt } }));
}

// Só aplica o evento se a tentativa gravada ainda é a mesma: resultado antigo nunca pisa na nova.
function apply(attempt: FirstConversationAttempt, event: FirstConversationEvent): FirstConversationAttempt | null {
  const current = useNewConversation.getState().attempts[attempt.serverId];
  if (!current || current.id !== attempt.id) return null;
  const next = transitionFirstConversation(current, event);
  if (next !== current) persist(next);
  return next;
}

function serverById(serverId: string): Server | null {
  return useServers.getState().servers.find((s) => s.id === serverId) ?? null;
}

function statusOf(cause: unknown): number | null {
  const status = (cause as { status?: unknown } | null)?.status;
  return typeof status === 'number' ? status : null;
}

function messageOf(cause: unknown): string {
  return cause instanceof Error && cause.message ? cause.message : m.criar_sessao_erro();
}

function newId(): string {
  return `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 8)}`;
}

// Nome de quem deixou o campo em branco, pela regra das outras telas e do servidor (pasta, depois pasta-2). A lista
// só evita um nome já visível: o backend arbitra. `known` diz se ela respondeu.
export async function autoSessionName(server: Server, cwd: string, refused: Iterable<string> = []): Promise<{ name: string; known: boolean }> {
  let taken = new Set<string>();
  let known = true;
  try { taken = new Set((await fetchSessionsForServer(server)).map((s) => s.name)); } catch { known = false; }
  for (const name of refused) taken.add(name);
  return { name: uniqueSessionName(basename(cwd), taken), known };
}

// Processo reaberto: creating/sending não voltam a disparar POST, viram incertos.
export function restoreAttempt(serverId: string): FirstConversationAttempt | null {
  const raw = prefs.getString(KEY(serverId));
  if (raw === undefined) return null;
  let parsed: unknown;
  try { parsed = JSON.parse(raw); } catch { parsed = null; }
  if (!isAttempt(parsed, serverId)) {
    prefs.remove(KEY(serverId));
    setIssue(serverId, { kind: 'local', message: m.nova_conversa_tentativa_invalida() });
    return null;
  }
  const memory = useNewConversation.getState().attempts[serverId];
  if (memory?.id === parsed.id && inFlight.has(serverId)) return memory;
  let attempt = parsed;
  if (attempt.phase === 'creating') attempt = transitionFirstConversation(attempt, { type: 'create_unknown' });
  if (attempt.phase === 'sending') attempt = transitionFirstConversation(attempt, { type: 'send_unknown' });
  try { persist(attempt); } catch {
    setIssue(serverId, { kind: 'local', message: m.nova_conversa_salvar_erro() });
    return null;
  }
  if (attempt.phase === 'create_unknown' && !useNewConversation.getState().issues[serverId]) {
    setIssue(serverId, { kind: 'unknown', message: m.nova_conversa_criacao_incerta() });
  }
  if (attempt.phase === 'send_unknown' && !useNewConversation.getState().issues[serverId]) {
    setIssue(serverId, { kind: 'unknown', message: m.nova_conversa_envio_incerto() });
  }
  return attempt;
}

async function runCreate(serverId: string, input: NewConversationInput, retried = false): Promise<void> {
  const current = useNewConversation.getState().attempts[serverId] ?? restoreAttempt(serverId);
  if (!current && prefs.getString(KEY(serverId)) !== undefined) return;
  if (current && current.phase !== 'draft') return;

  const server = serverById(serverId);
  if (!server) { setIssue(serverId, { kind: 'local', message: m.nova_conversa_servidor_ausente() }); return; }

  // Nome escolhido pela pessoa não ganha sufixo: conflito volta como 409 e ela decide.
  const explicit = input.body.name?.trim();
  const id = newId();
  // Nome automático já recusado não volta: sem a lista, o mesmo nome seria recusado a cada envio.
  // A reserva guarda o nome dela no servidor; a repetição por nome em uso volta ao automático.
  const finalName = explicit || (!retried && input.reserveName)
    || (await autoSessionName(server, input.body.cwd, refusedNames.get(serverId) ?? [])).name;
  // A reserva só vai com o nome dela: outro nome (digitado ou da repetição) a deixaria adotada sob nome errado.
  const { prewarm, ...base } = input.body;
  // Branch nova sem nome digitado segue o nome final da sessão, só conhecido aqui, com a limpeza que o
  // backend faz no nome: o digitado pode ter espaço e acento.
  const draft: FirstConversationAttempt = Object.assign({
    id, serverId, text: input.text, phase: 'draft' as const, sessionName: null,
    body: { ...base, name: finalName, ...(prewarm && finalName === input.reserveName ? { prewarm } : {}),
      ...(input.body.new_branch && !input.body.branch?.trim() ? { branch: sanitizeSessionName(finalName) } : {}) },
  }, input.attachment ? { attachment: input.attachment } : {});
  const creating = transitionFirstConversation(draft, { type: 'begin' });
  try { persist(creating); } catch {
    setIssue(serverId, { kind: 'local', message: m.nova_conversa_salvar_erro() });
    return;
  }
  setIssue(serverId, null);

  let session: SessionInfo;
  try {
    session = await createSessionForServer(server, creating.body);
  } catch (cause) {
    const status = statusOf(cause);
    // 4xx definitivo não criou nada; 409, 408, 5xx e rede podem ter criado. Nome em uso é recusa:
    // a sessão com esse nome é outra, e recuperá-la pelo nome mandaria a mensagem para a conversa errada.
    const nameInUse = (cause as { code?: unknown } | null)?.code === 'erro_nome_em_uso';
    if (nameInUse && !explicit) refusedNames.set(serverId, new Set([...(refusedNames.get(serverId) ?? []), finalName]));
    const rejected = nameInUse
      || (status !== null && status >= 400 && status < 500 && status !== 408 && status !== 409);
    if (!apply(creating, { type: rejected ? 'create_rejected' : 'create_unknown' })) return;
    // Nome automático em uso (lista fora do ar, ou outra tela criou no meio): uma tentativa com o próximo.
    if (nameInUse && !explicit && !retried) return runCreate(serverId, input, true);
    setIssue(serverId, rejected
      ? { kind: 'rejected', message: messageOf(cause) }
      : status === 409
        ? { kind: 'conflict', message: messageOf(cause) }
        : { kind: 'unknown', message: m.nova_conversa_criacao_incerta() });
    return;
  }
  try { apply(creating, { type: 'create_ok', sessionName: session.name }); } catch {
    // Mantém uma recuperação por leitura disponível, sem publicar destino sem rascunho durável.
    useNewConversation.setState((s) => ({ attempts: { ...s.attempts, [serverId]: {
      ...creating, phase: 'create_unknown',
    } } }));
    setIssue(serverId, { kind: 'local', message: m.nova_conversa_salvar_erro() });
  }
}

function exclusive(serverId: string, work: () => Promise<void>): Promise<void> {
  const running = inFlight.get(serverId);
  if (running) return running;
  useNewConversation.setState((s) => ({ busy: { ...s.busy, [serverId]: true } }));
  const run = work().finally(() => {
    inFlight.delete(serverId);
    useNewConversation.setState((s) => ({ busy: { ...s.busy, [serverId]: false } }));
  });
  inFlight.set(serverId, run);
  return run;
}

// Único caminho que cria. Só sai POST quando não há tentativa ou ela ainda é rascunho.
export function beginAttempt(serverId: string, input: NewConversationInput): Promise<void> {
  return exclusive(serverId, () => runCreate(serverId, input));
}

// A criação e o input são ações separadas; só a sessão já gravada recebe este snapshot.
export function sendFirstInput(serverId: string, attemptId: string): Promise<void> {
  return exclusive(serverId, async () => {
    const attempt = useNewConversation.getState().attempts[serverId] ?? restoreAttempt(serverId);
    if (!attempt || attempt.id !== attemptId || attempt.phase !== 'created' || !attempt.sessionName) return;
    const server = serverById(serverId);
    if (!server) { setIssue(serverId, { kind: 'local', message: m.nova_conversa_servidor_ausente() }); return; }
    const sessionName = attempt.sessionName;
    const attach = attemptAttachment(attempt);
    let path = attach && uploadedPath(attempt, attach);
    if (attach && !path) {
      try {
        const blob = await (await fetch(attach.uri)).blob();
        path = (await uploadFileForServer(server, sessionName, new File([blob], attach.name, { type: attach.mime }))).path;
      } catch (cause) {
        // A sessão existe; o anexo segue na tentativa e no rascunho dela, para reenviar aqui ou na conversa.
        setIssue(serverId, { kind: 'local', message: m.nova_conversa_anexo_falhou({ erro: messageOf(cause) }) });
        return;
      }
      try {
        const kept = readDraft(serverId, sessionName);
        // Removido na conversa antes do upload: não volta à lista.
        const base = kept ?? { version: 1 as const, text: attempt.text, revision: 1, transcript: null, attachments: [attach], submission: null };
        const uploaded = { ...withoutUpload(attach), uploadedPath: path, uploadedFor: { serverId, name: sessionName, transcript: null } };
        if (base.attachments.some((a) => a.uri === attach.uri)) {
          writeDraft(serverId, sessionName, { ...base, attachments: base.attachments.map((a) => (a.uri === attach.uri ? uploaded : a)) });
        }
      } catch {
        setIssue(serverId, { kind: 'local', message: m.nova_conversa_salvar_erro() });
        return;
      }
    }
    const message = attach && path ? withAttach(attempt.text.trim(), attachInsert(attach, path)) : attempt.text;
    // Com anexo o texto enviado difere do rascunho; a revisão explícita deixa o ACK limpar o campo.
    let draftRevision: number | undefined;
    if (attach) {
      try {
        const current = readDraft(serverId, sessionName);
        if (current && current.text.trim() === attempt.text.trim()) draftRevision = current.revision;
      } catch { /* sem revisão, o ACK só não limpa o campo */ }
    }
    const releaseAttachment = () => {
      if (!attach) return;
      try {
        const current = readDraft(serverId, sessionName);
        if (!current?.attachments.some((a) => a.uri === attach.uri)) return;
        const attachments = current.attachments.filter((a) => a.uri !== attach.uri);
        if (!current.text && !current.submission && !attachments.length) clearDraft(serverId, sessionName);
        else writeDraft(serverId, sessionName, { ...current, attachments });
      } catch { /* o anexo já foi; sobra só a prévia no rascunho */ }
    };
    let sending: FirstConversationAttempt | null;
    try { sending = apply(attempt, { type: 'send_begin' }); } catch {
      setIssue(serverId, { kind: 'local', message: m.nova_conversa_salvar_erro() });
      return;
    }
    if (!sending) return;
    setIssue(serverId, null);

    // O catch da rede não pode interpretar uma falha posterior no disco como recusa do servidor.
    let posted = false;
    let acked = false;
    try {
      await submitConversationDraft(serverId, sessionName, message,
        async () => {
          posted = true;
          await sendInputForServer(server, sessionName, message);
          acked = true;
          releaseAttachment();
        }, draftRevision);
    } catch (cause) {
      if (!posted) {
        try { apply(sending, { type: 'send_rejected' }); } catch {
          setIssue(serverId, { kind: 'local', message: m.nova_conversa_resultado_salvar_erro() });
          return;
        }
        setIssue(serverId, { kind: 'local', message: messageOf(cause) });
        return;
      }
      if (acked) {
        try { apply(sending, { type: 'send_ok' }); } catch {
          setIssue(serverId, { kind: 'local', message: m.nova_conversa_resultado_salvar_erro() });
          return;
        }
        setIssue(serverId, { kind: 'local', message: messageOf(cause) });
        return;
      }
      const status = statusOf(cause);
      const rejected = status !== null && status >= 400 && status < 500 && status !== 408;
      try {
        if (!apply(sending, { type: rejected ? 'send_rejected' : 'send_unknown' })) return;
      } catch {
        setIssue(serverId, { kind: 'local', message: m.nova_conversa_resultado_salvar_erro() });
        return;
      }
      setIssue(serverId, rejected
        ? { kind: 'rejected', message: m.nova_conversa_envio_recusado({ erro: messageOf(cause) }) }
        : { kind: 'unknown', message: m.nova_conversa_envio_incerto() });
      return;
    }
    try { apply(sending, { type: 'send_ok' }); } catch {
      setIssue(serverId, { kind: 'local', message: m.nova_conversa_resultado_salvar_erro() });
    }
  });
}

// A rota lê sem consumir: sent não vira draft; sending/incerto exigem conferir antes de reenviar.
export function readFirstInput(serverId: string, name: string): FirstConversationAttempt | null {
  const attempt = useNewConversation.getState().attempts[serverId] ?? restoreAttempt(serverId);
  if (!attempt || attempt.sessionName !== name || attempt.phase === 'draft'
    || attempt.phase === 'creating' || attempt.phase === 'create_unknown') return null;
  return attempt;
}

// Finaliza somente o snapshot cujo ACK já foi gravado, nunca uma tentativa nova ou incerta.
export function confirmFirstInput(attemptId: string): void {
  const attempt = Object.values(useNewConversation.getState().attempts).find((a) => a.id === attemptId);
  if (attempt?.phase !== 'sent') return;
  try { persist(attempt); } catch {
    setIssue(attempt.serverId, { kind: 'local', message: m.nova_conversa_resultado_salvar_erro() });
    return;
  }
  discardAttempt(attempt.serverId, attemptId);
}

// Consulta lista e progresso do MESMO servidor e nome; nunca cria e nunca troca de nome.
export function recoverAttempt(serverId: string): Promise<void> {
  return exclusive(serverId, async () => {
    const attempt = useNewConversation.getState().attempts[serverId];
    if (!attempt || (attempt.phase !== 'create_unknown' && attempt.phase !== 'send_unknown')) return;
    const server = serverById(serverId);
    if (!server) { setIssue(serverId, { kind: 'local', message: m.nova_conversa_servidor_ausente() }); return; }
    const name = attempt.body.name;
    if (attempt.phase === 'send_unknown' && attempt.sessionName) {
      try {
        const events = await getHistory(attempt.sessionName, 120, undefined, 10000, server);
        // Histórico inclui fila, mas texto igual não identifica este POST.
        setIssue(serverId, { kind: 'unknown', message: m.nova_conversa_envio_incerto(), events });
      } catch (cause) {
        setIssue(serverId, { kind: 'recover_failed', message: m.nova_conversa_envio_conferir_erro({ erro: messageOf(cause) }) });
      }
      return;
    }
    try {
      const found = (await fetchSessionsForServer(server)).find((s) => s.name === name);
      if (found) {
        const compatible = found.cwd === attempt.body.cwd
          && (found.provider ?? 'claude') === (attempt.body.provider ?? 'claude');
        setIssue(serverId, compatible
          ? { kind: 'candidate', message: m.nova_conversa_candidata({ nome: name }), session: found }
          : { kind: 'conflict', message: m.nova_conversa_nome_conflito({ nome: name }) });
        return;
      }
      // Lista vazia não prova que falhou: a criação pode estar em curso ou a lista atrasada.
      const progress = await getCreationProgress(name, server);
      setIssue(serverId, progress.step
        ? { kind: 'in_progress', message: m.nova_conversa_criando() }
        : { kind: 'not_found', message: m.nova_conversa_nao_encontrada() });
    } catch (cause) {
      setIssue(serverId, { kind: 'recover_failed', message: m.nova_conversa_conferir_erro({ erro: messageOf(cause) }) });
    }
  });
}

// Ação explícita da pessoa depois de revisar a candidata: adota a sessão como criada.
export function adoptCandidate(serverId: string, attemptId: string): boolean {
  const issue = useNewConversation.getState().issues[serverId];
  const attempt = useNewConversation.getState().attempts[serverId];
  if (issue?.kind !== 'candidate' || attempt?.id !== attemptId) return false;
  let next: FirstConversationAttempt | null;
  try { next = apply(attempt, { type: 'create_ok', sessionName: issue.session.name }); } catch {
    setIssue(serverId, { kind: 'local', message: m.nova_conversa_salvar_erro() });
    return false;
  }
  if (!next) return false;
  setIssue(serverId, null);
  return true;
}

export function abandonUnknownAttempt(serverId: string, attemptId: string): boolean {
  if (inFlight.has(serverId)) return false;
  const attempt = useNewConversation.getState().attempts[serverId] ?? restoreAttempt(serverId);
  if (!attempt || attempt.id !== attemptId || attempt.phase !== 'send_unknown') return false;
  try {
    prefs.remove(KEY(serverId));
  } catch {
    setIssue(serverId, { kind: 'local', message: m.nova_conversa_salvar_erro() });
    return false;
  }
  useNewConversation.setState((s) => {
    const attempts = { ...s.attempts };
    delete attempts[serverId];
    return { attempts, issues: { ...s.issues, [serverId]: null } };
  });
  return true;
}

// Descartar é sempre pedido explícito; nada aqui avisa o servidor nem apaga sessão remota.
export function discardAttempt(serverId: string, attemptId: string): void {
  if (inFlight.has(serverId)) return;
  const attempt = useNewConversation.getState().attempts[serverId];
  if (attempt?.id !== attemptId) return;
  prefs.remove(KEY(serverId));
  useNewConversation.setState((s) => {
    const attempts = { ...s.attempts };
    delete attempts[serverId];
    return { attempts, issues: { ...s.issues, [serverId]: null } };
  });
}

export function _resetNewConversationForTests(): void {
  inFlight.clear();
  refusedNames.clear();
  useNewConversation.setState({ attempts: {}, issues: {}, busy: {} });
}
