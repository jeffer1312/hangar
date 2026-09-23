// Falha de rede suspende as tentativas por um prazo persistido; resposta confirma a recuperação.
import { registrar as registrarDiag } from './diag';

const CHAVE = 'hangar_servidores_desligados';
const CHAVE_RESPOSTAS = 'hangar_servidores_responderam';

type Estado = { failures: number; retryAt: number };
// As duas primeiras esperas são curtas: queda isolada (backend reiniciando) volta em segundos,
// e só quem falha três vezes seguidas passa a esperar de verdade.
const RETRY_DELAYS_MS = [2_000, 5_000, 30_000, 60_000, 120_000, 240_000, 300_000, 600_000, 1_800_000];
// Máquina que respondeu há menos disto está ligada: a falha dela vem do aparelho (o iOS mata o
// socket na suspensão, a VPN demora a acordar), então a espera para em 30 s.
const RESPONDEU_RECENTE_MS = 24 * 60 * 60_000;
const TETO_RECENTE = RETRY_DELAYS_MS.indexOf(30_000) + 1;

/** Id do teste de "Testar e adicionar": alvo digitado na hora, nunca herda nem deixa marca. */
export const SERVIDOR_CANDIDATO = 'candidato';

const estados = new Map<string, Estado>();
const respostas = new Map<string, number>();
let carregado = false;
let avisouArmazem = false;
const recoveredListeners = new Set<(id: string) => void>();

export function onServerRecovered(listener: (id: string) => void): () => void {
  recoveredListeners.add(listener);
  return () => { recoveredListeners.delete(listener); };
}
// A página saindo protege também os fetches cancelados pela navegação.
let protegido: (id: string) => boolean = () => false;

/** Permite ignorar falhas causadas pelo encerramento da página. */
export function definirProtegido(fn: (id: string) => boolean): void {
  protegido = fn;
}

/** O pedaço de `Storage` que este módulo usa. O core não toca DOM: quem tem `localStorage` (o web)
 *  entrega por `definirArmazem`; no app nativo e nos testes o estado é só de memória. */
export interface ArmazemEsfriamento {
  getItem(chave: string): string | null;
  setItem(chave: string, valor: string): void;
  removeItem(chave: string): void;
}
let armazemAtual: ArmazemEsfriamento | null = null;

export function definirArmazem(a: ArmazemEsfriamento | null): void {
  armazemAtual = a;
  carregado = false;   // armazém novo, estado gravado novo
  // Marca feita ANTES da injeção (só em memória) não pode se perder: funde com o que está
  // gravado e persiste — `registrarFalha` só grava na transição, não gravaria de novo.
  if (estados.size > 0 || respostas.size > 0) {
    carregar();
    gravar();
    gravarRespostas();
  }
}

function armazem(): ArmazemEsfriamento | null {
  return armazemAtual;
}

function carregarRespostas(): void {
  let bruto: string | null | undefined;
  try {
    bruto = armazem()?.getItem(CHAVE_RESPOSTAS);
  } catch (e) {
    avisarSemArmazem(e);
    return;
  }
  if (!bruto) return;
  try {
    const saved: unknown = JSON.parse(bruto);
    if (!saved || typeof saved !== 'object') return;
    for (const [id, em] of Object.entries(saved)) {
      if (typeof em === 'number' && Number.isFinite(em) && em > (respostas.get(id) ?? 0)) respostas.set(id, em);
    }
  } catch (e) {
    registrarDiag({ evento: 'esfriamento.estado_invalido', nivel: 'aviso', codigo: 'responderam',
      detalhe: e instanceof Error ? e.message : String(e) });
  }
}

function gravarRespostas(): void {
  try {
    armazem()?.setItem(CHAVE_RESPOSTAS, JSON.stringify(Object.fromEntries(respostas)));
  } catch (e) {
    avisarSemArmazem(e);
  }
}

function carregar(): void {
  if (carregado) return;
  carregado = true;
  carregarRespostas();
  let bruto: string | null | undefined;
  try {
    bruto = armazem()?.getItem(CHAVE);
  } catch (e) {
    // Armazém que existe mas não responde (modo privado): é "sem armazém", não conteúdo inválido.
    avisarSemArmazem(e);
    return;
  }
  if (!bruto) return;
  try {
    const saved: unknown = JSON.parse(bruto);
    if (Array.isArray(saved)) {
      for (const id of saved) if (typeof id === 'string' && !estados.has(id)) {
        estados.set(id, { failures: 1, retryAt: Date.now() + RETRY_DELAYS_MS[0] });
      }
      gravar(); // Migra a marca antiga uma vez, sem renovar o prazo a cada recarga.
    } else if (saved && typeof saved === 'object') {
      for (const [id, value] of Object.entries(saved)) {
        if (!value || !Number.isInteger(value.failures) || value.failures < 1 ||
          !Number.isFinite(value.retryAt)) continue;
        if ((estados.get(id)?.retryAt ?? 0) <= value.retryAt) estados.set(id, value);
      }
    }
    // Marca do teste de adicionar gravada por versão anterior: apaga, não só ignora.
    if (estados.delete(SERVIDOR_CANDIDATO)) gravar();
  } catch (e) {
    // Conteúdo estragado não pode impedir o app de subir: começa limpo — mas fica no diário,
    // senão "voltou a procurar todo mundo" não tem explicação.
    registrarDiag({ evento: 'esfriamento.estado_invalido', nivel: 'aviso', detalhe: e instanceof Error ? e.message : String(e) });
  }
}

function gravar(): void {
  const saved = Object.fromEntries(estados);
  try {
    if (estados.size) armazem()?.setItem(CHAVE, JSON.stringify(saved));
    else armazem()?.removeItem(CHAVE);
  } catch (e) {
    // Cota cheia ou modo privado: o estado segue valendo em memória nesta sessão, e NÃO sobrevive
    // ao recarregamento — no iPhone é exatamente o caso que a feature existe pra cobrir. Uma vez
    // por sessão no diário, pra a volta das tentativas ter causa.
    avisarSemArmazem(e);
  }
}

/** Uma vez por sessão: o armazém está indisponível e a marca não sobrevive ao recarregamento. */
export function avisarSemArmazem(e: unknown): void {
  if (avisouArmazem) return;
  avisouArmazem = true;
  registrarDiag({ evento: 'esfriamento.sem_armazem', nivel: 'aviso', detalhe: e instanceof Error ? e.message : String(e) });
}

/** A última tentativa falhou e ainda não houve resposta que confirme a recuperação? */
export function estaDesligado(id: string): boolean {
  carregar();
  return estados.has(id);
}

/** Prazo até a próxima tentativa; expirar não é confirmação de que o servidor voltou. */
export function retryAfterMs(id: string): number {
  carregar();
  return Math.max(0, (estados.get(id)?.retryAt ?? 0) - Date.now());
}

/** Respondeu nas últimas 24 h: a falha é do aparelho, não da máquina. */
export function respondeuRecentemente(id: string): boolean {
  carregar();
  const em = respostas.get(id);
  return em !== undefined && Date.now() - em < RESPONDEU_RECENTE_MS;
}

/** Falhas simultâneas não renovam o prazo; nova tentativa frustrada aumenta a espera. */
export function registrarFalha(id: string): void {
  if (id === SERVIDOR_CANDIDATO) return;
  carregar();
  if (protegido(id) || retryAfterMs(id) > 0) return;
  const teto = respondeuRecentemente(id) ? TETO_RECENTE : RETRY_DELAYS_MS.length;
  const failures = Math.min((estados.get(id)?.failures ?? 0) + 1, teto);
  estados.set(id, { failures, retryAt: Date.now() + RETRY_DELAYS_MS[failures - 1] });
  gravar();
}

/** Respondeu: está de pé. */
export function registrarSucesso(id: string): void {
  if (id === SERVIDOR_CANDIDATO) return;
  carregar();
  // Toda resposta passa por aqui, inclusive o ping de 8 s: grava no máximo uma vez por minuto.
  const agora = Date.now();
  if (agora - (respostas.get(id) ?? 0) > 60_000) {
    respostas.set(id, agora);
    gravarRespostas();
  }
  if (!estados.delete(id)) return;
  gravar();
  for (const listener of [...recoveredListeners]) {
    try { listener(id); } catch (error) {
      registrarDiag({ evento: 'esfriamento.reconexao_falhou', nivel: 'erro',
        codigo: error instanceof Error ? error.name : 'erro' });
    }
  }
}

/** "Buscar agora": permite tentar de novo por ação da pessoa, sem aguardar uma resposta. */
export function retentarAgora(id?: string): void {
  carregar();
  if (id === undefined) estados.clear();
  else estados.delete(id);
  gravar();
}

/** Servidor saiu da lista: some com o estado dele. */
export function esquecerServidor(id: string): void {
  carregar();
  if (estados.delete(id)) gravar();
  if (respostas.delete(id)) gravarRespostas();
}

export function _limparEsfriamentoParaTestes(): void {
  estados.clear();
  respostas.clear();
  carregado = false;
  try {
    armazem()?.removeItem(CHAVE);
    armazem()?.removeItem(CHAVE_RESPOSTAS);
  } catch {
    /* sem armazém nos testes de nó */
  }
}
