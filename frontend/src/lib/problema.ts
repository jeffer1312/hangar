// Código de problema do backend (`SessionInfo.problema` / `StateEvent.problema`) -> texto na
// língua da tela. O backend manda só o código (regra de i18n); código desconhecido some em vez
// de virar um id cru na tela.
import * as m from '../paraglide/messages';

export function textoProblema(codigo: string | null | undefined): string | null {
  switch (codigo) {
    case 'codex_hooks_nao_aprovados': return m.problema_codex_hooks();
    case 'codex_abertura_falhou': return m.problema_codex_abertura_falhou();
    case 'headless_nao_subiu': return m.problema_headless_nao_subiu();
    case 'codex_headless_nao_subiu': return m.problema_codex_headless_nao_subiu();
    case 'codex_sem_conexao': return m.problema_codex_sem_conexao();
    case 'codex_limite_uso': return m.problema_codex_limite_uso();
    case 'limit_switch_no_room': return m.problema_limit_switch_no_room();
    case 'limit_switch_failed': return m.problema_limit_switch_failed();
    case 'limit_switch_no_target': return m.problema_limit_switch_no_target();
    case 'codex_sem_login': return m.problema_codex_sem_login();
    case 'codex_turno_cortado': return m.problema_codex_turno_cortado();
    case 'codex_esforco_nao_aplicado': return m.problema_codex_esforco_nao_aplicado();
    case 'codex_conversa_nao_abriu': return m.problema_codex_conversa_nao_abriu();
    case 'headless_caiu': return m.problema_headless_caiu();
    case 'codex_prompt_bloqueado': return m.problema_codex_prompt_bloqueado();
    case 'headless_sem_resposta': return m.problema_headless_sem_resposta();
    case 'headless_processo_caiu': return m.problema_headless_processo_caiu();
    case 'headless_turno_erro': return m.problema_headless_turno_erro();
    case 'headless_delivery_stalled': return m.problema_headless_delivery_stalled();
    case 'headless_sem_login': return m.problema_headless_sem_login();
    case 'runtime_falhou': return m.problema_runtime_falhou();
    case 'terminal_observacao_falhou': return m.problema_terminal_observacao_falhou();
    case 'state_facts_unavailable': return m.problema_state_facts_unavailable();
    case 'permission_observe_failed': return m.problema_permission_observe_failed();
    case 'terminal_input_composer_busy': return m.problema_terminal_input_composer_busy();
    case 'terminal_input_composer_unreadable': return m.problema_terminal_input_composer_unreadable();
    case 'terminal_input_capture_failed': return m.problema_terminal_input_capture_failed();
    case 'terminal_input_stalled': return m.problema_terminal_input_stalled();
    case 'terminal_input_delivery_unproved': return m.problema_terminal_input_delivery_unproved();
    case 'terminal_input_no_safe_delivery': return m.problema_terminal_input_no_safe_delivery();
    case 'terminal_clear_not_applied': return m.problema_terminal_clear_not_applied();
    case 'list_capture_failed': return m.problema_list_capture_failed();
    case 'list_runtime_unavailable': return m.problema_list_runtime_unavailable();
    case 'list_runtime_absent': return m.problema_list_runtime_absent();
    case 'list_facts_unavailable': return m.problema_list_facts_unavailable();
    case 'interacao_indisponivel': return m.problema_interacao_indisponivel();
    default: return null;
  }
}
