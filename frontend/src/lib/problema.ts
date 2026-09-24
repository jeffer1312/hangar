// Código de problema do backend (`SessionInfo.problema` / `StateEvent.problema`) -> texto na
// língua da tela. O backend manda só o código (regra de i18n); código desconhecido some em vez
// de virar um id cru na tela.
import * as m from '../paraglide/messages';

export function textoProblema(codigo: string | null | undefined): string | null {
  switch (codigo) {
    case 'codex_hooks_nao_aprovados': return m.problema_codex_hooks();
    case 'headless_nao_subiu': return m.problema_headless_nao_subiu();
    case 'codex_headless_nao_subiu': return m.problema_codex_headless_nao_subiu();
    case 'codex_sem_conexao': return m.problema_codex_sem_conexao();
    case 'codex_prompt_bloqueado': return m.problema_codex_prompt_bloqueado();
    case 'headless_sem_resposta': return m.problema_headless_sem_resposta();
    case 'headless_processo_caiu': return m.problema_headless_processo_caiu();
    case 'headless_turno_erro': return m.problema_headless_turno_erro();
    case 'headless_sem_login': return m.problema_headless_sem_login();
    default: return null;
  }
}
