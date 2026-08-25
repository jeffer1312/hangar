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
    return mensagemDeErro(env.code, env.params ?? {}) ?? env.msg ?? env.code;
  }
  return undefined;
}

const ERROS: Record<string, (params: Parametros) => string> = {
  // /api/claude-configs — apagar conta recusado por alguma condicao da maquina
  erro_config_dirs_fixo: () => m.erro_config_dirs_fixo(),
  erro_conta_ativa_backend: () => m.erro_conta_ativa_backend(),
  erro_conta_lista_fixa: () => m.erro_conta_lista_fixa(),
  erro_conta_nome_invalido: () => m.erro_conta_nome_invalido(),
  erro_conta_inexistente: (p) => m.erro_conta_inexistente({ nome: String(p.nome) }),
  erro_login_ja_em_curso: () => m.erro_login_ja_em_curso(),
  erro_login_sem_tentativa: () => m.erro_login_sem_tentativa(),
  erro_login_timeout: () => m.erro_login_timeout(),
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
  erro_motor_nome_com_dados: () => m.erro_motor_nome_com_dados(),
  erro_motor_nome_ou_dados: () => m.erro_motor_nome_ou_dados(),

  // /api/model-options e POST /api/sessions — catalogo do Pi falhou ou provider fora de escopo
  erro_pi_list_models: (p) => m.erro_pi_list_models({ erro: String(p.erro) }),
  // Sem `{erro}` de proposito: o texto do backend aqui e a instrucao ("instale o Pi / ajuste o
  // PATH"), nao um errno pra repassar — o que a pessoa precisa ler ja esta na frase traduzida.
  erro_pi_ausente: () => m.erro_pi_ausente(),
  erro_provider_invalido: () => m.erro_provider_invalido(),
  // POST /api/ditado/relimpar — so aparece se o cliente mandar um estilo fora da lista (os botoes
  // da barra do ditado saem de estilosDitado, entao na pratica e defesa de contrato).
  erro_estilo_invalido: () => m.erro_estilo_invalido(),
  // POST /api/sessions aceita claude/codex/pi/kimi — o texto generico nao pode orientar so
  // claude/pi (contrato do erro_provider_invalido, que so vale pro /api/model-options)
  erro_provider_sessao_invalido: () => m.erro_provider_sessao_invalido(),

  // /api/archive — arquivo inexistente, path invalido, motor errado no resume
  // (erro_imagem_nao_encontrada tambem e usado por /api/sessions/{name}/transcript-image)
  erro_path_invalido: () => m.erro_path_invalido(),
  erro_projeto_nao_encontrado: () => m.erro_projeto_nao_encontrado(),
  erro_transcript_nao_encontrado: () => m.erro_transcript_nao_encontrado(),
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
  erro_initiator_invalido: () => m.erro_initiator_invalido(),
  erro_pareamento_cross_1_1: () => m.erro_pareamento_cross_1_1(),
  erro_pareamento_server_id_ausente: () => m.erro_pareamento_server_id_ausente(),
  erro_pareamento_desfeito: (p) => m.erro_pareamento_desfeito({ avisos: fmtParam(p.avisos) }),
  erro_pareamento_nao_confirmado: (p) => m.erro_pareamento_nao_confirmado({ srv: String(p.srv), erro: String(p.erro) }),
  erro_pareamento_rejeitado: (p) => m.erro_pareamento_rejeitado({ erro: fmtParam(p.erro) }),
  erro_pareamento_aviso_falhou: (p) => m.erro_pareamento_aviso_falhou({ nome: String(p.nome), erro: fmtParam(p.erro) }),
  erro_pareamento_aviso_parcial: (p) => m.erro_pareamento_aviso_parcial({ avisos: fmtParam(p.avisos) }),
  erro_pareamento_aviso_local: (p) => m.erro_pareamento_aviso_local({ sessao: fmtParam(p.sessao), erro: fmtParam(p.erro) }),
  erro_pareamento_aviso_unpair: (p) => m.erro_pareamento_aviso_unpair({ sessao: fmtParam(p.sessao), erro: fmtParam(p.erro) }),
  erro_pareamento_grupo_falha: (p) => m.erro_pareamento_grupo_falha({ avisos: fmtParam(p.avisos) }),
  erro_pareamento_saida_falhou: (p) => m.erro_pareamento_saida_falhou({ avisos: fmtParam(p.avisos) }),

  // Envio: falhas fixas dos helpers _send_one/_send_one_codex, agora envelopadas
  erro_envio_incompleto_limpo: () => m.erro_envio_incompleto_limpo(),
  erro_envio_incompleto_composer: () => m.erro_envio_incompleto_composer(),
  erro_fila_nao_digitada: () => m.erro_fila_nao_digitada(),
  erro_fila_nao_entregue: () => m.erro_fila_nao_entregue(),
  erro_envio_falhou_desconhecida: () => m.erro_envio_falhou_desconhecida(),
  erro_envio_falhou: (p) => m.erro_envio_falhou({ erro: fmtParam(p.erro) }),
  erro_group_message_slash: () => m.erro_group_message_slash(),
  erro_sessao_sem_grupo: () => m.erro_sessao_sem_grupo(),
  erro_sessao_nao_pareada: () => m.erro_sessao_nao_pareada(),

  // Estado errado: terminal aberto, sessao trabalhando, loop ativo
  erro_terminal_aberto: () => m.erro_terminal_aberto(),
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
  erro_resume_codex: () => m.erro_resume_codex(),
  erro_motor_sem_claude: () => m.erro_motor_sem_claude(),
  erro_conta_reconciliacao_falhou: (p) => m.erro_conta_reconciliacao_falhou({ nome_conta: String(p.nome_conta), erro: String(p.erro) }),
  erro_cwd_indisponivel: () => m.erro_cwd_indisponivel(),
  erro_arquivo_grande: () => m.erro_arquivo_grande(),
  erro_sem_plano_ativo: () => m.erro_sem_plano_ativo(),
  erro_sem_pasta_planos: () => m.erro_sem_pasta_planos(),
  erro_nome_plano_invalido: (p) => m.erro_nome_plano_invalido({ nome: String(p.nome) }),
  erro_plano_nao_encontrado: (p) => m.erro_plano_nao_encontrado({ nome: String(p.nome) }),
  erro_gravar_pin: (p) => m.erro_gravar_pin({ erro: String(p.erro) }),
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

  // Capacidade ausente: extensao do Pi, catalogo, resposta que nao veio
  erro_sem_pergunta_pi: () => m.erro_sem_pergunta_pi(),
  erro_sem_pergunta_kimi: () => m.erro_sem_pergunta_kimi(),
  erro_sem_resposta: () => m.erro_sem_resposta(),
  erro_drive_sem_fallback: (p) => m.erro_drive_sem_fallback({ erro: String(p.erro) }),
  erro_drive_fallback_falhou: (p) => m.erro_drive_fallback_falhou({ erro: fmtParam(p.erro) }),
  erro_catalogo_pi_indisponivel: () => m.erro_catalogo_pi_indisponivel(),
  erro_pi_recusou_troca: (p) => m.erro_pi_recusou_troca({ provider: String(p.provider), id: String(p.id), thinking: String(p.thinking) }),
};

export function mensagemDeErro(code: string, params: Parametros = {}): string | undefined {
  // Propriedade PROPRIA, nunca a leitura crua: nome herdado do prototipo (toString, constructor,
  // hasOwnProperty, __proto__) existe em todo objeto e chamaria a funcao errada — medido:
  // `ERROS['toString']` devolve a funcao herdada e a chamada retorna '[object Undefined]'.
  if (!Object.prototype.hasOwnProperty.call(ERROS, code)) return undefined;
  return ERROS[code](params);
}
