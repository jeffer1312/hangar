import { useCallback, useEffect, useRef, useState } from 'react';
import { baseOf, decisionsPayload, initialReason, registrarDiag } from '@hangar/core';
import type { WorkReview, WorkReviewChoice, WorkReviewChoices, WorkReviewResponse } from '@hangar/core';
import { useServers } from '../../stores/servers';
import { haptic } from '../../ui/haptics';
import { fetchWorkReview, isStaleReview, sendWorkReviewDecisions } from './api';
import { useWorkReviewSummary } from './summaries';
import * as m from '../../paraglide/messages';

export type WorkReviewLoad =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ready'; data: WorkReviewResponse };

// O servidor responde "NNN: frase"; na seção o número não ajuda quem lê.
export const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/^\d{3}: /, '');

export function useWorkReview(serverId: string, name: string) {
  const server = useServers((s) => s.servers.find((item) => item.id === serverId) ?? null);
  const summary = useWorkReviewSummary(serverId, name);
  const [load, setLoad] = useState<WorkReviewLoad>({ status: 'loading' });
  const [choices, setChoices] = useState<WorkReviewChoices>({});
  const [index, setIndex] = useState(0);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const generation = useRef(0);
  const hasData = useRef(false);
  const serverRef = useRef(server);
  serverRef.current = server;

  const reviewId = useRef<string | undefined>(undefined);
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
  }

  const apply = useCallback((data: WorkReviewResponse) => {
    const next = data.linked ? data.review?.id : undefined;
    // Rodada nova começa do zero; a mesma rodada (lotes) conserva o que a pessoa já escolheu.
    if (reviewId.current !== next) { reviewId.current = next; setChoices({}); setIndex(0); }
    hasData.current = true;
    setLoad({ status: 'ready', data });
  }, []);

  /** `quiet`: releitura em segundo plano; a falha vai ao diário e a tela fica com a última leitura. */
  const reload = useCallback((quiet = false) => {
    const target = serverRef.current;
    const gen = ++generation.current;
    if (!target) { setLoad({ status: 'error', message: m.chat_servidor_removido() }); return; }
    if (!quiet && !hasData.current) setLoad({ status: 'loading' });
    fetchWorkReview(target, name).then((data) => {
      if (gen === generation.current) apply(data);
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
  const summaryKey = summary ? `${summary.review_id}|${summary.state}|${summary.updated_at}` : '';
  const first = useRef(true);
  useEffect(() => {
    if (first.current) { first.current = false; reload(); return; }
    reload(true);
  }, [summaryKey, reload, sessionKey]);
  useEffect(() => () => { generation.current++; }, []);

  const review: WorkReview | null = load.status === 'ready' && load.data.linked ? load.data.review : null;

  const pick = useCallback((decisionId: string, choice: WorkReviewChoice) => {
    haptic.selection();
    setSendError(null);
    setChoices((prev) => ({
      ...prev,
      [decisionId]: { choice: choice.id, reason: prev[decisionId]?.choice === choice.id ? prev[decisionId].reason : initialReason(choice) },
    }));
  }, []);

  const setReason = useCallback((decisionId: string, reason: string) => {
    setChoices((prev) => (prev[decisionId] ? { ...prev, [decisionId]: { ...prev[decisionId], reason } } : prev));
  }, []);

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
  const assetRetried = useRef<string | null>(null);
  const onAssetError = useCallback(() => {
    if (!review || assetRetried.current === review.id) return;
    assetRetried.current = review.id;
    reload(true);
  }, [reload, review]);

  const count = review?.decisions.length ?? 0;
  const safeIndex = Math.min(index, Math.max(0, count - 1));

  return { server, summary, load, review, choices, index: safeIndex, setIndex, pick, setReason, send, sending, sendError,
    reload: () => reload(), onAssetError };
}

export type WorkReviewController = ReturnType<typeof useWorkReview>;
