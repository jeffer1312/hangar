import { useCallback, useEffect, useRef, useState } from 'react';
import { baseOf, decisionsPayload, firstPendingIndex, initialReason, registrarDiag } from '@hangar/core';
import type { Server, WorkReview, WorkReviewChoice, WorkReviewChoices, WorkReviewDraftBody, WorkReviewResponse } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { haptic } from '../../ui/haptics';
import { fetchWorkReview, isStaleReview, saveWorkReviewDraft, sendWorkReviewDecisions } from './api';
import { useWorkReviewSummary } from './summaries';
import * as m from '../../paraglide/messages';

export type WorkReviewLoad =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; data: WorkReviewResponse };

// Uma gravação das escolhas em voo por sessão. Fica fora do componente: sair da sessão no meio não a interrompe.
// `unsaved` junta as decisões mudadas que o servidor ainda não confirmou: cada PUT leva só elas (o servidor junta
// por decisão, e as de outro aparelho ficam), e a leitura as põe por cima do guardado. Só sai o que o servidor
// confirmou; tentativas esgotadas deixam o resto para a próxima escolha. `epoch` sobe a cada confirmação: leitura
// começada antes dela trouxe o guardado velho e é refeita.
type DraftSlot = { running: boolean; server: Server | null; unsaved: WorkReviewDraftBody | null; epoch: number };
const drafts = new Map<string, DraftSlot>();
// ponytail: rede fora ou servidor em erro tenta de novo a cada 5 s por 1 min; depois só a próxima escolha leva tudo.
const DRAFT_RETRIES = 12;
const DRAFT_RETRY_MS = 5_000;
const draftKey = (serverId: string, name: string) => `${serverId}::${name}`;
const draftSlot = (key: string) => {
  const slot = drafts.get(key) ?? { running: false, server: null, unsaved: null, epoch: 0 };
  drafts.set(key, slot);
  return slot;
};
function saveDraft(name: string, server: Server, reviewId: string, decisionId: string, pick: WorkReviewChoices[string]) {
  const slot = draftSlot(draftKey(server.id, name));
  slot.server = server;
  if (slot.unsaved?.review_id !== reviewId) slot.unsaved = { review_id: reviewId, choices: {} };
  slot.unsaved.choices[decisionId] = pick;
  if (slot.running) return;
  slot.running = true;
  void (async () => {
    let tries = 0;
    while (slot.unsaved && slot.server) {
      const body = { review_id: slot.unsaved.review_id, choices: { ...slot.unsaved.choices } };
      const target = slot.server;
      try {
        await saveWorkReviewDraft(target, name, body);
        tries = 0;
        slot.epoch++;
        const left = slot.unsaved;
        if (left?.review_id === body.review_id) {
          for (const [id, sent] of Object.entries(body.choices)) {
            const now = left.choices[id];
            if (now && now.choice === sent.choice && now.reason === sent.reason) delete left.choices[id];
          }
          if (Object.keys(left.choices).length === 0) slot.unsaved = null;
        }
      } catch (e) {
        const { status, code } = (e ?? {}) as { status?: unknown; code?: unknown };
        registrarDiag({ evento: 'work_review.draft_failed', nivel: 'aviso', tela: 'work_review', codigo: String(code ?? status ?? 'erro') },
          baseOf(target));
        if ((typeof status === 'number' && status < 500) || tries >= DRAFT_RETRIES) break;
        tries++;
        await new Promise((resolve) => setTimeout(resolve, DRAFT_RETRY_MS));
      }
    }
    slot.running = false;
  })();
}

// O servidor responde "NNN: frase"; na seção o número não ajuda quem lê.
export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/^\d{3}: /, '');

/** `enabled` falso não lê: o painel do tablet só lê quando a lista anunciou a revisão ou a aba foi pedida. */
export function useWorkReview(serverId: string, name: string, enabled = true) {
  const server = useServers((s) => s.servers.find((item) => item.id === serverId) ?? null);
  const summary = useWorkReviewSummary(serverId, name);
  const [load, setLoad] = useState<WorkReviewLoad>({ status: 'loading' });
  const [choices, setChoices] = useState<WorkReviewChoices>({});
  const choicesRef = useRef(choices);
  choicesRef.current = choices;
  const [index, setIndex] = useState(0);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const generation = useRef(0);
  const hasData = useRef(false);
  const serverRef = useRef(server);
  serverRef.current = server;

  const reviewId = useRef<string | undefined>(undefined);
  const assetRetried = useRef<string | null>(null);
  const assetLogged = useRef(new Set<string>());
  // O painel do tablet troca de sessão sem remontar: a revisão e as escolhas da anterior não passam para a nova.
  const sessionKey = `${serverId}::${name}`;
  // Muda a cada troca de sessão, inclusive A → B → A: o envio da passagem anterior não vale na seguinte.
  const visit = useRef(0);
  const [loadedFor, setLoadedFor] = useState(sessionKey);
  if (loadedFor !== sessionKey) {
    setLoadedFor(sessionKey);
    setLoad({ status: 'loading' });
    setChoices({});
    setIndex(0);
    setSending(false);
    setSendError(null);
    // Leitura em voo da sessão anterior chega depois e não pode cair nesta.
    generation.current++;
    visit.current++;
    hasData.current = false;
    reviewId.current = undefined;
    assetLogged.current.clear();
  }

  const apply = useCallback((data: WorkReviewResponse) => {
    const next = data.linked ? data.review?.id : undefined;
    const shown = data.linked ? data.review : null;
    const queued = drafts.get(draftKey(serverId, name))?.unsaved;
    const merged = { ...((data.linked && data.draft) || {}), ...(queued && queued.review_id === next ? queued.choices : {}) };
    // Só o que a rodada ainda mostra: decisões de lotes já enviados saem.
    const keep = (c: WorkReviewChoices) => Object.fromEntries(Object.entries(c).filter(([id]) => shown?.decisions.some((d) => d.id === id)));
    const saved = keep(merged);
    // Abre com o guardado e, por cima, o que este aparelho ainda não confirmou; rodada nova vai à primeira que falta.
    // Na mesma rodada o guardado também vence o que já foi confirmado aqui: outro aparelho pode tê-lo mudado.
    if (reviewId.current !== next) {
      reviewId.current = next;
      setChoices(saved);
      setIndex(data.linked && data.review ? firstPendingIndex(data.review, saved) : 0);
    } else setChoices(saved);
    hasData.current = true;
    setLoad({ status: 'ready', data });
  }, [serverId, name]);

  /** `quiet`: releitura em segundo plano; a falha vai ao diário e a tela fica com a última leitura. */
  const reload = useCallback((quiet = false) => {
    const target = serverRef.current;
    const gen = ++generation.current;
    if (!target) { setLoad({ status: 'error', message: m.chat_servidor_removido() }); return; }
    if (!quiet && !hasData.current) setLoad({ status: 'loading' });
    const slot = draftSlot(draftKey(target.id, name));
    const epoch = slot.epoch;
    fetchWorkReview(target, name).then((data) => {
      if (gen !== generation.current) return;
      // Uma gravação confirmou no meio da leitura: o guardado que veio é anterior a ela.
      if (slot.epoch !== epoch) { reload(true); return; }
      apply(data);
    }).catch((e: unknown) => {
      if (gen !== generation.current) return;
      if (quiet && hasData.current) {
        registrarDiag({ evento: 'work_review.reread_failed', nivel: 'aviso', tela: 'work_review',
          codigo: String((e as { code?: unknown } | null)?.code ?? 'erro') }, baseOf(target));
        return;
      }
      setLoad({ status: 'error', message: m.work_review_load_error({ msg: errorText(e) }) });
    });
  }, [apply, name]);

  // Abrir lê; depois, só quando o evento da lista diz que a revisão mudou.
  // Sem leitura ainda, o `quiet` não muda nada: a tela já está em carregando e a falha aparece.
  const summaryKey = summary ? `${summary.review_id}|${summary.state}|${summary.updated_at}` : '';
  useEffect(() => {
    if (enabled) reload(true);
  }, [enabled, summaryKey, reload, sessionKey]);
  useEffect(() => () => { generation.current++; }, []);

  const review: WorkReview | null = load.status === 'ready' && load.data.linked ? load.data.review : null;

  // Cada escolha vai ao servidor na hora: sair, trocar de aba ou fechar o app não a perde.
  const update = useCallback((next: WorkReviewChoices, decisionId: string) => {
    choicesRef.current = next;
    setChoices(next);
    const target = serverRef.current;
    if (target && reviewId.current) saveDraft(name, target, reviewId.current, decisionId, next[decisionId]);
  }, [name]);

  const pick = useCallback((decisionId: string, choice: WorkReviewChoice) => {
    haptic.selection();
    setSendError(null);
    const prev = choicesRef.current;
    update({
      ...prev,
      [decisionId]: { choice: choice.id, reason: prev[decisionId]?.choice === choice.id ? prev[decisionId].reason : initialReason(choice) },
    }, decisionId);
  }, [update]);

  const setReason = useCallback((decisionId: string, reason: string) => {
    const prev = choicesRef.current;
    if (prev[decisionId]) update({ ...prev, [decisionId]: { ...prev[decisionId], reason } }, decisionId);
  }, [update]);

  const send = useCallback(() => {
    const target = serverRef.current;
    const body = review && decisionsPayload(review, choices);
    if (!target || !body || sending) return;
    // Resposta que chega com outra sessão no painel é descartada: load e reload aqui são os da que enviou.
    const sentIn = visit.current;
    const current = () => visit.current === sentIn;
    setSending(true);
    setSendError(null);
    sendWorkReviewDecisions(target, name, body).then((res) => {
      if (!current()) return;
      haptic.success();
      if (res.review && load.status === 'ready' && load.data.linked) apply({ ...load.data, review: res.review });
      reload(true);
    }).catch((e: unknown) => {
      if (!current()) return;
      haptic.error();
      const stale = isStaleReview(e);
      setSendError(stale ? m.work_review_stale() : m.work_review_send_error({ msg: errorText(e) }));
      if (stale) reload(true);
    }).finally(() => { if (current()) setSending(false); });
  }, [apply, choices, load, name, reload, review, sending]);

  // Imagem recusada pode ser rodada nova (409): relê uma vez por rodada, nunca em laço.
  // Cada falha vai ao diário uma vez antes e uma depois da releitura, para separar rodada trocada de imagem quebrada.
  const onAssetError = useCallback((asset: string) => {
    if (!review) return;
    const retried = assetRetried.current === review.id;
    const key = `${review.id}|${asset}|${retried}`;
    if (!assetLogged.current.has(key)) {
      assetLogged.current.add(key);
      const target = serverRef.current;
      registrarDiag({ evento: 'work_review.asset_failed', nivel: 'aviso', tela: 'work_review',
        codigo: retried ? 'asset_failed_after_retry' : 'asset_failed', detalhe: `review=${review.id} asset=${asset}` },
      target ? baseOf(target) : undefined);
    }
    if (retried) return;
    assetRetried.current = review.id;
    reload(true);
  }, [reload, review]);

  const count = review?.decisions.length ?? 0;
  const safeIndex = Math.min(index, Math.max(0, count - 1));

  return { server, summary, load, review, choices, index: safeIndex, setIndex, pick, setReason, send, sending, sendError,
    reload: () => reload(), onAssetError };
}

export type WorkReviewController = ReturnType<typeof useWorkReview>;
