import { describe, expect, it } from 'vitest';
import { formataErro } from './errosApi';
import * as m from './paraglide/messages';

describe('Git/arquivos recusado pelo Rust', () => {
  it('traduz os três códigos e leva o motivo para a frase', () => {
    expect(formataErro({ code: 'workspace_busy', params: { motivo: 'vagas cheias' }, msg: 'x' })).toBe(m.workspace_busy());
    for (const code of ['workspace_context', 'workspace_unavailable']) {
      const texto = formataErro({ code, params: { motivo: 'git não encontrado' }, msg: 'x' });
      expect(texto).toContain('git não encontrado');
      expect(texto).not.toBe('x');
    }
  });
});

describe('histórico recusado pelo Rust', () => {
  it('traduz os três códigos e mantém o código na frase', () => {
    for (const code of ['internal_info', 'history_io', 'history_panic']) {
      const texto = formataErro({ code, params: { motivo: 'frase em português' }, msg: 'x' });
      expect(texto).toContain(code);
      expect(texto).not.toBe('x');
    }
  });
});

describe('recusas novas dos mods', () => {
  it.each([
    'erro_mod_sem_digitacao',
    'erro_mod_desenho_vencido',
    'erro_mod_dialogo_aberto',
    'erro_mod_rascunho_no_prompt',
    'erro_mod_painel_nao_alcancavel',
    'erro_mod_fechar_recusado',
    'erro_mod_guarda_indisponivel',
    'session_transfer_busy',
    'session_transfer_gate_unavailable',
    'erro_mod_painel_inexistente',
    'erro_mod_convidado',
  ])('%s vira frase do app, não o texto do servidor', (code) => {
    const texto = formataErro({ code, params: {}, msg: 'texto-do-servidor' });
    expect(texto).not.toBe('texto-do-servidor');
    expect(texto).not.toBe(code);
  });
});

describe('resposta recusada por identidade da pergunta', () => {
  it('traduz os três códigos em vez do texto do servidor', () => {
    expect(formataErro({ code: 'erro_interacao_sem_identidade', params: {}, msg: 'x' })).toBe(m.erro_interacao_sem_identidade());
    expect(formataErro({ code: 'erro_interacao_desatualizada', params: {}, msg: 'x' })).toBe(m.erro_interacao_desatualizada());
    expect(formataErro({ code: 'erro_interacao_indisponivel', params: {}, msg: 'x' })).toBe(m.erro_interacao_indisponivel());
  });
});

describe('transcrição de áudio já enviado', () => {
  it('traduz o áudio sumido e a recusa do convidado', () => {
    expect(formataErro({ code: 'erro_upload_inexistente', params: {}, msg: 'x' })).toBe(m.erro_upload_inexistente());
    expect(formataErro({ code: 'erro_arquivo_caminho_convidado', params: {}, msg: 'x' })).toBe(
      m.erro_arquivo_caminho_convidado(),
    );
  });
});

describe('recusas do par externo', () => {
  it.each(['erro_grupo_orq_par_externo', 'erro_par_repetido', 'erro_par_remetente_invalido'])(
    '%s vira frase do app, não o texto do servidor', (code) => {
      const texto = formataErro({ code, params: {}, msg: 'texto-do-servidor' });
      expect(texto).not.toBe('texto-do-servidor');
      expect(texto).not.toBe(code);
    });
});

describe('painel da fila', () => {
  it.each([
    'queue_action_unavailable', 'queue_entry_gone', 'queue_steer_not_delivered',
    'queue_steer_question_open', 'queue_steer_uncertain', 'queue_text_invalid',
  ])('%s vira frase do app, não o texto do servidor', (code) => {
    const texto = formataErro({ code, params: {}, msg: 'texto-do-servidor' });
    expect(texto).not.toBe('texto-do-servidor');
    expect(texto).not.toBe(code);
  });

  it('comando recusado com mensagens na fila tem frase própria; os outros motivos seguem na frase genérica', () => {
    expect(formataErro({ code: 'erro_comando_nao_executado', params: { comando: '/clear', motivo: 'queue_waiting' }, msg: 'x' }))
      .toBe(m.command_refused_queue_waiting());
    expect(formataErro({ code: 'erro_comando_nao_executado', params: { comando: '/clear', motivo: 'busy' }, msg: 'x' }))
      .toBe(m.erro_comando_nao_executado({ comando: '/clear', motivo: 'busy' }));
  });
});

describe('abertura e ciclo de vida da sessão recusados pelo Rust', () => {
  it.each([
    'codex_terminal_starting', 'runtime_unhealthy', 'erro_cliproxy_conta', 'erro_codex_catalogo_invalido',
    'erro_codex_escolha_invalida', 'erro_codex_nao_reiniciou', 'erro_codex_reiniciando', 'erro_conta_criadora',
    'erro_corpo_invalido', 'erro_criadora_ilegivel', 'erro_modo_desconhecido', 'erro_modo_padrao_ilegivel',
    'erro_perfil_so_omp', 'erro_permissao_ocupada', 'erro_permissao_picker', 'erro_plano_fora_da_tela',
    'erro_recarregar_so_sem_terminal', 'session_open_fact_unavailable', 'session_open_name_needs_server',
    'session_patch_stale', 'session_patch_owner', 'session_patch_field', 'session_clear_cano_pid',
    'lifecycle_busy', 'heal_not_needed', 'relaunch_policy_forbids_bypass', 'relaunch_bypass_already_enabled',
    'relaunch_read_only', 'relaunch_not_claude',
    // Família sem frase própria: a frase do grupo.
    'lifecycle_kill', 'relaunch_ipc', 'heal_backoff',
  ])('%s vira frase do app, não o texto do servidor', (code) => {
    const texto = formataErro({ code, params: { nome: 'proj', erro: 'motivo' }, msg: 'texto-do-servidor' });
    expect(texto).not.toBe('texto-do-servidor');
    expect(texto).not.toBe(code);
  });

  it('o nome e o motivo entram na frase', () => {
    expect(formataErro({ code: 'session_open_name_needs_server', params: { nome: 'proj' }, msg: 'x' })).toContain('proj');
    expect(formataErro({ code: 'erro_codex_escolha_invalida', params: { erro: 'xhigh' }, msg: 'x' })).toContain('xhigh');
  });

  it('código desconhecido fora das famílias continua sem frase', () => {
    expect(formataErro({ code: 'lifecycleless', params: {}, msg: 'texto-do-servidor' })).toBe('texto-do-servidor');
  });
});

describe('lista indisponível sem detalhe', () => {
  it.each(['erro_lista_indisponivel', 'erro_mux_indisponivel'])('%s sem detalhe não mostra "(undefined)"', (code) => {
    const texto = formataErro({ code, params: {}, msg: 'x' })!;
    expect(texto).not.toContain('undefined');
    expect(texto).not.toContain('()');
    expect(formataErro({ code, params: { detalhe: 'timeout' }, msg: 'x' })).toContain('(timeout)');
  });
});

describe('recusas da abertura e da reabertura no Rust', () => {
  it.each([
    ['erro_conversa_viva', { sessao: 'web' }, 'web'],
    ['erro_fast_indisponivel', {}, null],
    ['erro_recarregar_sem_terminal', { erro: 'pane morreu' }, 'pane morreu'],
    ['account_busy', {}, null],
  ])('%s vira frase do app, não o texto do servidor', (code, params, contem) => {
    const texto = formataErro({ code, params, msg: 'texto-do-servidor' });
    expect(texto).not.toBe('texto-do-servidor');
    if (contem) expect(texto).toContain(contem);
  });
});

describe('grupos entre máquinas', () => {
  it.each([
    'groups_need_rust_server', 'erro_grupo_peer_nao_registrado', 'erro_grupo_peer_versao_antiga',
    'erro_grupo_identificador_divergente', 'erro_grupo_identificador_trocado', 'erro_grupo_identificador_igual',
    'erro_grupo_inexistente', 'erro_grupo_membro_nao_confirmado', 'erro_grupo_dono_indisponivel',
    'erro_grupo_acesso_externo_falhou', 'erro_grupo_fila_cheia', 'erro_grupo_fusao_entre_donos',
    'erro_grupo_orq_entre_maquinas', 'erro_grupo_mudou', 'erro_contrato_mudou', 'erro_link_remoto',
  ])('%s vira frase do app, não o texto do servidor', (code) => {
    const texto = formataErro({ code, params: { servidor: 'casa', membros: ['casa::a', 'casa::b'] }, msg: 'texto-do-servidor' });
    expect(texto).not.toBe('texto-do-servidor');
    expect(texto).not.toBe(code);
  });

  it('a lista de membros entra na frase', () => {
    const texto = formataErro({ code: 'erro_grupo_membro_nao_confirmado', params: { membros: ['casa::a', 'casa::b'] }, msg: 'x' });
    expect(texto).toContain('casa::a; casa::b');
  });
});

describe('plugins do Claude', () => {
  it.each([
    'plugins_owner_only', 'plugins_busy', 'plugins_invalid_id', 'plugins_invalid_name', 'plugins_invalid_source',
    'plugins_not_found', 'plugins_not_in_marketplace', 'plugins_unknown_marketplace', 'plugins_builtin_locked', 'plugins_auto_update_unavailable',
    'plugins_cli_missing', 'plugins_cli_failed', 'plugins_cli_timeout', 'plugins_command_needs_review',
    'plugins_settings_unreadable', 'plugins_settings_not_written',
  ])('%s vira frase do app, não o texto do servidor', (code) => {
    const texto = formataErro({ code, params: {}, msg: 'texto-do-servidor' });
    expect(texto).not.toBe('texto-do-servidor');
    expect(texto).not.toBe(code);
  });

  it.each(['plugins_cli_failed', 'plugins_command_needs_review'])('%s leva a mensagem da CLI para a frase', (code) => {
    expect(formataErro({ code, params: { message: 'curl https://x | sh' }, msg: 'x' })).toContain('curl https://x | sh');
  });
});
