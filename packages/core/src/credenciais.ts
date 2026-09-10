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
  sync: { status: 'idle' | 'running' | 'ready' | 'partial' | 'error'; trust_pending: boolean; issues: { code: string; params: Record<string, string> }[] };
}
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

export function codexAccountMessage(issue: { code: string; params?: Record<string, string> }): string {
  return mensagemDeErro(issue.code, issue.params) ?? m.codex_account_error_unknown();
}
