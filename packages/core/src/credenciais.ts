import type { CotaContaResumo } from './cotaResumo';
import { mensagemDeErro } from './errosApi';
import * as m from './paraglide/messages';

export type AuthMethod = 'oauth' | 'api_key' | 'none' | 'unknown';
export interface CodexAccount {
  id: string;
  credential_id: string;
  name: string;
  home: string;
  is_default: boolean;
  auth: { method: AuthMethod; status: 'connected' | 'disconnected' | 'unavailable'; email: string | null; plan: string | null };
  sync: { status: CodexSyncStatus; trust_pending: boolean; issues: { code: string; params: Record<string, string> }[];
    /** Em que ponto a herança está: `principal`, `configuracoes`, `recursos` ou `plugins`. */
    etapa?: string | null;
    /** O que a conta recebeu, por tipo, quando a herança terminou. */
    herdado?: Record<string, number> | null };
  /** Só na padrão: tem config/plugins/hooks que valham herdar numa conta adicional. */
  has_settings?: boolean;
}
export type CodexSyncStatus = 'idle' | 'running' | 'ready' | 'partial' | 'error';
export interface CodexLoginAttempt {
  account_id: string;
  attempt_id: string;
  status: 'waiting' | 'completed' | 'failed' | 'cancelled';
  user_code?: string;
  verification_url?: string;
  error?: { code: string; params: Record<string, string> };
}
export type TipoCredencial = 'claude' | 'chave' | 'codex';
export interface CotaResumo extends Omit<CotaContaResumo, 'id'> {
  ts?: number | null;
  idade_s?: number | null;
}
export interface Credencial {
  id: string;
  tipo: TipoCredencial;
  auth_method?: AuthMethod;
  codex_account?: string | null;
  /** Conta Codex adicional: herança da padrão. `idle` = nunca herdou, o card oferece o botão. */
  codex_sync?: CodexSyncStatus | null;
  nome: string;
  nome_natural: string;
  apelido?: string | null;
  ativa: boolean;
  path?: string | null;
  login?: { estado: 'ok' | 'indisponivel'; loggedIn?: boolean | null; email?: string | null; plano?: string | null; motivo?: string | null } | null;
  base_url?: string | null;
  chave_mascarada?: string | null;
  usos: string[];
  cota?: CotaResumo | null;
  aceita_cookie?: boolean;
  cookie_definido?: boolean;
  /** false = chave do próprio agente (o `apikey` do Kimi): o app lista, mas não apaga. */
  gerenciada?: boolean;
}

export function credentialAuth(c: Credencial): AuthMethod {
  return c.auth_method ?? (c.tipo === 'claude' ? 'oauth' : 'unknown');
}
export function credentialGroup(c: Credencial): 'subscription' | 'claude_engine' | 'api_key' | 'unknown' {
  const auth = credentialAuth(c);
  if (auth === 'oauth') return 'subscription';
  if (auth === 'api_key') return c.usos?.includes('claude_code') ? 'claude_engine' : 'api_key';
  return 'unknown';
}

/**
 * Conta que o "adicionar conta Codex" deve logar. A padrao (~/.codex) sem login e a primeira
 * conta da maquina: criar uma adicional ali deixaria a padrao vazia e copiaria dela o nada.
 */
export function contaCodexParaEntrar(accounts: CodexAccount[] | undefined): 'default' | undefined {
  const padrao = accounts?.find((a) => a.is_default);
  return padrao?.auth.status === 'disconnected' ? 'default' : undefined;
}

/** Nome digitado → id da conta Codex (regra do backend: `[a-z0-9][a-z0-9_-]{0,31}`). */
export function idContaCodex(texto: string): string {
  return texto.toLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '')
    .replace(/[^a-z0-9_-]+/g, '-').replace(/^[-_]+|-+$/g, '').slice(0, 32);
}

/** Estado da importação Claude → Codex (`GET /api/harness/codex/integracao`), só o que o login usa. */
export interface CodexIntegracaoEstado {
  estado: 'ocioso' | 'executando' | 'ok' | 'parcial' | 'erro' | 'indisponivel';
  ultima_execucao: string | null;
  /** O que está acontecendo agora; código traduzido por `harness_codex_m_<codigo>`, ou texto cru. */
  etapa?: { codigo: string | null; params: Record<string, string>; texto: string } | string | null;
}

/** A etapa da integração chega como código + parâmetros, ou como texto de um backend antigo. */
export function textoEtapaCodex(etapa: CodexIntegracaoEstado['etapa'],
                                traduz: (codigo: string, params: Record<string, string>) => string | undefined): string {
  if (!etapa) return '';
  if (typeof etapa === 'string') return etapa;
  return (etapa.codigo ? traduz(etapa.codigo, etapa.params ?? {}) : undefined) ?? etapa.texto ?? '';
}

/**
 * O que oferecer depois do login: a padrão (primeira conta) importa do Claude; a adicional herda
 * da padrão — só se a padrão tiver algo e esta nunca tiver herdado. `null` = nada a perguntar.
 */
export function importacaoAposLoginCodex(accounts: CodexAccount[] | undefined, id: string): 'claude' | 'heranca' | null {
  const conta = accounts?.find((a) => a.id === id);
  if (!conta) return null;
  if (conta.is_default) return 'claude';
  const padrao = accounts?.find((a) => a.is_default);
  return conta.sync.status === 'idle' && padrao?.has_settings ? 'heranca' : null;
}

export function codexAccountMessage(issue: { code: string; params?: Record<string, string> }): string {
  return mensagemDeErro(issue.code, issue.params) ?? m.codex_account_error_unknown();
}

export function codexPreparationMessage(stage?: string | null): string {
  if (stage === 'principal') return m.codex_etapa_principal();
  if (stage === 'configuracoes') return m.codex_etapa_configuracoes();
  if (stage === 'recursos') return m.codex_etapa_recursos();
  if (stage === 'plugins') return m.codex_etapa_plugins();
  return m.codex_ui_preparing();
}
