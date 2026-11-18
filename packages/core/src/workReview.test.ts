import { describe, expect, it } from 'vitest';
import {
  decisionsPayload, frameZoom, initialReason, parseWorkReviewDecideResponse, parseWorkReviewResponse, parseWorkReviewsEvent,
  pendingCount, readyToSend, stateTone, stepStatus, waitingHeadlineText, type WorkReview, type WorkReviewChoices,
} from './workReview';

// Revisão do desenho aprovado: comparação com o mock, esperando duas decisões.
const waitingRaw = () => ({
  id: 'r-7', title: 'Comparação com o mock', subject: 'Controle Bancário', updated_at: '2026-11-18T09:20:00Z',
  state: 'waiting', extra_from_connector: true,
  steps: { labels: ['Prints', 'Comparação', 'Correções', 'Teste', 'Sua decisão', 'Pronto'], current: 4 },
  headline: { value: '2', text: 'diferenças esperam a sua decisão', detail: 'O agente já corrigiu 15 sozinho.' },
  badge: { text: 'mock: 2 para você', tone: 'warning' },
  decisions: [
    {
      id: '47', tag: 'Diferença 47', title: 'Janela da Movimentação menor que no mock',
      reason: 'padrão do kit', reason_label: 'Por que o agente não igualou',
      left: { label: 'Mock', value: '~1110 px', asset: 'a1', width: 1600, height: 900, box: [385, 263, 830, 372] },
      right: { label: 'Tela', value: '~830 px', asset: 'a2', width: 1600, height: 900, box: [385, 263, 830, 372] },
      choices: [
        { id: 'igualar', label: 'Igualar ao mock', hint: 'o agente corrige a tela' },
        { id: 'manter', label: 'Manter como está', requires_reason: true, default_reason: 'padrão do kit: AreaComponente',
          reason_prompt: 'Por que manter? Vai para o relatório e para a PM' },
      ],
    },
    {
      id: '86', title: 'Título da impressão do saldo projetado',
      left: { label: 'Mock', asset: 'a3', width: 1600, height: 900, box: [476, 146, 398, 14] },
      choices: [
        { id: 'igualar', label: 'Igualar ao mock' },
        { id: 'manter', label: 'Manter como está', requires_reason: true },
      ],
    },
  ],
  resolved: { label: 'Corrigidas pelo agente', count: 15, items: [{ title: 'Cabeçalho do mock', before: 'a4', after: 'a5' }] },
  kept: { label: 'Mantidas pelo agente, com motivo', items: [{ title: 'Selo Open Finance', reason: 'dado sem fonte no backend' }] },
  links: [{ label: 'Relatório completo', url: 'http://127.0.0.1:47613/c/x/', local: true }],
});
const linked = (review: unknown) => ({
  linked: true, via: 'ticket', connector: 'help-example', ticket_id: 'ABC-123', key: 'ABC-123',
  supported: true, review, fetched_at: '2026-11-18T09:21:00Z',
});
const parsedWaiting = (): WorkReview => {
  const res = parseWorkReviewResponse(linked(waitingRaw()));
  if (!res || !res.linked || !res.review) throw new Error('revisão de exemplo não leu');
  return res.review;
};

describe('parseWorkReviewResponse', () => {
  it('sessão sem vínculo', () => {
    expect(parseWorkReviewResponse({ linked: false })).toEqual({ linked: false });
  });

  it('lê a revisão inteira, larga campo desconhecido e guarda os opcionais', () => {
    const review = parsedWaiting();
    expect(review).not.toHaveProperty('extra_from_connector');
    expect(review.state).toBe('waiting');
    expect(review.steps).toEqual({ labels: ['Prints', 'Comparação', 'Correções', 'Teste', 'Sua decisão', 'Pronto'], current: 4 });
    expect(review.decisions[0]).toMatchObject({ tag: 'Diferença 47', reason_label: 'Por que o agente não igualou' });
    expect(review.decisions[0].right?.box).toEqual([385, 263, 830, 372]);
    expect(review.decisions[0].choices[1]).toMatchObject({ requires_reason: true, reason_prompt: 'Por que manter? Vai para o relatório e para a PM' });
    // Sem `right`, o painel mostra só o lado esquerdo.
    expect(review.decisions[1].right).toBeUndefined();
    expect(review.resolved).toEqual({ label: 'Corrigidas pelo agente', count: 15, items: [{ title: 'Cabeçalho do mock', before: 'a4', after: 'a5' }] });
    expect(review.kept?.count).toBeUndefined();
    expect(review.links).toEqual([{ label: 'Relatório completo', url: 'http://127.0.0.1:47613/c/x/', local: true }]);
  });

  it('vínculo pela branch e revisão nula', () => {
    expect(parseWorkReviewResponse({ ...linked(null), via: 'branch', supported: false })).toEqual({
      linked: true, via: 'branch', connector: 'help-example', ticket_id: 'ABC-123', key: 'ABC-123',
      supported: false, review: null, fetched_at: '2026-11-18T09:21:00Z',
    });
  });

  it('resposta torta não vira revisão', () => {
    expect(parseWorkReviewResponse('x')).toBeNull();
    expect(parseWorkReviewResponse({ linked: true })).toBeNull();
    expect(parseWorkReviewResponse({ ...linked(null), via: 'outro' })).toBeNull();
    expect(parseWorkReviewResponse(linked({ ...waitingRaw(), state: 'paused' }))).toBeNull();
    const badBox = waitingRaw();
    badBox.decisions[0].left!.box = [1, 2, 3] as unknown as [number, number, number, number];
    expect(parseWorkReviewResponse(linked(badBox))).toBeNull();
    for (const size of [{ width: 0 }, { height: -900 }]) {
      const badSize = waitingRaw();
      Object.assign(badSize.decisions[0].right!, size);
      expect(parseWorkReviewResponse(linked(badSize))).toBeNull();
    }
    const noPrint = waitingRaw();
    delete (noPrint.decisions[1] as { left?: unknown }).left;
    expect(parseWorkReviewResponse(linked(noPrint))?.linked).toBe(true);
    const noChoices = waitingRaw();
    noChoices.decisions[1].choices = 'manter' as never;
    expect(parseWorkReviewResponse(linked(noChoices))).toBeNull();
  });
});

describe('parseWorkReviewDecideResponse', () => {
  it('ok com a revisão relida', () => {
    expect(parseWorkReviewDecideResponse({ ok: true, review: null })).toEqual({ ok: true, review: null });
    expect(parseWorkReviewDecideResponse({ ok: true, review: { ...waitingRaw(), state: 'applying', decisions: [] } })?.review?.state).toBe('applying');
    expect(parseWorkReviewDecideResponse({ ok: false, review: null })).toBeNull();
  });
});

describe('parseWorkReviewsEvent', () => {
  it('lê o resumo por sessão e larga a linha torta', () => {
    expect(parseWorkReviewsEvent('{')).toBeNull();
    expect(parseWorkReviewsEvent(JSON.stringify({ reviews: [] }))).toBeNull();
    expect(parseWorkReviewsEvent(JSON.stringify({ reviews: {
      'ABC-123': { review_id: 'r-7', state: 'waiting', badge: { text: 'mock: 2 para você', tone: 'warning' }, updated_at: '2026-11-18T09:20:00Z', x: 1 },
      outra: { review_id: 'r-1', state: 'running', badge: null, updated_at: '2026-11-18T09:00:00Z' },
      torta: { review_id: 'r-2', state: 'sleeping', badge: null, updated_at: '2026-11-18T09:00:00Z' },
      cor: { review_id: 'r-3', state: 'done', badge: { text: 'ok', tone: 'red' }, updated_at: '2026-11-18T09:00:00Z' },
    } }))).toEqual({ reviews: {
      'ABC-123': { review_id: 'r-7', state: 'waiting', badge: { text: 'mock: 2 para você', tone: 'warning' }, updated_at: '2026-11-18T09:20:00Z' },
      outra: { review_id: 'r-1', state: 'running', badge: null, updated_at: '2026-11-18T09:00:00Z' },
    } });
  });
});

describe('decisões pendentes', () => {
  it('conta sem escolha e manter sem motivo; só envia com tudo decidido', () => {
    const review = parsedWaiting();
    const choices: WorkReviewChoices = {};
    expect(pendingCount(review, choices)).toBe(2);
    expect(readyToSend(review, choices)).toBe(false);
    choices['47'] = { choice: 'igualar', reason: '' };
    expect(pendingCount(review, choices)).toBe(1);
    choices['86'] = { choice: 'manter', reason: '   ' };
    expect(pendingCount(review, choices)).toBe(1);
    choices['86'] = { choice: 'manter', reason: 'padrão do kit' };
    expect(pendingCount(review, choices)).toBe(0);
    expect(readyToSend(review, choices)).toBe(true);
  });

  it('a faixa usa o texto do conector só enquanto o número dele é o pendente', () => {
    const headline = { value: '3', text: 'diferenças esperam a sua decisão' };
    expect(waitingHeadlineText(headline, 3)).toBe('connector');
    expect(waitingHeadlineText(headline, 1)).toBe('waiting_one');
    expect(waitingHeadlineText(headline, 2)).toBe('waiting');
    expect(waitingHeadlineText(undefined, 2)).toBe('waiting');
    expect(waitingHeadlineText(headline, 0)).toBe('ready');
  });

  it('escolha que não existe na decisão continua pendente', () => {
    const review = parsedWaiting();
    expect(pendingCount(review, { '47': { choice: 'apagar', reason: '' }, '86': { choice: 'igualar', reason: '' } })).toBe(1);
  });

  it('fora de waiting nada vai ao agente', () => {
    const review = { ...parsedWaiting(), state: 'applying' as const };
    const all = { '47': { choice: 'igualar', reason: '' }, '86': { choice: 'igualar', reason: '' } };
    expect(readyToSend(review, all)).toBe(false);
    expect(decisionsPayload(review, all)).toBeNull();
  });

  it('motivo nasce do default_reason', () => {
    const [igualar, manter] = parsedWaiting().decisions[0].choices;
    expect(initialReason(manter)).toBe('padrão do kit: AreaComponente');
    expect(initialReason(igualar)).toBe('');
  });

  it('corpo do POST leva todas as decisões e o motivo só onde é exigido', () => {
    const review = parsedWaiting();
    expect(decisionsPayload(review, { '47': { choice: 'igualar', reason: 'sobra' } })).toBeNull();
    expect(decisionsPayload(review, {
      '47': { choice: 'igualar', reason: 'sobra' }, '86': { choice: 'manter', reason: '  padrão do kit  ' },
    })).toEqual({ review_id: 'r-7', decisions: [{ id: '47', choice: 'igualar' }, { id: '86', choice: 'manter', reason: 'padrão do kit' }] });
  });
});

describe('cores e trilha', () => {
  it('cor sai do estado', () => {
    expect(stateTone('running')).toBe('accent');
    expect(stateTone('waiting')).toBe('warning');
    expect(stateTone('applying')).toBe('success');
    expect(stateTone('done')).toBe('success');
  });

  const steps = { labels: ['Prints', 'Comparação', 'Correções', 'Teste', 'Sua decisão', 'Pronto'], current: 4 };
  it('esperando você: feitas em verde, a atual em âmbar parada', () => {
    expect(stepStatus(steps, 'waiting', 3)).toEqual({ status: 'done', tone: 'success', pulse: false });
    expect(stepStatus(steps, 'waiting', 4)).toEqual({ status: 'current', tone: 'warning', pulse: false });
    expect(stepStatus(steps, 'waiting', 5)).toEqual({ status: 'pending', tone: null, pulse: false });
  });

  it('rodando: a atual em roxo pulsando', () => {
    expect(stepStatus({ ...steps, current: 0 }, 'running', 0)).toEqual({ status: 'current', tone: 'accent', pulse: true });
    expect(stepStatus({ ...steps, current: 0 }, 'running', 1)).toEqual({ status: 'pending', tone: null, pulse: false });
  });

  it('current igual ao total ou revisão concluída: todas feitas', () => {
    for (const index of [0, 5]) {
      expect(stepStatus({ ...steps, current: 6 }, 'applying', index)).toEqual({ status: 'done', tone: 'success', pulse: false });
      expect(stepStatus(steps, 'done', index)).toEqual({ status: 'done', tone: 'success', pulse: false });
    }
  });
});

describe('zoom no trecho', () => {
  const side = { width: 1600, height: 900 };
  // Números do desenho aprovado (`enquadrar`): quadro de 300×150, trecho da diferença 47.
  it('o trecho cabe no quadro com folga e ganha a moldura em volta', () => {
    const z = frameZoom({ width: 300, height: 150 }, { ...side, box: [385, 263, 830, 372] });
    expect(z.scale).toBeCloseTo(0.30303, 4);
    expect(z.imageWidth).toBeCloseTo(484.85, 1);
    expect(z.imageHeight).toBeCloseTo(272.73, 1);
    expect(z.left).toBeCloseTo(-92.42, 1);
    expect(z.top).toBeCloseTo(-61.06, 1);
    expect(z.mark?.left).toBeCloseTo(21.24, 1);
    expect(z.mark?.top).toBeCloseTo(15.64, 1);
    expect(z.mark?.width).toBeCloseTo(257.52, 1);
    expect(z.mark?.height).toBeCloseTo(118.73, 1);
  });

  it('trecho pequeno não passa de 3×', () => {
    expect(frameZoom({ width: 1000, height: 1000 }, { ...side, box: [10, 10, 10, 10] }).scale).toBe(3);
  });

  it('sem trecho a imagem inteira cabe centrada, sem moldura', () => {
    const z = frameZoom({ width: 300, height: 150 }, side);
    expect(z.scale).toBeCloseTo(0.16667, 4);
    expect(z.imageWidth).toBeCloseTo(266.67, 1);
    expect(z.imageHeight).toBeCloseTo(150, 1);
    expect(z.left).toBeCloseTo(16.67, 1);
    expect(z.top).toBeCloseTo(0, 4);
    expect(z.mark).toBeNull();
  });
});
