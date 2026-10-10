// Traducao dos erros de API que o backend manda no formato {code, params, msg}
// (backend/app/mensagens.py). O `code` e o contrato: e por ele que o front acha a mensagem
// no idioma do app. `params` sao os valores que entram na frase; `msg` e o texto em portugues
// que o backend mandou junto, e so a rede quando o code e desconhecido.
//
// Mapa EXPLICITO, nunca `m['erro_' + code]`: o acesso dinamico anula o tipo gerado pelo
// Paraglide, que e metade da trava deste mecanismo. Code fora do mapa devolve `undefined`
// e o errorDetail cai no `msg` — backend mais novo que o front mostra texto legivel em vez
// de um codigo cru na tela.
import * as m from './paraglide/messages';

type Parametros = Record<string, unknown>;

/** Forma do erro que o backend manda: {code, params, msg} (backend/app/mensagens.py). */
export interface EnvelopeErro {
  code: string;
  params?: Parametros;
  msg: string;
}

/**
 * Resolve params aninhados: envelope {code, params, msg} ou {sessao, erro}; tambem arrays
 * (avisos compostos); sem isto o Paraglide renderizaria [object Object] no template.
 */
function fmtParam(v: unknown): string {
  if (Array.isArray(v)) return v.map(fmtParam).join('; ');
  if (v && typeof v === 'object') {
    const o = v as Record<string, unknown>;
    if (typeof o.code === 'string') {
      return mensagemDeErro(o.code, (o.params ?? {}) as Parametros) ?? String(o.msg ?? o.code);
    }
    if (o.sessao !== undefined && o.erro !== undefined) {
      return `${o.sessao}: ${fmtParam(o.erro)}`;
    }
    return JSON.stringify(v);
  }
  return v == null ? '' : String(v);
}

/**
 * Formata um erro que pode ser string crua (endpoint antigo) OU envelope {code, params, msg}.
 * Usado onde o backend manda erro no CORPO (não no `detail`): BroadcastResult.error e
 * PairResult.warning. Retorna undefined quando não é nenhum dos dois (o chamador decide o fallback).
 */
export function formataErro(e: unknown): string | undefined {
  if (typeof e === 'string') return e;
  if (e && typeof e === 'object' && typeof (e as EnvelopeErro).code === 'string') {
    const env = e as EnvelopeErro;
    return mensagemDeErro(env.code, env.params ?? {})
      ?? env.msg
      ?? (env.code.startsWith('codex_account_') || env.code.startsWith('codex_login_')
        ? m.codex_account_error_unknown() : env.code);
  }
  return undefined;
}

const ERROS: Record<string, (params: Parametros) => string> = {
  claude_context_invalid: () => m.claude_context_invalid(),
  claude_context_unreadable: () => m.claude_context_unreadable(),
  claude_customizations_busy: () => m.claude_customizations_busy(),
  claude_customizations_failed: () => m.claude_customizations_failed(),
  claude_customizations_invalid: () => m.claude_customizations_invalid(),
  claude_customizations_owner_only: () => m.claude_customizations_owner_only(),
  claude_customizations_unavailable: () => m.claude_customizations_unavailable(),
  claude_customizations_read_failed: () => m.claude_customizations_read_failed(),
  claude_customizations_record_invalid: () => m.claude_customizations_record_invalid(),
  claude_customizations_store_unavailable: () => m.claude_customizations_store_unavailable(),
  claude_customizations_write_failed: () => m.claude_customizations_write_failed(),
  claude_plugin_unknown: () => m.claude_plugin_unknown(),
  claude_plugins_ambiguous: () => m.claude_plugins_ambiguous(),
  claude_plugins_failed: () => m.claude_plugins_failed(),
  claude_plugins_invalid: () => m.claude_plugins_invalid(),
  claude_plugins_timeout: () => m.claude_plugins_timeout(),
  claude_plugins_too_large: () => m.claude_plugins_too_large(),
  claude_plugins_unavailable: () => m.claude_plugins_unavailable(),
  claude_plugins_unreadable: () => m.claude_plugins_unreadable(),
  claude_settings_invalid: () => m.claude_settings_invalid(),
  claude_settings_unreadable: () => m.claude_settings_unreadable(),
  claude_skill_metadata_invalid: () => m.claude_skill_metadata_invalid(),
  claude_skill_restricted: () => m.claude_skill_restricted(),
  claude_skill_unknown: () => m.claude_skill_unknown(),
  codex_account_error_unknown: () => m.codex_account_error_unknown(),
  codex_account_ambiguous_rollout: () => m.codex_account_ambiguous_rollout(),
  codex_account_archive_mismatch: () => m.codex_account_archive_mismatch(),
  codex_account_conflict: () => m.codex_account_conflict(),
  codex_account_create_failed: () => m.codex_account_create_failed(),
  codex_account_exists: () => m.codex_account_exists(),
  codex_account_invalid_marker: () => m.codex_account_invalid_marker(),
  codex_account_invalid_name: () => m.codex_account_invalid_name(),
  codex_account_marker_failed: () => m.codex_account_marker_failed(),
  codex_account_not_found: () => m.codex_account_not_found(),
  codex_account_auth_storage_invalid: () => m.codex_account_auth_storage_invalid(),
  codex_account_creation_in_progress: () => m.codex_account_creation_in_progress(),
  codex_account_in_use: () => m.codex_account_in_use(),
  codex_account_default_protected: () => m.codex_account_default_protected(),
  codex_account_delete_failed: () => m.codex_account_delete_failed(),
  codex_account_login_in_progress: () => m.codex_account_login_in_progress(),
  codex_account_prepare_required: () => m.codex_account_prepare_required(),
  codex_account_preparing: () => m.codex_account_preparing(),
  codex_account_service_unavailable: () => m.codex_account_service_unavailable(),
  codex_account_login_failed: () => m.codex_account_login_failed(),
  codex_account_login_timeout: () => m.codex_account_login_timeout(),
  codex_account_prepare_failed: () => m.codex_account_prepare_failed(),
  codex_account_usage_unknown: () => m.codex_account_usage_unknown(),
  codex_account_so_codex: () => m.codex_account_so_codex(),
  codex_account_plugin_config_failed: () => m.codex_account_plugin_config_failed(),
  codex_account_plugin_hooks_failed: () => m.codex_account_plugin_hooks_failed(),
  codex_account_plugin_install_failed: () => m.codex_account_plugin_install_failed(),
  codex_account_plugin_inventory_failed: () => m.codex_account_plugin_inventory_failed(),
  codex_account_plugin_marketplace_add_failed: () => m.codex_account_plugin_marketplace_add_failed(),
  codex_account_plugin_marketplace_invalid: () => m.codex_account_plugin_marketplace_invalid(),
  codex_account_plugin_marketplace_missing: () => m.codex_account_plugin_marketplace_missing(),
  codex_account_plugin_marketplace_unavailable: () => m.codex_account_plugin_marketplace_unavailable(),
  codex_account_plugin_not_installed: () => m.codex_account_plugin_not_installed(),
  codex_account_plugin_origin_conflict: () => m.codex_account_plugin_origin_conflict(),
  codex_account_plugin_origin_unknown: () => m.codex_account_plugin_origin_unknown(),
  codex_account_plugin_trust_unavailable: () => m.codex_account_plugin_trust_unavailable(),
  codex_account_plugin_update_failed: () => m.codex_account_plugin_update_failed(),
  codex_account_plugin_version_conflict: () => m.codex_account_plugin_version_conflict(),
  codex_account_source_destination_conflict: () => m.codex_account_source_destination_conflict(),
  codex_account_source_sync_incomplete: () => m.codex_account_source_sync_incomplete(),
  codex_account_changed_during_prepare: () => m.codex_account_changed_during_prepare(),
  codex_account_destination_invalid: () => m.codex_account_destination_invalid(),
  codex_account_local_change: () => m.codex_account_local_change(),
  codex_account_mcp_auth_excluded: () => m.codex_account_mcp_auth_excluded(),
  codex_account_mcp_runtime_excluded: () => m.codex_account_mcp_runtime_excluded(),
  codex_account_path_conflict: () => m.codex_account_path_conflict(),
  codex_account_preference_invalid: () => m.codex_account_preference_invalid(),
  codex_account_provider_auth_excluded: () => m.codex_account_provider_auth_excluded(),
  codex_account_provider_divergence: () => m.codex_account_provider_divergence(),
  codex_account_resource_conflict: () => m.codex_account_resource_conflict(),
  codex_account_resource_local_change: () => m.codex_account_resource_local_change(),
  codex_account_restriction_conflict: () => m.codex_account_restriction_conflict(),
  codex_account_source_broad_link: () => m.codex_account_source_broad_link(),
  codex_account_source_cycle: () => m.codex_account_source_cycle(),
  codex_account_source_forbidden: () => m.codex_account_source_forbidden(),
  codex_account_source_invalid: (p) => m.codex_account_source_invalid({ path: String(p.path ?? '') }),
  codex_account_source_root_link: () => m.codex_account_source_root_link(),
  codex_account_source_unreadable: () => m.codex_account_source_unreadable(),
  codex_account_state_invalid: () => m.codex_account_state_invalid(),
  codex_account_unknown_preference: () => m.codex_account_unknown_preference(),
  codex_account_unmapped_reference: () => m.codex_account_unmapped_reference(),
  codex_login_attempt_mismatch: () => m.codex_login_attempt_mismatch(),
  erro_codex_resposta_invalida: () => m.erro_codex_resposta_invalida(),
  erro_codex_resposta_envio: () => m.erro_codex_resposta_envio(),
  erro_codex_controle: () => m.erro_codex_controle(),
  erro_plano_removido: () => m.chat_plan_ausente(),
  erro_plano_ilegivel: () => m.chat_plan_erro(),
  // /api/claude-configs — apagar conta recusado por alguma condicao da maquina
  erro_config_dirs_fixo: () => m.erro_config_dirs_fixo(),
  erro_conta_ativa_backend: () => m.erro_conta_ativa_backend(),
  erro_conta_lista_fixa: () => m.erro_conta_lista_fixa(),
  erro_conta_nome_invalido: () => m.erro_conta_nome_invalido(),
  erro_conta_inexistente: (p) => m.erro_conta_inexistente({ nome: String(p.nome) }),
  erro_login_ja_em_curso: () => m.erro_login_ja_em_curso(),
  erro_login_sem_tentativa: () => m.erro_login_sem_tentativa(),
  erro_login_credencial_ilegivel: () => m.erro_login_credencial_ilegivel(),
  erro_login_timeout: () => m.erro_login_timeout(),
  erro_login_nao_confirmado: () => m.contas_login_nao_confirmado(),
  erro_logout_nao_confirmado: () => m.erro_logout_nao_confirmado(),
  erro_config_dir_sessao: (p) => m.erro_config_dir_sessao({ nome: String(p.nome) }),
  erro_sessao_usa_conta: (p) => m.erro_sessao_usa_conta({ nome: String(p.nome) }),
  erro_varredura_processos: () => m.erro_varredura_processos(),
  erro_processos_usam_conta: (p) => m.erro_processos_usam_conta({ pids: String(p.pids) }),

  // /api/desktop — a paleta e o papel de parede sao resposta de negocio, nao erro
  erro_sem_paleta: () => m.erro_sem_paleta(),
  erro_sem_papel_de_parede: () => m.erro_sem_papel_de_parede(),

  // /api/broadcast — slash-command nao viaja no fan-out
  erro_broadcast_slash: () => m.erro_broadcast_slash(),

  // /api/config e /api/engines — corpo que nao e objeto
  erro_corpo_deve_ser_objeto: () => m.erro_corpo_deve_ser_objeto(),

  // /api/engines — motor nao existe, nome misturado com dados, ou faltando
  erro_motor_nao_encontrado: () => m.erro_motor_nao_encontrado(),
  erro_credencial_nao_gerenciada: () => m.erro_credencial_nao_gerenciada(),
  erro_motor_nome_com_dados: () => m.erro_motor_nome_com_dados(),
  erro_motor_nome_ou_dados: () => m.erro_motor_nome_ou_dados(),

  // /api/model-options e POST /api/sessions — catalogo do Pi falhou ou provider fora de escopo
  erro_pi_list_models: (p) => m.erro_pi_list_models({ erro: String(p.erro) }),
  // omp e o fork do Pi mas tem catalogo e binario proprios — codigo separado do erro_pi_list_models
  // pra sessao omp nao mostrar a instrucao/comando do Pi (ver erro_catalogo_omp_indisponivel).
  erro_omp_list_models: (p) => m.erro_omp_list_models({ erro: String(p.erro) }),
  // Sem `{erro}` de proposito: o texto do backend aqui e a instrucao ("instale o Pi / ajuste o
  // PATH"), nao um errno pra repassar — o que a pessoa precisa ler ja esta na frase traduzida.
  erro_pi_ausente: () => m.erro_pi_ausente(),
  // Binario proprio, instalacao propria: o omp e um executavel nativo, nao o pacote npm do Pi.
  erro_omp_ausente: () => m.erro_omp_ausente(),
  // Mesmo par pro Codex, e pelo mesmo motivo: "nao achei o codex" e outra conversa que "o
  // model/list falhou". O `{erro}` so viaja no segundo, onde a mensagem do provedor e a pista.
  erro_codex_model_list: (p) => m.erro_codex_model_list({ erro: String(p.erro) }),
  erro_codex_ausente: () => m.erro_codex_ausente(),
  erro_provider_invalido: () => m.erro_provider_invalido(),
  // POST /api/ditado/relimpar — so aparece se o cliente mandar um estilo fora da lista (os botoes
  // da barra do ditado saem de estilosDitado, entao na pratica e defesa de contrato).
  erro_estilo_invalido: () => m.erro_estilo_invalido(),
  // POST /api/sessions aceita claude/codex/pi/kimi/omp — o texto generico nao pode orientar so
  // claude/pi (contrato do erro_provider_invalido, que so vale pro /api/model-options)
  erro_provider_sessao_invalido: () => m.erro_provider_sessao_invalido(),

  // /api/archive — arquivo inexistente, path invalido, motor errado no resume
  // (erro_imagem_nao_encontrada tambem e usado por /api/sessions/{name}/transcript-image)
  erro_path_invalido: () => m.erro_path_invalido(),
  erro_projeto_nao_encontrado: () => m.erro_projeto_nao_encontrado(),
  erro_transcript_nao_encontrado: () => m.erro_transcript_nao_encontrado(),
  erro_trecho_nao_encontrado: () => m.erro_trecho_nao_encontrado(),
  erro_computer_control_read: (p) => m.erro_computer_control_read({ file: String(p.file), error: String(p.error) }),
  erro_computer_control_dir: (p) => m.erro_computer_control_dir({ dir: String(p.dir) }),
  erro_computer_control_agent: (p) => m.erro_computer_control_agent({ file: String(p.file) }),
  erro_computer_control_url: () => m.erro_computer_control_url(),
  erro_computer_control_effort: (p) => m.erro_computer_control_effort({ effort: String(p.effort) }),
  erro_computer_control_cliproxy_key: () => m.erro_computer_control_cliproxy_key(),
  erro_computer_control_models: (p) => m.erro_computer_control_models({ error: String(p.error) }),
  erro_computer_control_target_name: () => m.erro_computer_control_target_name(),
  erro_computer_control_target_exists: (p) => m.erro_computer_control_target_exists({ name: String(p.name) }),
  erro_computer_control_local_only_windows: () => m.erro_computer_control_local_only_windows(),
  erro_computer_control_jev_key_moved: () => m.erro_computer_control_jev_key_moved(),
  erro_computer_control_target_host: () => m.erro_computer_control_target_host(),
  erro_computer_control_not_installed: () => m.erro_computer_control_not_installed(),
  erro_computer_control_no_ssh_key: () => m.erro_computer_control_no_ssh_key(),
  erro_computer_control_release: (p) => m.erro_computer_control_release({ error: String(p.error) }),
  erro_nao_encontrado: () => m.erro_nao_encontrado(),
  erro_imagem_nao_encontrada: () => m.erro_imagem_nao_encontrada(),
  erro_cwd_ausente: () => m.erro_cwd_ausente(),
  erro_motor_invalido: () => m.erro_motor_invalido(),

  // /api/ask-history e /api/sessions/{name}/loop — kill-switch desligado
  erro_automacoes_desligadas: () => m.erro_automacoes_desligadas(),

  // /api/tts — texto vazio apos limpar, teto, limite, audio sumido do cache
  erro_tts_sem_texto: () => m.erro_tts_sem_texto(),
  erro_tts_teto: (p) => m.erro_tts_teto({ n: String(p.n), teto: String(p.teto) }),
  erro_tts_limite: (p) => m.erro_tts_limite({ n: String(p.n), limite: String(p.limite) }),
  erro_tts_audio_invalido: () => m.erro_tts_audio_invalido(),
  erro_tts_sem_cache: () => m.erro_tts_sem_cache(),

  // /api/sync — hub zero-knowledge (login, cadastro, rate limit)
  erro_nao_autorizado: () => m.erro_nao_autorizado(),
  erro_compartilhar_pre_requisito: () => m.erro_compartilhar_pre_requisito(),
  erro_compartilhamento_inexistente: () => m.erro_compartilhamento_inexistente(),
  erro_compartilhar_sem_rede_local: () => m.erro_compartilhar_sem_rede_local(),
  erro_compartilhar_porta_do_convite: (p) => m.erro_compartilhar_porta_do_convite({ port: String(p.port) }),
  erro_fora_do_convite: () => m.erro_fora_do_convite(),
  erro_convite_encerrado: () => m.erro_convite_encerrado(),
  erro_convite_usado: () => m.erro_convite_usado(),
  erro_convite_vencido: () => m.erro_convite_vencido(),
  erro_convite_revogado: () => m.erro_convite_revogado(),
  erro_convite_inexistente: () => m.erro_convite_inexistente(),
  erro_sessao_indisponivel: () => m.erro_sessao_indisponivel(),
  erro_bootstrap_invalido: () => m.erro_bootstrap_invalido(),
  erro_ja_registrado: () => m.erro_ja_registrado(),
  erro_muitas_tentativas: () => m.erro_muitas_tentativas(),

  // /api/deploy — webhook do GitHub (o front nunca chama; a traducao existe pro caso de um dia)
  erro_assinatura_invalida: () => m.erro_assinatura_invalida(),
  erro_payload_invalido: () => m.erro_payload_invalido(),
  erro_deploy_falhou: () => m.erro_deploy_falhou(),

  // /api/alcance/pareamento (Task 6) — candidato desconhecido, credencial de fábrica
  alcance_endereco_desconhecido: () => m.erro_alcance_endereco_desconhecido(),
  alcance_sem_credencial: () => m.erro_alcance_sem_credencial(),

  // /api/peers/descobrir — tailscale ausente, deslogado ou parado (não é "ninguém achado")
  descoberta_sem_tailscale: () => m.erro_descoberta_sem_tailscale(),

  // require_auth / require_loopback — 429 do backoff, 403 do loopback
  erro_so_loopback: () => m.erro_so_loopback(),

  // ===== /api/sessions — Task 11 =====

  // Existencia: sessao/pane/transcript que nao existe (shell, rename, loop, history,
  // workflows, subagents, events, uploads, plan, file, model/options, engine/model)
  erro_sessao_inexistente: () => m.erro_sessao_inexistente(),
  erro_sessao_nao_encontrada_detalhe: (p) => m.erro_sessao_nao_encontrada_detalhe({ detalhe: String(p.detalhe) }),
  erro_sessao_recado_nao_enfileirado: () => m.erro_sessao_recado_nao_enfileirado(),
  erro_sessao_opcao_nao_enviada: () => m.erro_sessao_opcao_nao_enviada(),
  erro_opcao_nao_convergiu: (p) => m.erro_opcao_nao_convergiu({ detalhe: String(p.detalhe) }),
  erro_opcao_fora_da_lista: () => m.erro_opcao_fora_da_lista(),
  erro_lista_indisponivel: (p) => m.erro_lista_indisponivel({ detalhe: String(p.detalhe) }),
  erro_mux_indisponivel: (p) => m.erro_mux_indisponivel({ detalhe: String(p.detalhe) }),
  erro_workflow_inexistente: () => m.erro_workflow_inexistente(),
  erro_agente_inexistente: () => m.erro_agente_inexistente(),
  erro_subagente_inexistente: () => m.erro_subagente_inexistente(),
  erro_transcript_ausente: () => m.erro_transcript_ausente(),

  // Conflito: nome em uso, sessao viva, pane ocupado
  erro_nome_em_uso: () => m.erro_nome_em_uso(),
  erro_nome_invalido: () => m.erro_nome_invalido(),
  sessao_falha_renomear: () => m.sessao_falha_renomear(),
  erro_encadeamento_proprio: () => m.erro_encadeamento_proprio(),
  erro_sessao_tmux_em_uso: (p) => m.erro_sessao_tmux_em_uso({ nome: String(p.nome) }),
  erro_shell_criacao_falhou: () => m.erro_shell_criacao_falhou(),
  erro_peer_nao_informado: () => m.erro_peer_nao_informado(),
  erro_autopareamento: () => m.erro_autopareamento(),
  erro_grupo_sem_conversa: () => m.erro_grupo_sem_conversa(),
  erro_grupo_limpeza_falhou: () => m.erro_grupo_limpeza_falhou(),
  erro_grupo_indisponivel: (p) => m.erro_grupo_indisponivel({ detalhe: String(p.detalhe) }),
  erro_initiator_invalido: () => m.erro_initiator_invalido(),
  erro_pareamento_cross_1_1: () => m.erro_pareamento_cross_1_1(),
  erro_pareamento_server_id_ausente: () => m.erro_pareamento_server_id_ausente(),
  erro_pareamento_desfeito: (p) => m.erro_pareamento_desfeito({ avisos: fmtParam(p.avisos) }),
  erro_pareamento_mistura_cross: () => m.erro_pareamento_mistura_cross(),
  erro_peer_nao_avisado: (p) => m.erro_peer_nao_avisado({ peer: fmtParam(p.peer) }),
  erro_par_limpeza_falhou: (p) => m.erro_par_limpeza_falhou({ peer: fmtParam(p.peer) }),
  erro_pareamento_nao_confirmado: (p) => m.erro_pareamento_nao_confirmado({ srv: String(p.srv), erro: String(p.erro) }),
  erro_pareamento_rejeitado: (p) => m.erro_pareamento_rejeitado({ erro: fmtParam(p.erro) }),
  erro_pareamento_aviso_falhou: (p) => m.erro_pareamento_aviso_falhou({ nome: String(p.nome), erro: fmtParam(p.erro) }),
  erro_pareamento_aviso_parcial: (p) => m.erro_pareamento_aviso_parcial({ avisos: fmtParam(p.avisos) }),
  erro_pareamento_aviso_local: (p) => m.erro_pareamento_aviso_local({ sessao: fmtParam(p.sessao), erro: fmtParam(p.erro) }),
  erro_pareamento_aviso_unpair: (p) => m.erro_pareamento_aviso_unpair({ sessao: fmtParam(p.sessao), erro: fmtParam(p.erro) }),
  erro_pareamento_grupo_falha: (p) => m.erro_pareamento_grupo_falha({ avisos: fmtParam(p.avisos) }),
  erro_pareamento_saida_falhou: (p) => m.erro_pareamento_saida_falhou({ avisos: fmtParam(p.avisos) }),
  erro_fila_entrada_nao_encontrada: () => m.erro_fila_entrada_nao_encontrada(),
  erro_pareamento_tarefa_existente: (p) => m.erro_pareamento_tarefa_existente({ existente: fmtParam(p.existente) }),

  // Envio: falhas fixas dos helpers _send_one/_send_one_codex, agora envelopadas
  erro_envio_incompleto_limpo: () => m.erro_envio_incompleto_limpo(),
  erro_envio_incompleto_composer: () => m.erro_envio_incompleto_composer(),
  erro_fila_nao_digitada: () => m.erro_fila_nao_digitada(),
  erro_fila_nao_entregue: () => m.erro_fila_nao_entregue(),
  erro_envio_falhou_desconhecida: () => m.erro_envio_falhou_desconhecida(),
  erro_envio_falhou: (p) => m.erro_envio_falhou({ erro: fmtParam(p.erro) }),
  erro_comando_nao_executado: (p) => m.erro_comando_nao_executado({ comando: fmtParam(p.comando), motivo: fmtParam(p.motivo) }),
  erro_group_message_slash: () => m.erro_group_message_slash(),
  erro_group_message_resposta: () => m.erro_group_message_resposta(),
  erro_group_message_tempestade: (p) => m.erro_group_message_tempestade({ max: fmtParam(p.max), janela: fmtParam(p.janela) }),
  erro_sessao_sem_grupo: () => m.erro_sessao_sem_grupo(),
  erro_sessao_nao_pareada: () => m.erro_sessao_nao_pareada(),

  // Orquestração (política de contas + papéis do grupo)
  erro_orq_conta_desconhecida: () => m.erro_orq_conta_desconhecida(),
  erro_orq_modelo_desconhecido: (p) => m.erro_orq_modelo_desconhecido({ modelos: fmtParam(p.modelos) }),
  erro_orq_celula_invalida: () => m.erro_orq_celula_invalida(),
  erro_orq_arquivo_mudou: () => m.erro_orq_arquivo_mudou(),
  erro_orq_conta_nao_liberada: () => m.erro_orq_conta_nao_liberada(),
  erro_orq_modelo_nao_liberado: () => m.erro_orq_modelo_nao_liberado(),
  erro_orq_conta_travada: () => m.erro_orq_conta_travada(),
  erro_orq_esforco_invalido: () => m.erro_orq_esforco_invalido(),
  erro_orq_headless_provider: () => m.erro_orq_headless_provider(),
  erro_orq_headless_read_only: () => m.erro_orq_headless_read_only(),
  erro_sessao_orq: () => m.erro_sessao_orq(),

  // Estado errado: terminal aberto, sessao trabalhando, loop ativo
  erro_terminal_aberto: () => m.erro_terminal_aberto(),
  erro_terminal_indisponivel: (p) => m.erro_terminal_indisponivel({ detalhe: String(p.detalhe) }),
  erro_btw_so_claude: () => m.erro_btw_so_claude(),
  erro_btw_sem_conversa: () => m.erro_btw_sem_conversa(),
  erro_btw_fork_falhou: () => m.erro_btw_fork_falhou(),
  erro_btw_vazia: () => m.erro_btw_vazia(),
  erro_btw_nao_abriu: () => m.erro_btw_nao_abriu(),
  erro_btw_sem_resposta: () => m.erro_btw_sem_resposta(),
  erro_btw_ilegivel: () => m.erro_btw_ilegivel(),
  erro_btw_composer_ocupado: () => m.erro_btw_composer_ocupado(),
  erro_btw_fechado: () => m.erro_btw_fechado(),
  erro_btw_nao_digitou: () => m.erro_btw_nao_digitou(),
  erro_btw_barra_perdida: () => m.erro_btw_barra_perdida(),
  erro_btw_overlay_aberto: () => m.erro_btw_overlay_aberto(),
  erro_btw_composer_ilegivel: () => m.erro_btw_composer_ilegivel(),
  erro_btw_enter_nao_enviado: () => m.erro_btw_enter_nao_enviado(),
  erro_terminal_invalido: (p) => m.erro_terminal_invalido({ nome: String(p.nome) }),
  erro_terminal_ausente: () => m.erro_terminal_ausente(),
  erro_terminal_abertura_falhou: (p) => m.erro_terminal_abertura_falhou({ erro: String(p.erro) }),
  erro_terminal_saiu_cedo: (p) => m.erro_terminal_saiu_cedo({ saida: String(p.saida) }),
  erro_loop_provider_invalido: () => m.erro_loop_provider_invalido(),
  erro_loop_ja_ativo: () => m.erro_loop_ja_ativo(),
  erro_loop_branch_invalida: (p) => m.erro_loop_branch_invalida({ br: String(p.br) }),
  erro_loop_inexistente: () => m.erro_loop_inexistente(),
  erro_loop_estado_errado: () => m.erro_loop_estado_errado(),
  erro_sem_confirmacao_resposta: () => m.erro_sem_confirmacao_resposta(),
  erro_sem_confirmacao_troca: () => m.erro_sem_confirmacao_troca(),

  // Entrada invalida: nome, caminho, modelo, motor
  erro_config_dir_invalido: () => m.erro_config_dir_invalido(),
  erro_rollout_sem_id: () => m.erro_rollout_sem_id(),
  erro_motor_sem_claude: () => m.erro_motor_sem_claude(),
  erro_subagente_so_claude: () => m.erro_subagente_so_claude(),
  erro_conta_reconciliacao_falhou: (p) => m.erro_conta_reconciliacao_falhou({ nome_conta: String(p.nome_conta), erro: String(p.erro) }),
  erro_cwd_indisponivel: () => m.erro_cwd_indisponivel(),
  erro_cwd_inexistente: (p) => m.erro_cwd_inexistente({ cwd: String(p.cwd) }),
  erro_arquivo_grande: () => m.erro_arquivo_grande(),
  erro_upload_inexistente: () => m.erro_upload_inexistente(),
  erro_arquivo_caminho_convidado: () => m.erro_arquivo_caminho_convidado(),
  erro_sem_plano_ativo: () => m.erro_sem_plano_ativo(),
  erro_sem_pasta_planos: () => m.erro_sem_pasta_planos(),
  erro_nome_plano_invalido: (p) => m.erro_nome_plano_invalido({ nome: String(p.nome) }),
  erro_plano_nao_encontrado: (p) => m.erro_plano_nao_encontrado({ nome: String(p.nome) }),
  erro_gravar_pin: (p) => m.erro_gravar_pin({ erro: String(p.erro) }),
  erro_marcar_step: (p) => m.erro_marcar_step({ erro: String(p.erro) }),
  erro_arquivar_plano: (p) => m.erro_arquivar_plano({ erro: String(p.erro) }),
  erro_editor_falhou: (p) => m.erro_editor_falhou({ binario: String(p.binario), erro: String(p.erro) }),
  erro_arquivo_nao_citado: () => m.erro_arquivo_nao_citado(),
  erro_caminho_fora_cwd: () => m.erro_caminho_fora_cwd(),
  erro_arquivo_nao_encontrado: () => m.erro_arquivo_nao_encontrado(),

  // /api/sessions/{name}/files/* e /git/path-diff — arvore de arquivos, busca e diff por caminho
  erro_arq_caminho_invalido: () => m.erro_arq_caminho_invalido(),
  erro_arq_fora_da_raiz: () => m.erro_arq_fora_da_raiz(),
  erro_arq_area_do_git: () => m.erro_arq_area_do_git(),
  erro_arq_inexistente: () => m.erro_arq_inexistente(),
  erro_arq_lista_falhou: () => m.erro_arq_lista_falhou(),
  erro_arq_e_pasta: () => m.erro_arq_e_pasta(),
  erro_arq_nao_e_pasta: () => m.erro_arq_nao_e_pasta(),
  erro_arq_nao_e_arquivo: () => m.erro_arq_nao_e_arquivo(),
  erro_arq_binario: () => m.erro_arq_binario(),
  erro_arq_mudou_no_disco: () => m.erro_arq_mudou_no_disco(),
  erro_arq_sumiu: () => m.erro_arq_sumiu(),
  erro_arq_escrita_falhou: () => m.erro_arq_escrita_falhou(),
  erro_arq_sem_digest: () => m.erro_arq_sem_digest(),
  erro_arq_grande_demais: () => m.erro_arq_grande_demais(),
  erro_arq_salvar_falhou: () => m.erro_arq_salvar_falhou(),
  erro_arq_sem_permissao: () => m.erro_arq_sem_permissao(),
  erro_arq_nao_e_repo_git: () => m.erro_arq_nao_e_repo_git(),
  erro_arq_busca_vazia: () => m.erro_arq_busca_vazia(),
  erro_arq_busca_falhou: (p) => m.erro_arq_busca_falhou({ msg: String(p.msg) }),
  // Git/arquivos que o Rust não rodou: 503 com o motivo, nunca repassado ao Python.
  workspace_busy: () => m.workspace_busy(),
  workspace_context: (p) => m.workspace_context({ motivo: String(p.motivo ?? '') }),
  workspace_unavailable: (p) => m.workspace_unavailable({ motivo: String(p.motivo ?? '') }),
  internal_info: () => m.history_internal_info(),
  costs_no_scopes: () => m.costs_no_scopes(),
  costs_no_disk: () => m.costs_no_disk(),
  costs_reader_panic: () => m.costs_reader_panic(),
  costs_sqlite: () => m.costs_sqlite(),
  costs_json: () => m.costs_json(),
  costs_worker_join: () => m.costs_worker_join(),
  costs_io: () => m.costs_io(),
  costs_non_finite: () => m.costs_non_finite(),
  history_io: () => m.history_io(),
  history_panic: () => m.history_panic(),
  workspace_request_too_large: () => m.workspace_request_too_large(),
  workspace_invalid_request: () => m.workspace_invalid_request(),
  erro_arq_modo_invalido: () => m.erro_arq_modo_invalido(),
  erro_git_diff: (p) => m.erro_git_diff({ msg: String(p.msg) }),

  // path-diff — motivo do escopo caido (PathDiff.motivo): o backend manda CODIGO (chave arq_*),
  // e a traducao e a mesma via do envelope; codigo desconhecido devolve undefined e a tela nao
  // desenha nada (FileViewer). Sem isto o motivo aparecia em portugues no app em ingles.
  arq_motivo_sem_commit: () => m.arq_motivo_sem_commit(),
  arq_motivo_sem_commit_proprio: () => m.arq_motivo_sem_commit_proprio(),
  arq_motivo_sem_base_conhecida: () => m.arq_motivo_sem_base_conhecida(),
  erro_rota_so_claude: () => m.erro_rota_so_claude(),
  erro_rota_so_motor: () => m.erro_rota_so_motor(),
  erro_rota_so_pi: () => m.erro_rota_so_pi(),
  erro_limits_so_codex: () => m.erro_limits_so_codex(),
  erro_models_so_codex: () => m.erro_models_so_codex(),
  erro_model_so_codex: () => m.erro_model_so_codex(),
  erro_model_effort_faltando: () => m.erro_model_effort_faltando(),
  erro_motor_ausente: (p) => m.erro_motor_ausente({ nome: String(p.nome) }),
  erro_provedor_offline: (p) => m.erro_provedor_offline({ nome: String(p.nome), erro: String(p.erro) }),
  erro_modelo_fora_catalogo: (p) => m.erro_modelo_fora_catalogo({ motor: String(p.motor), modelo: String(p.modelo) }),
  erro_permissao_so_claude: () => m.erro_permissao_so_claude(),
  erro_permissao_invalida: () => m.erro_permissao_invalida(),
  erro_permissao_leitura: () => m.erro_permissao_leitura(),
  erro_permissao_teto: (p) => m.erro_permissao_teto({ alvo: String(p.alvo), ficou: String(p.ficou) }),
  erro_sessao_trabalhando: () => m.erro_sessao_trabalhando(),
  erro_sessao_iniciando: () => m.erro_sessao_iniciando(),
  erro_sessao_esperando_resposta: () => m.erro_sessao_esperando_resposta(),
  erro_fila_pendente: () => m.erro_fila_pendente(),
  erro_modo_so_claude: () => m.erro_modo_so_claude(),
  erro_troca_modo: (p) => m.erro_troca_modo({ erro: String(p.erro ?? '') }),
  erro_conta_so_claude: () => m.erro_conta_so_claude(),
  erro_conta_cheia: (p) => m.erro_conta_cheia({ conta: String(p.conta ?? ''), pct: String(Math.round(Number(p.pct ?? 0))) }),
  erro_conversa_ja_na_conta: () => m.erro_conversa_ja_na_conta(),
  erro_troca_conta: (p) => m.erro_troca_conta({ erro: String(p.erro ?? '') }),
  erro_mover_conversa: (p) => m.erro_mover_conversa({ erro: String(p.erro ?? '') }),

  // Capacidade ausente: extensao do Pi, catalogo, resposta que nao veio
  erro_sem_pergunta_pi: () => m.erro_sem_pergunta_pi(),
  erro_sem_pergunta_kimi: () => m.erro_sem_pergunta_kimi(),
  erro_sem_resposta: () => m.erro_sem_resposta(),
  erro_drive_sem_fallback: (p) => m.erro_drive_sem_fallback({ erro: String(p.erro) }),
  erro_drive_fallback_falhou: (p) => m.erro_drive_fallback_falhou({ erro: fmtParam(p.erro) }),
  erro_resposta_nao_entregue: () => m.erro_resposta_nao_entregue(),
  erro_catalogo_pi_indisponivel: () => m.erro_catalogo_pi_indisponivel(),
  // Mesmo motivo do erro_omp_list_models: codigo proprio pra sessao omp mostrar "feche e reabra"
  // em vez da instrucao do Pi ("reinicie a sessao").
  erro_catalogo_omp_indisponivel: () => m.erro_catalogo_omp_indisponivel(),
  erro_pi_recusou_troca: (p) => m.erro_pi_recusou_troca({ provider: String(p.provider), id: String(p.id), thinking: String(p.thinking) }),
  erro_reinicio_indisponivel: () => m.erro_reinicio_indisponivel(),
  erro_atualizacao_branch: (p) => m.erro_atualizacao_branch({ alvo: String(p.alvo ?? 'main') }),
  erro_atualizacao_dependencia: (p) => m.erro_atualizacao_dependencia({ faltando: fmtParam(p.faltando) }),
  erro_atualizacao_ja_rodando: () => m.erro_atualizacao_ja_rodando(),

  // Passagem de bastao (POST /api/sessions/{name}/bastao). O `erro_bastao_fila` e o que mais
  // importa traduzir: e o unico em que a sessao JA nasceu e o conserto e humano — a frase tem
  // que carregar o nome dela e o caminho do dossie, senao quem le acha que nada aconteceu.
  erro_bastao_para_si_mesma: () => m.erro_bastao_para_si_mesma(),
  erro_bastao_sem_cwd: () => m.erro_bastao_sem_cwd(),
  erro_bastao_cwd_inexistente: (p) => m.erro_bastao_cwd_inexistente({ cwd: String(p.cwd) }),
  erro_bastao_sem_dossie: () => m.erro_bastao_sem_dossie(),
  erro_bastao_gravar: (p) => m.erro_bastao_gravar({ motivo: String(p.motivo ?? '') }),
  erro_bastao_fila: (p) => m.erro_bastao_fila({ nome: String(p.nome), dossie: String(p.dossie) }),

  // Configuração compartilhada (/api/config-sync).
  config_sync_unknown_item: (p) => m.config_sync_unknown_item({ items: String(p.items ?? '') }),
  config_sync_bundle_too_big: (p) => m.config_sync_bundle_too_big({ largest: String(p.largest ?? '') }),
  config_sync_busy: () => m.config_sync_busy(),
  config_sync_invalid_keys: () => m.config_sync_invalid_keys(),
  config_sync_invalid_bundle: () => m.config_sync_invalid_bundle(),
  config_sync_version: () => m.config_sync_version(),
  // Atalhos do projeto (/api/sessions/{name}/project-shortcuts e shortcut-shell).
  erro_project_shortcuts: (p) => m.erro_project_shortcuts({ detalhe: String(p.detalhe ?? '') }),
  erro_project_shortcuts_projeto: (p) => m.erro_project_shortcuts_projeto({ detalhe: String(p.detalhe ?? '') }),
  erro_project_shortcuts_arquivo: (p) => m.erro_project_shortcuts_arquivo({ detalhe: String(p.detalhe ?? '') }),
  erro_shortcut_pasta: (p) => m.erro_shortcut_pasta({ detalhe: String(p.detalhe ?? '') }),
  erro_shortcut_hangar_convidado: () => m.erro_shortcut_hangar_convidado(),
  erro_run_code_longo: () => m.erro_run_code_longo(),
  erro_run_code_invalido: () => m.erro_run_code_invalido(),
  erro_run_code_linguagem: () => m.erro_run_code_linguagem(),
  erro_run_code_shell_incompativel: (p) => m.erro_run_code_shell_incompativel({ linguagem: String(p.linguagem ?? ''), sistema: String(p.sistema ?? '') }),
  erro_run_code_shell_ausente: (p) => m.erro_run_code_shell_ausente({ linguagem: String(p.linguagem ?? '') }),
  erro_run_code_terminal: () => m.erro_run_code_terminal(),
  // Convidados com login próprio (guest_user_gate, guest_users_api, hub /api/sync/guests).
  erro_fora_do_convidado: () => m.erro_fora_do_convidado(),
  erro_fora_da_pasta: () => m.erro_fora_da_pasta(),
  erro_pasta_inexistente: () => m.erro_pasta_inexistente(),
  erro_convidado_inexistente: () => m.erro_convidado_inexistente(),
  erro_convidados_ilegivel: () => m.erro_convidados_ilegivel(),
  erro_so_dono: () => m.erro_so_dono(),
  erro_usuario_em_uso: () => m.erro_usuario_em_uso(),
  // Botões e painéis dos mods do Claude Code (/api/sessions/{name}/plugin/press e plugin/close).
  erro_mod_mouse_desligado: () => m.erro_mod_mouse_desligado(),
  erro_mod_botao_nao_achado: (p) => m.erro_mod_botao_nao_achado({ rotulo: String(p.rotulo ?? '') }),
  erro_mod_botao_ambiguo: (p) => m.erro_mod_botao_ambiguo({ rotulo: String(p.rotulo ?? '') }),
  erro_mod_clique_sem_resposta: () => m.erro_mod_clique_sem_resposta(),
  erro_mod_botao_inexistente: () => m.erro_mod_botao_inexistente(),
  erro_mod_terminal_em_modo: () => m.erro_mod_terminal_em_modo(),
  erro_mod_sem_digitacao: () => m.erro_mod_sem_digitacao(),
  erro_mod_desenho_vencido: () => m.erro_mod_desenho_vencido(),
  erro_mod_dialogo_aberto: () => m.erro_mod_dialogo_aberto(),
  erro_mod_rascunho_no_prompt: () => m.erro_mod_rascunho_no_prompt(),
  erro_mod_painel_nao_alcancavel: () => m.erro_mod_painel_nao_alcancavel(),
  erro_mod_fechar_recusado: () => m.erro_mod_fechar_recusado(),
  erro_mod_guarda_indisponivel: () => m.erro_mod_guarda_indisponivel(),
  erro_mod_painel_inexistente: () => m.erro_mod_painel_inexistente(),
  erro_mod_convidado: () => m.erro_mod_convidado(),
  // Troca de agente em curso: o Python recusa com este código, e o Rust o repassa nas rotas dos mods.
  session_transfer_busy: () => m.session_transfer_busy(),
  session_transfer_gate_unavailable: () => m.session_transfer_gate_unavailable(),
};

// Falha com código do servidor (503 do dono único): a frase traduzida já está no `message`.
// Erro sem código (rede, HTTP sem envelope) fica com a frase genérica de quem chama.
export function motivoDoServidor(e: unknown): string | null {
  return e instanceof Error && typeof (e as { code?: unknown }).code === 'string' && e.message ? e.message : null;
}

export function mensagemDeErro(code: string, params: Parametros = {}): string | undefined {
  // Propriedade PROPRIA, nunca a leitura crua: nome herdado do prototipo (toString, constructor,
  // hasOwnProperty, __proto__) existe em todo objeto e chamaria a funcao errada — medido:
  // `ERROS['toString']` devolve a funcao herdada e a chamada retorna '[object Undefined]'.
  if (!Object.prototype.hasOwnProperty.call(ERROS, code)) return undefined;
  return ERROS[code](params);
}
