// Controller do bloco de horas silenciosas (PushQuiet). Extraído pro arquivo PRA SER TESTÁVEL: toda a
// máquina de corrida (dedup de load, serialização load/save, watchdog de timeout, invalidação por
// troca de alvo, dispose) vive aqui, em TS puro, sem runes nem DOM — o componente só renderiza e
// dispara `sync()`/`save()`. O `estado` é injetado (no componente, um objeto `$state`; no teste, um
// objeto simples) e mutado diretamente pelo controller — quem renderiza vê a mudança.
//
// Contrato de comportamento (rounds 2 e 3 do review):
// - mesmo alvo + load em voo => dedup: reabrir não dispara 2º GET;
// - mesmo alvo + save pendente => NÃO limpar/recarregar/invalidar o save: refresh é deferido
//   (refreshDepois) e roda quando o save concluir — SÓ no sucesso, e nunca com o menu fechado;
// - draft sujo do mesmo alvo (campos editados ≠ último valor carregado) não é sobrescrito por reload;
// - ownership: o finally/callback de um save ANTIGO (invalidação por troca de alvo ou por dispose)
//   não muta saving/flags da operação atual;
// - transição para 'unavailable' invalida a operação anterior e deixa loading/saving coerentes;
// - getPushSettings não tem timeout próprio: watchdog impede loading preso pra sempre, e a resposta
//   tardia não pinta nada (geração avançada);
// - dispose(): limpa o watchdog e avança a geração — nenhum callback tardio publica nem inicia load.

import * as m from '../paraglide/messages';
import type { Server } from './auth';

export type PushTarget =
  | { mode: 'global' }
  | { mode: 'server'; server: Server }
  | { mode: 'unavailable' };

export interface QuietState {
  qhStart: string;
  qhEnd: string;
  qhMsg: string;
  loading: boolean;
  saving: boolean;
}

export interface QuietHoursApi {
  getPushSettings(): Promise<{ muted: string[]; quiet_hours: { start: string; end: string } | null }>;
  getPushSettingsForServer(s: Server): Promise<{ muted: string[]; quiet_hours: { start: string; end: string } | null }>;
  setQuietHours(start: string | null, end: string | null): Promise<unknown>;
  setQuietHoursForServer(s: Server, start: string | null, end: string | null): Promise<unknown>;
}

export interface QuietHoursDeps {
  estado: QuietState;
  getAlvo: () => PushTarget;
  getOpen: () => boolean;
  podePush: () => boolean;
  api: QuietHoursApi;
  timeoutMs?: number;
}

export class QuietHoursController {
  private generation = 0;
  private loadInFlight = false;
  private refreshDepois = false;
  private loadedStart = '';
  private loadedEnd = '';
  private ultimoAlvo: string | null = null;
  private watchdog: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;

  constructor(private d: QuietHoursDeps) {}

  /** Rascunho sujo: usuário editou um campo e ainda não salvou/carregou. */
  get dirty(): boolean {
    return this.d.estado.qhStart !== this.loadedStart || this.d.estado.qhEnd !== this.loadedEnd;
  }

  // Chamado pelo componente a cada mudança de alvo/abertura. Invalida a operação anterior quando o
  // alvo troca; só então (aberto + push suportado) decide se carrega. DEVE ser chamado dentro de
  // untrack(...) pelo efeito do componente: o alvo é lido aqui, e sem untrack o efeito ficaria
  // dependente do OBJETO target (re-criado a cada render do pai) — reload a cada recomputo.
  sync(): void {
    if (this.disposed) return;
    const alvo = this.d.getAlvo();
    const key = alvo.mode === 'server' ? alvo.server.id : alvo.mode;
    if (key !== this.ultimoAlvo) {
      // Alvo trocou (ou primeira chamada): operação anterior morre aqui — inclusive um save em voo,
      // cujo resultado não pode pintar por cima do novo alvo. Campos limpos pro alvo novo.
      this.generation++;
      this.loadInFlight = false;
      this.refreshDepois = false;
      this.d.estado.loading = false;
      this.d.estado.saving = false;
      this.d.estado.qhStart = '';
      this.d.estado.qhEnd = '';
      this.d.estado.qhMsg = '';
      this.loadedStart = '';
      this.loadedEnd = '';
      this.ultimoAlvo = key;
    }
    if (!this.d.getOpen() || !this.d.podePush()) return;
    if (alvo.mode === 'unavailable') {
      this.d.estado.qhMsg = m.quiet_servidor_indisponivel();
      return;
    }
    if (this.d.estado.saving) {
      // Save em voo do MESMO alvo: não limpa nem recarrega por cima dele; refresh fica deferido.
      this.refreshDepois = true;
      return;
    }
    if (this.loadInFlight) return;   // dedup: 1 GET por abertura/alvo
    if (this.dirty) return;          // draft sujo do mesmo alvo: reload sobrescreveria a edição
    void this.load();
  }

  async save(): Promise<void> {
    if (this.disposed) return;
    if (this.d.estado.saving || this.d.estado.loading) return;   // duplo clique / save durante load
    const alvo = this.d.getAlvo();
    const inicio = this.d.estado.qhStart;   // snapshot: salva o que o usuário editou NESTE alvo
    const fim = this.d.estado.qhEnd;
    this.d.estado.saving = true;
    const mine = ++this.generation;       // invalida load em voo: nada dele repinta depois do save
    this.d.estado.qhMsg = '';
    const meuWatchdog = this.armarWatchdog(() => {
      if (mine !== this.generation) return;
      // Estourou: o save morreu. Geração avança pra resposta tardia não pintar; draft fica no campo.
      this.generation++;
      this.refreshDepois = false;
      this.d.estado.saving = false;
      this.d.estado.qhMsg = m.quiet_erro_salvar();
    });
    let sucesso = false;
    try {
      if (alvo.mode === 'server') {
        await this.d.api.setQuietHoursForServer(alvo.server, inicio || null, fim || null);
      } else if (alvo.mode === 'global') {
        await this.d.api.setQuietHours(inicio || null, fim || null);
      } else {
        throw new Error(m.quiet_servidor_indisponivel());
      }
      if (mine !== this.generation) return;
      sucesso = true;
      this.d.estado.qhMsg = inicio && fim ? `silenciado ${inicio}–${fim}` : 'desligado';
      this.loadedStart = inicio;   // o que salvou vira a base: draft limpo, reload permitido
      this.loadedEnd = fim;
    } catch (e) {
      if (mine !== this.generation) return;
      this.refreshDepois = false;  // falhou: nada de refresh; draft e erro ficam na tela
      this.d.estado.qhMsg = e instanceof Error ? e.message : m.quiet_erro_salvar();
    } finally {
      // Limpa SÓ o watchdog desta operação: o this.watchdog pode já ser de um save novo (alvo
      // trocou no meio), e limpá-lo deixaria a operação nova sem timeout.
      if (meuWatchdog) clearTimeout(meuWatchdog);
      if (mine === this.generation) {
        // Ownership: só a operação ATUAL mexe nas flags. Um save antigo (alvo trocou / dispose)
        // já foi invalidado pela geração e não pode derrubar o saving do save novo.
        this.d.estado.saving = false;
        if (sucesso && this.refreshDepois) {
          // Refresh deferido durante o save: roda agora, SÓ no sucesso e SÓ com o menu aberto.
          this.refreshDepois = false;
          if (this.d.getOpen()) void this.load();
        }
      }
    }
  }

  dispose(): void {
    this.disposed = true;
    this.generation++;              // invalida qualquer op em voo: callbacks tardios não publicam
    this.limparWatchdog();
  }

  private armarWatchdog(fn: () => void): ReturnType<typeof setTimeout> {
    this.limparWatchdog();
    this.watchdog = setTimeout(() => {
      this.watchdog = null;
      fn();
    }, this.d.timeoutMs ?? 15000);
    return this.watchdog;
  }

  private limparWatchdog(): void {
    if (this.watchdog) {
      clearTimeout(this.watchdog);
      this.watchdog = null;
    }
  }

  private async load(): Promise<void> {
    if (this.disposed || this.loadInFlight) return;
    const alvo = this.d.getAlvo();
    if (alvo.mode === 'unavailable') return;
    const mine = ++this.generation;
    this.loadInFlight = true;
    this.d.estado.loading = true;
    this.d.estado.qhMsg = '';
    const meuWatchdog = this.armarWatchdog(() => {
      if (mine !== this.generation) return;
      // Estourou: avança a geração pra resposta tardia não pintar, e libera o loading.
      this.generation++;
      this.loadInFlight = false;
      this.d.estado.loading = false;
      this.d.estado.qhMsg = m.quiet_erro_carregar();
    });
    try {
      const result = alvo.mode === 'server'
        ? await this.d.api.getPushSettingsForServer(alvo.server)
        : await this.d.api.getPushSettings();
      if (mine !== this.generation) return;   // resposta tardia (timeout/invalidação): não pinta
      this.d.estado.qhStart = result.quiet_hours?.start ?? '';
      this.d.estado.qhEnd = result.quiet_hours?.end ?? '';
      this.loadedStart = this.d.estado.qhStart;
      this.loadedEnd = this.d.estado.qhEnd;
    } catch (e) {
      if (mine !== this.generation) return;
      // Global segue best-effort (offline/rota ausente -> campos vazios, salvar depois resolve).
      // Por-servidor NÃO: apiFetchForServer não faz o self-heal de 401 de propósito (não pode
      // derrubar a credencial de outra máquina), então token morto ficaria como "campos vazios"
      // pra sempre — indistinguível de "nunca configurei".
      if (alvo.mode === 'server') this.d.estado.qhMsg = e instanceof Error ? e.message : m.quiet_erro_carregar();
    } finally {
      if (meuWatchdog) clearTimeout(meuWatchdog);
      if (mine === this.generation) {
        this.loadInFlight = false;
        this.d.estado.loading = false;
      }
    }
  }
}
