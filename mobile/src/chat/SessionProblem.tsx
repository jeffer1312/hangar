import { useEffect, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { DELIVERY_STALLED, type StateEvent } from '@hangar/core';
import * as m from '../paraglide/messages';
import { superficie } from '../theme/superficie';

interface Props {
  problem?: StateEvent['problema'];
  detail?: StateEvent['problema_detalhe'];
}

/** Com o painel "Na fila" à mostra, a entrega parada fica só nele (com o Reabrir): aqui ela seria anunciada duas vezes. */
export function showsSessionProblem(problem: StateEvent['problema'] | undefined, queuePanelShown: boolean): boolean {
  return !!problem && !(problem === DELIVERY_STALLED && queuePanelShown);
}

const problemLabels: Record<string, () => string> = {
  codex_hooks_nao_aprovados: m.problema_codex_hooks,
  codex_abertura_falhou: m.problema_codex_abertura_falhou,
  headless_nao_subiu: m.problema_headless_nao_subiu,
  codex_headless_nao_subiu: m.problema_codex_headless_nao_subiu,
  codex_sem_conexao: m.problema_codex_sem_conexao,
  codex_limite_uso: m.problema_codex_limite_uso,
  limit_switch_no_room: m.problema_limit_switch_no_room,
  limit_switch_failed: m.problema_limit_switch_failed,
  limit_switch_no_target: m.problema_limit_switch_no_target,
  codex_sem_login: m.problema_codex_sem_login,
  codex_turno_cortado: m.problema_codex_turno_cortado,
  codex_esforco_nao_aplicado: m.problema_codex_esforco_nao_aplicado,
  codex_conversa_nao_abriu: m.problema_codex_conversa_nao_abriu,
  headless_caiu: m.problema_headless_caiu,
  codex_prompt_bloqueado: m.problema_codex_prompt_bloqueado,
  headless_sem_resposta: m.problema_headless_sem_resposta,
  headless_processo_caiu: m.problema_headless_processo_caiu,
  headless_turno_erro: m.problema_headless_turno_erro,
  headless_sem_login: m.problema_headless_sem_login,
  headless_delivery_stalled: m.problema_headless_delivery_stalled,
  runtime_falhou: m.problema_runtime_falhou,
  terminal_observacao_falhou: m.problema_terminal_observacao_falhou,
  state_facts_unavailable: m.problema_state_facts_unavailable,
  permission_observe_failed: m.problema_permission_observe_failed,
  terminal_input_composer_busy: m.problema_terminal_input_composer_busy,
  terminal_input_composer_unreadable: m.problema_terminal_input_composer_unreadable,
  terminal_input_capture_failed: m.problema_terminal_input_capture_failed,
  terminal_input_stalled: m.problema_terminal_input_stalled,
  terminal_input_delivery_unproved: m.problema_terminal_input_delivery_unproved,
  terminal_clear_not_applied: m.problema_terminal_clear_not_applied,
  list_capture_failed: m.problema_list_capture_failed,
  list_runtime_unavailable: m.problema_list_runtime_unavailable,
  list_runtime_absent: m.problema_list_runtime_absent,
  list_facts_unavailable: m.problema_list_facts_unavailable,
  interacao_indisponivel: m.problema_interacao_indisponivel,
};

export function SessionProblem({ problem, detail }: Props) {
  const [expanded, setExpanded] = useState(false);
  useEffect(() => setExpanded(false), [problem, detail]);

  // O detalhe é saída crua: só o código de saída reconhecido pode chegar à tela.
  const match = problem === 'headless_processo_caiu'
    ? detail?.split('\n').at(-1)?.match(/^rc=(-?\d+)$/)
    : null;
  const exitCode = match ? Number(match[1]) : NaN;
  const hasDetail = Number.isSafeInteger(exitCode);
  const label = problem && Object.hasOwn(problemLabels, problem) ? problemLabels[problem]() : m.session_problem_unknown();
  // Runtime e observação mandam código e frase fixa do Rust, sem texto da conversa: pode ir à tela.
  const runtimeDetail = problem === 'runtime_falhou' || problem === 'terminal_observacao_falhou' || problem === 'terminal_input_stalled' ? detail?.split('\n')[0] : null;

  if (!problem) return null;

  return (
    <View style={styles.wrap}>
      <Text style={styles.message} accessibilityRole="alert" accessibilityLiveRegion="polite">{label}</Text>
      {runtimeDetail ? <Text style={styles.detail} selectable>{runtimeDetail}</Text> : null}
      {hasDetail ? (
        <Pressable
          onPress={() => setExpanded((value) => !value)}
          style={styles.toggle}
          accessibilityRole="button"
          accessibilityState={{ expanded }}
        >
          <Text style={styles.toggleText}>{expanded ? m.session_problem_hide_details() : m.session_problem_show_details()}</Text>
        </Pressable>
      ) : null}
      {hasDetail && expanded ? (
        <Text style={styles.detail} selectable>{m.session_problem_exit_code({ code: String(exitCode) })}</Text>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: {
    backgroundColor: superficie(theme, 0.8),
    borderColor: theme.tokens.status.warning,
    borderWidth: 1,
    borderRadius: theme.base.radius.md,
    padding: theme.base.space[3],
    gap: theme.base.space[1],
  },
  message: {
    color: theme.tokens.text.primary,
    fontSize: theme.base.text.base,
  },
  toggle: {
    alignSelf: 'flex-start',
    minWidth: 44,
    minHeight: 44,
    justifyContent: 'center',
  },
  toggleText: {
    color: theme.tokens.accent.base,
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
  },
  detail: {
    color: theme.tokens.text.secondary,
    fontSize: theme.base.text.sm,
  },
}));
