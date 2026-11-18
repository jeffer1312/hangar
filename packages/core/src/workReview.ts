// Revisão do trabalho (`work_review/v1`) ligada à sessão. Os textos de fornecedor vêm do conector;
// aqui ficam só a forma da resposta do servidor e as contas que os clientes repetiriam.

export type WorkReviewState = 'running' | 'waiting' | 'applying' | 'done';
export type WorkReviewBadgeTone = 'warning' | 'success' | 'info';
export type WorkReviewTone = 'accent' | 'warning' | 'success';

export interface WorkReviewBadge { text: string; tone: WorkReviewBadgeTone }
export interface WorkReviewSteps { labels: string[]; current: number }
export interface WorkReviewHeadline { value: string; text: string; detail?: string }
export interface WorkReviewProgress { label: string; done: number; total: number }
/** `box` em px do asset: x, y, largura, altura. */
export interface WorkReviewSide {
  label: string; value?: string; asset: string; width: number; height: number;
  box?: [number, number, number, number];
}
export interface WorkReviewChoice {
  id: string; label: string; hint?: string; requires_reason?: boolean; default_reason?: string; reason_prompt?: string;
}
export interface WorkReviewDecision {
  id: string; tag?: string; title: string; reason?: string; reason_label?: string;
  left?: WorkReviewSide; right?: WorkReviewSide; choices: WorkReviewChoice[];
}
export interface WorkReviewResolvedItem { title: string; before?: string; after?: string }
export interface WorkReviewResolved { label: string; count?: number; items: WorkReviewResolvedItem[] }
export interface WorkReviewKeptItem { title: string; reason: string }
export interface WorkReviewKept { label: string; count?: number; items: WorkReviewKeptItem[] }
export interface WorkReviewLink { label: string; url: string; local?: boolean }
export interface WorkReview {
  id: string; title: string; subject?: string; updated_at: string; state: WorkReviewState;
  steps?: WorkReviewSteps; headline?: WorkReviewHeadline; progress?: WorkReviewProgress; badge?: WorkReviewBadge;
  decisions: WorkReviewDecision[]; resolved?: WorkReviewResolved; kept?: WorkReviewKept; links: WorkReviewLink[];
}

/** `GET /api/sessions/{nome}/work-review`. */
export type WorkReviewResponse =
  | { linked: false }
  | {
    linked: true; via: 'ticket' | 'branch'; connector: string; ticket_id: string; key: string | null;
    supported: boolean; review: WorkReview | null; fetched_at: string | null;
  };
/** `POST /api/sessions/{nome}/work-review/decisions`. */
export interface WorkReviewDecideResponse { ok: true; review: WorkReview | null }

/** Uma linha do evento `work_reviews` da lista do dono. */
export interface WorkReviewSummary { review_id: string; state: WorkReviewState; badge: WorkReviewBadge | null; updated_at: string }
export interface WorkReviewsEvent { reviews: Record<string, WorkReviewSummary> }

/** O que a pessoa escolheu, por id da decisão. */
export type WorkReviewChoices = Record<string, { choice: string; reason: string }>;
export interface WorkReviewDecisionsBody { review_id: string; decisions: { id: string; choice: string; reason?: string }[] }

export type WorkReviewStepStatus = 'done' | 'current' | 'pending';
/** `tone` nulo = etapa por fazer; `pulse` só na etapa atual de uma revisão rodando. */
export interface WorkReviewStep { status: WorkReviewStepStatus; tone: WorkReviewTone | null; pulse: boolean }

const STATES: readonly WorkReviewState[] = ['running', 'waiting', 'applying', 'done'];
const BADGE_TONES: readonly WorkReviewBadgeTone[] = ['warning', 'success', 'info'];

// Item torto derruba a revisão inteira: largar uma decisão faria o envio parecer completo e o servidor recusar.
class Bad extends Error {}
const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v);
const obj = (v: unknown): Record<string, unknown> => { if (!isObject(v)) throw new Bad(); return v; };
const str = (v: unknown): string => { if (typeof v !== 'string') throw new Bad(); return v; };
const num = (v: unknown): number => { if (typeof v !== 'number' || !Number.isFinite(v)) throw new Bad(); return v; };
// Tamanho de imagem: zero ou negativo dá escala infinita no zoom e proporção quebrada na cortina.
const positive = (v: unknown): number => { const n = num(v); if (n <= 0) throw new Bad(); return n; };
const bool = (v: unknown): boolean => { if (typeof v !== 'boolean') throw new Bad(); return v; };
const list = <T>(v: unknown, item: (x: unknown) => T): T[] => { if (!Array.isArray(v)) throw new Bad(); return v.map(item); };
const oneOf = <T extends string>(v: unknown, values: readonly T[]): T => {
  if (!(values as readonly unknown[]).includes(v)) throw new Bad();
  return v as T;
};
/** Ausente ou `null` = campo fora; presente, tem de ter a forma. */
const opt = <T>(v: unknown, read: (x: unknown) => T): T | undefined => (v == null ? undefined : read(v));
// Sem chave para o opcional ausente: `toEqual` e o memo dos clientes comparam a forma.
const clean = <T extends object>(o: T): T => {
  for (const k of Object.keys(o) as (keyof T)[]) if (o[k] === undefined) delete o[k];
  return o;
};

const badge = (v: unknown): WorkReviewBadge => { const o = obj(v); return { text: str(o.text), tone: oneOf(o.tone, BADGE_TONES) }; };
const side = (v: unknown): WorkReviewSide => {
  const o = obj(v);
  const box = opt(o.box, (b) => list(b, num));
  if (box && box.length !== 4) throw new Bad();
  return clean({ label: str(o.label), value: opt(o.value, str), asset: str(o.asset), width: positive(o.width), height: positive(o.height),
    box: box as [number, number, number, number] | undefined });
};
const choice = (v: unknown): WorkReviewChoice => {
  const o = obj(v);
  return clean({ id: str(o.id), label: str(o.label), hint: opt(o.hint, str), requires_reason: opt(o.requires_reason, bool),
    default_reason: opt(o.default_reason, str), reason_prompt: opt(o.reason_prompt, str) });
};
const decision = (v: unknown): WorkReviewDecision => {
  const o = obj(v);
  return clean({ id: str(o.id), tag: opt(o.tag, str), title: str(o.title), reason: opt(o.reason, str),
    reason_label: opt(o.reason_label, str), left: opt(o.left, side), right: opt(o.right, side), choices: list(o.choices, choice) });
};
const resolvedItem = (v: unknown): WorkReviewResolvedItem => {
  const o = obj(v); return clean({ title: str(o.title), before: opt(o.before, str), after: opt(o.after, str) });
};
const keptItem = (v: unknown): WorkReviewKeptItem => { const o = obj(v); return { title: str(o.title), reason: str(o.reason) }; };
const group = <I>(v: unknown, item: (x: unknown) => I): { label: string; count?: number; items: I[] } => {
  const o = obj(v); return clean({ label: str(o.label), count: opt(o.count, num), items: list(o.items, item) });
};
const link = (v: unknown): WorkReviewLink => {
  const o = obj(v); return clean({ label: str(o.label), url: str(o.url), local: opt(o.local, bool) });
};
const review = (v: unknown): WorkReview => {
  const o = obj(v);
  return clean({
    id: str(o.id), title: str(o.title), subject: opt(o.subject, str), updated_at: str(o.updated_at),
    state: oneOf(o.state, STATES),
    steps: opt(o.steps, (s) => { const x = obj(s); return { labels: list(x.labels, str), current: num(x.current) }; }),
    headline: opt(o.headline, (h) => { const x = obj(h); return clean({ value: str(x.value), text: str(x.text), detail: opt(x.detail, str) }); }),
    progress: opt(o.progress, (p) => { const x = obj(p); return { label: str(x.label), done: num(x.done), total: num(x.total) }; }),
    badge: opt(o.badge, badge),
    decisions: opt(o.decisions, (d) => list(d, decision)) ?? [],
    resolved: opt(o.resolved, (r) => group(r, resolvedItem)),
    kept: opt(o.kept, (k) => group(k, keptItem)),
    links: opt(o.links, (l) => list(l, link)) ?? [],
  });
};
const attempt = <T>(read: () => T): T | null => {
  try { return read(); } catch (e) { if (e instanceof Bad) return null; throw e; }
};

/** Revisão como o servidor manda; forma errada = `null`. */
export function parseWorkReview(raw: unknown): WorkReview | null { return attempt(() => review(raw)); }

export function parseWorkReviewResponse(raw: unknown): WorkReviewResponse | null {
  return attempt((): WorkReviewResponse => {
    const o = obj(raw);
    if (o.linked === false) return { linked: false };
    if (o.linked !== true) throw new Bad();
    return {
      linked: true, via: oneOf(o.via, ['ticket', 'branch'] as const), connector: str(o.connector), ticket_id: str(o.ticket_id),
      key: o.key == null ? null : str(o.key), supported: bool(o.supported),
      review: o.review == null ? null : review(o.review), fetched_at: o.fetched_at == null ? null : str(o.fetched_at),
    };
  });
}

export function parseWorkReviewDecideResponse(raw: unknown): WorkReviewDecideResponse | null {
  return attempt((): WorkReviewDecideResponse => {
    const o = obj(raw);
    if (o.ok !== true) throw new Bad();
    return { ok: true, review: o.review == null ? null : review(o.review) };
  });
}

/** Lê o `data` do evento; linha torta sai sozinha, como nas etiquetas. */
export function parseWorkReviewsEvent(data: string): WorkReviewsEvent | null {
  let raw: unknown;
  try { raw = JSON.parse(data); } catch { return null; }
  if (!isObject(raw) || !isObject(raw.reviews)) return null;
  const reviews: Record<string, WorkReviewSummary> = {};
  for (const [name, row] of Object.entries(raw.reviews)) {
    const summary = attempt((): WorkReviewSummary => {
      const o = obj(row);
      return { review_id: str(o.review_id), state: oneOf(o.state, STATES), badge: o.badge == null ? null : badge(o.badge),
        updated_at: str(o.updated_at) };
    });
    if (summary) reviews[name] = summary;
  }
  return { reviews };
}

function isDecided(decision: WorkReviewDecision, picked: { choice: string; reason: string } | undefined): boolean {
  const option = picked && decision.choices.find((c) => c.id === picked.choice);
  return !!option && (!option.requires_reason || picked.reason.trim() !== '');
}

/** Decisões sem escolha válida ou com motivo exigido em branco. */
export function pendingCount(review: WorkReview, choices: WorkReviewChoices): number {
  return review.decisions.filter((d) => !isDecided(d, choices[d.id])).length;
}

/** Qual texto a faixa usa enquanto espera decisões; o cliente traduz `ready`, `waiting_one` e `waiting`. */
export type WaitingHeadlineText = 'ready' | 'connector' | 'waiting_one' | 'waiting';

// O texto do conector conta o número dele: só vale enquanto esse número é o pendente de agora.
export function waitingHeadlineText(headline: WorkReviewHeadline | undefined, pending: number): WaitingHeadlineText {
  if (pending === 0) return 'ready';
  if (headline && headline.value === String(pending)) return 'connector';
  return pending === 1 ? 'waiting_one' : 'waiting';
}

/** O servidor recebe todas as pendentes de uma vez, e só enquanto a revisão espera. */
export function readyToSend(review: WorkReview, choices: WorkReviewChoices): boolean {
  return review.state === 'waiting' && review.decisions.length > 0 && pendingCount(review, choices) === 0;
}

export function initialReason(choice: WorkReviewChoice): string { return choice.default_reason ?? ''; }

export function stateTone(state: WorkReviewState): WorkReviewTone {
  if (state === 'running') return 'accent';
  if (state === 'waiting') return 'warning';
  return 'success';
}

export function stepStatus(steps: WorkReviewSteps, state: WorkReviewState, index: number): WorkReviewStep {
  // Concluída fica toda verde mesmo que o conector não avance o `current` até o fim.
  if (state === 'done' || index < steps.current) return { status: 'done', tone: 'success', pulse: false };
  if (index === steps.current) return { status: 'current', tone: stateTone(state), pulse: state === 'running' };
  return { status: 'pending', tone: null, pulse: false };
}

export function decisionsPayload(review: WorkReview, choices: WorkReviewChoices): WorkReviewDecisionsBody | null {
  if (!readyToSend(review, choices)) return null;
  return {
    review_id: review.id,
    decisions: review.decisions.map((d) => {
      const { choice, reason } = choices[d.id];
      const option = d.choices.find((c) => c.id === choice);
      return option?.requires_reason ? { id: d.id, choice, reason: reason.trim() } : { id: d.id, choice };
    }),
  };
}

/** Posição da imagem e da moldura dentro do quadro, em pontos do quadro. */
export interface WorkReviewFrameZoom {
  scale: number; imageWidth: number; imageHeight: number; left: number; top: number;
  mark: { left: number; top: number; width: number; height: number } | null;
}

// O trecho marcado cabe no quadro com folga em volta (até 3×); sem trecho, a imagem inteira cabe centrada.
export function frameZoom(frame: { width: number; height: number }, side: Pick<WorkReviewSide, 'width' | 'height' | 'box'>): WorkReviewFrameZoom {
  const { width: W, height: H } = frame;
  const box = side.box;
  const scale = box ? Math.min(W / (box[2] + 160), H / (box[3] + 120), 3) : Math.min(W / side.width, H / side.height);
  const [cx, cy] = box ? [box[0] + box[2] / 2, box[1] + box[3] / 2] : [side.width / 2, side.height / 2];
  const x = cx * scale - W / 2;
  const y = cy * scale - H / 2;
  return {
    scale, imageWidth: side.width * scale, imageHeight: side.height * scale, left: -x, top: -y,
    mark: box ? { left: box[0] * scale - x - 3, top: box[1] * scale - y - 3, width: box[2] * scale + 6, height: box[3] * scale + 6 } : null,
  };
}
