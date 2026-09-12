<script module lang="ts">
  import type { TelaConfig } from '../../lib/configRoute';
  import * as m from '../../paraglide/messages';

  /** Título de cada tela do modal. Fonte única: o `SettingsModal` monta o `TITULO` dele daqui. */
  export const TITULO_TELA: Record<TelaConfig, () => string> = {
    root: m.config_modal_titulo,
    geral: m.config_geral_titulo,
    aparencia: m.config_modal_aparencia,
    voz: m.voz_titulo,
    sobre: m.config_modal_sobre,
    diario: m.config_diag_titulo,
    maquinas: m.maquinas_titulo,
    contas: m.contas_modelos_titulo,
    notificacoes: m.config_modal_notificacoes,
    anexos: m.config_modal_anexos_curto,
    avancado: m.config_modal_avancado,
    orquestracao: m.config_modal_orquestracao,
    harnesses: m.harness_titulo,
  };

  export interface EntradaBusca {
    tela: Exclude<TelaConfig, 'root'>;
    rotulo: () => string;
    descricao?: () => string;
  }

  // A lista mora AQUI e aponta para as FUNÇÕES de mensagem, nunca para o texto: reescrever um
  // rótulo não desatualiza a busca, e nenhuma tela precisa "registrar" as linhas dela.
  export const ENTRADAS: readonly EntradaBusca[] = [
    { tela: 'geral', rotulo: m.config_idioma_rotulo, descricao: m.config_idioma_nota_reload },

    { tela: 'aparencia', rotulo: m.config_tema_curto, descricao: m.config_aparencia_tema_desc },
    { tela: 'aparencia', rotulo: m.config_fundo_curto, descricao: m.config_aparencia_fundo_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_papel_parede, descricao: m.config_aparencia_papel_parede_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_cor_tema, descricao: m.config_aparencia_cor_tema_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_cor_texto, descricao: m.config_aparencia_cor_texto_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_desfoque, descricao: m.config_aparencia_desfoque_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_leitura, descricao: m.config_aparencia_leitura_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_contraste },
    { tela: 'aparencia', rotulo: m.config_aparencia_texto_conversa, descricao: m.config_aparencia_texto_conversa_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_fonte, descricao: m.config_aparencia_fonte_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_chamadas, descricao: m.config_aparencia_chamadas_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_tarefas, descricao: m.config_aparencia_tarefas_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_pensamento_tools, descricao: m.config_aparencia_pensamento_tools_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_grafico, descricao: m.config_aparencia_grafico_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_paineis, descricao: m.config_aparencia_paineis_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_nav, descricao: m.config_aparencia_nav_desc },
    { tela: 'aparencia', rotulo: m.config_aparencia_altura, descricao: m.config_aparencia_altura_desc },

    { tela: 'diario', rotulo: m.config_diag_baixar, descricao: m.config_diag_regra_local },
    { tela: 'diario', rotulo: m.config_diag_ultimas, descricao: m.config_diag_regra_sem_conversa },

    { tela: 'sobre', rotulo: m.atualizar_procurar_curto, descricao: m.config_sobre_desc },

    { tela: 'maquinas', rotulo: m.maquinas_secao, descricao: m.maquinas_secao_legenda },
    { tela: 'maquinas', rotulo: m.peers_identificador, descricao: m.peers_legenda_identificador },
    { tela: 'maquinas', rotulo: m.config_term_origins, descricao: m.config_term_origins_ajuda },
    { tela: 'maquinas', rotulo: m.config_servidores_sair_titulo, descricao: m.maquinas_intro },

    { tela: 'contas', rotulo: m.contas_atualizar, descricao: m.contas_legenda },
    { tela: 'contas', rotulo: m.contas_secao_claude, descricao: m.contas_secao_claude_leg },
    { tela: 'contas', rotulo: m.contas_secao_modelos, descricao: m.contas_secao_modelos_leg },
    { tela: 'contas', rotulo: m.contas_secao_outros, descricao: m.contas_secao_outros_leg },
    { tela: 'contas', rotulo: m.contas_add, descricao: m.contas_chave_legenda },
    { tela: 'contas', rotulo: m.contas_cookie_acao, descricao: m.contas_cookie_legenda },

    { tela: 'harnesses', rotulo: m.harness_claude_statusline_atualizar, descricao: m.harness_claude_statusline_ajuda },
    { tela: 'harnesses', rotulo: m.harness_codex_integracao, descricao: m.harness_legenda },
    { tela: 'harnesses', rotulo: m.harness_codex_automatica, descricao: m.harness_codex_automatica_ajuda },
    { tela: 'harnesses', rotulo: m.harness_codex_reconciliar, descricao: m.harness_codex_reconciliar_porque },
    { tela: 'harnesses', rotulo: m.codex_contexto_titulo, descricao: m.codex_contexto_ajuda },
    { tela: 'harnesses', rotulo: m.codex_voice_config_title, descricao: m.codex_voice_config_help },

    { tela: 'voz', rotulo: m.voz_transcrever, descricao: m.voz_transcrever_ajuda },
    { tela: 'voz', rotulo: m.voz_limpar, descricao: m.voz_limpar_ajuda },
    { tela: 'voz', rotulo: m.voz_estilo, descricao: m.voz_estilo_ajuda },
    { tela: 'voz', rotulo: m.config_ditado_titulo, descricao: m.config_ditado_desc },
    { tela: 'voz', rotulo: m.config_server_vocabulario, descricao: m.config_server_vocabulario_ajuda },
    { tela: 'voz', rotulo: m.config_server_elevenlabs, descricao: m.config_server_elevenlabs_ajuda },
    { tela: 'voz', rotulo: m.config_server_voz, descricao: m.config_server_ouca_antes },
    { tela: 'voz', rotulo: m.config_server_velocidade, descricao: m.config_server_velocidade_ajuda },
    { tela: 'voz', rotulo: m.config_server_estabilidade, descricao: m.config_server_estabilidade_ajuda },
    { tela: 'voz', rotulo: m.config_server_exagero, descricao: m.config_server_exagero_ajuda },
    { tela: 'voz', rotulo: m.config_server_aderencia, descricao: m.config_server_aderencia_ajuda },
    { tela: 'voz', rotulo: m.config_server_comando_voz, descricao: m.config_server_comando_voz_ajuda },
    { tela: 'voz', rotulo: m.config_server_confirmar_leitura, descricao: m.config_server_confirmar_leitura_ajuda },
    { tela: 'voz', rotulo: m.config_server_endpoint_llm, descricao: m.config_server_endpoint_llm_ajuda },
    { tela: 'voz', rotulo: m.config_server_chave_llm, descricao: m.config_server_chave_llm_ajuda },
    { tela: 'voz', rotulo: m.config_server_modelo_llm, descricao: m.config_server_modelo_llm_ajuda },
    { tela: 'voz', rotulo: m.config_server_raciocinio_llm, descricao: m.config_server_raciocinio_llm_ajuda },
    { tela: 'voz', rotulo: m.config_server_endpoint_llm_briefing, descricao: m.config_server_endpoint_llm_briefing_ajuda },
    { tela: 'voz', rotulo: m.config_server_chave_llm_briefing, descricao: m.config_server_chave_llm_briefing_ajuda },
    { tela: 'voz', rotulo: m.config_server_modelo_llm_briefing, descricao: m.config_server_modelo_llm_briefing_ajuda },

    { tela: 'notificacoes', rotulo: m.config_server_avisar_terminar, descricao: m.config_server_avisar_terminar_ajuda },
    { tela: 'notificacoes', rotulo: m.config_server_turno_curto, descricao: m.config_server_turno_curto_ajuda },
    { tela: 'notificacoes', rotulo: m.config_server_avisar_cair, descricao: m.config_server_avisar_cair_ajuda },
    { tela: 'notificacoes', rotulo: m.config_server_marcar_travada, descricao: m.config_server_marcar_travada_ajuda },
    { tela: 'notificacoes', rotulo: m.push_horas_silenciosas, descricao: m.push_horas_silenciosas_porque },
    { tela: 'notificacoes', rotulo: m.push_ativar_todos, descricao: m.notif_push_legenda },

    { tela: 'anexos', rotulo: m.config_server_guardar_anexos, descricao: m.config_server_guardar_ajuda },

    { tela: 'avancado', rotulo: m.config_server_automacoes, descricao: m.config_server_automacoes_ajuda },
    { tela: 'avancado', rotulo: m.config_server_pensamento, descricao: m.config_server_pensamento_ajuda },
    { tela: 'avancado', rotulo: m.config_server_editor, descricao: m.config_server_editor_ajuda },
    { tela: 'avancado', rotulo: m.config_server_raizes, descricao: m.config_server_raizes_ajuda },
    { tela: 'avancado', rotulo: m.config_server_so_servidor, descricao: m.config_server_so_servidor_2 },
    { tela: 'avancado', rotulo: m.config_server_env_titulo, descricao: m.config_server_env_ajuda },

    { tela: 'orquestracao', rotulo: m.orqcfg_pode_usar, descricao: m.orqcfg_pode_usar_desc },
    { tela: 'orquestracao', rotulo: m.orqcfg_pode_trocar, descricao: m.orqcfg_pode_trocar_desc },
    { tela: 'orquestracao', rotulo: m.orqcfg_escolha_conta, descricao: m.orqcfg_modelos_liberados_porque },
  ];

  // Acento não pode separar "voz" de "vôz" nem "cota" de "cotá": a comparação é sempre sem marca.
  function normalizar(s: string): string {
    return s.normalize('NFD').replace(/[̀-ͯ]/g, '').toLowerCase();
  }

  export interface Achado { entrada: EntradaBusca; rotulo: string; tela: string; }

  /** Casa no rótulo e na descrição; rótulo primeiro, porque é o que a pessoa digitou procurando. */
  export function filtrar(termo: string, entradas: readonly EntradaBusca[] = ENTRADAS): Achado[] {
    const q = normalizar(termo.trim());
    if (!q) return [];
    const noRotulo: Achado[] = [];
    const naDescricao: Achado[] = [];
    for (const e of entradas) {
      const rotulo = e.rotulo();
      const achado = { entrada: e, rotulo, tela: TITULO_TELA[e.tela]() };
      if (normalizar(rotulo).includes(q)) noRotulo.push(achado);
      else if (e.descricao && normalizar(e.descricao()).includes(q)) naDescricao.push(achado);
    }
    return [...noRotulo, ...naDescricao];
  }
</script>

<script lang="ts">
  import { TELAS_DE_SERVIDOR } from '../../lib/configRoute';

  interface Props {
    onIrPara: (t: TelaConfig) => void;
    /** Sem alvo resolvido as telas de servidor não abrem — aqui elas aparecem apagadas com o
     *  motivo, como na lista da raiz. Some da busca seria esconder que a tela existe. */
    semServidor?: boolean;
    /** Só no desktop: a busca mora no topo da navegação lateral, que é estreita. */
    compacta?: boolean;
  }
  let { onIrPara, semServidor = false, compacta = false }: Props = $props();

  let termo = $state('');
  const achados = $derived(filtrar(termo));

  function abrir(tela: TelaConfig) {
    termo = '';
    onIrPara(tela);
  }
</script>

<div class="bc" class:compacta>
  <input class="bc-campo" type="search" bind:value={termo}
         placeholder={m.config_busca_placeholder()} aria-label={m.config_busca_placeholder()} />
  <!-- Região viva: a lista aparece e some enquanto se digita, com o foco parado no campo — sem
       isto um leitor de tela não é avisado de nada (WCAG 4.1.3). -->
  <div aria-live="polite">
    {#if termo.trim()}
      {#if achados.length}
        <ul class="bc-lista">
          {#each achados as a (a.entrada.tela + a.rotulo)}
            {@const bloqueada = semServidor && TELAS_DE_SERVIDOR.includes(a.entrada.tela)}
            <li>
              <!-- O nome acessível vem do `aria-label`, não da soma dos spans: o "›" é decoração e
                   o espaço entre eles é do flex, que não conta no nome. -->
              <button class="bc-item" disabled={bloqueada}
                      aria-label={`${a.tela}: ${a.rotulo}${bloqueada ? ` — ${m.config_modal_escolha_servidor()}` : ''}`}
                      onclick={() => abrir(a.entrada.tela)}>
                <span class="bc-tela">{a.tela}</span>
                <span class="bc-sep" aria-hidden="true">›</span>
                <span class="bc-rot">{a.rotulo}</span>
                {#if bloqueada}<span class="bc-motivo">{m.config_modal_escolha_servidor()}</span>{/if}
              </button>
            </li>
          {/each}
        </ul>
      {:else}
        <p class="bc-nada">{m.busca_nenhum_resultado()}</p>
      {/if}
    {/if}
  </div>
</div>

<style>
  .bc { margin-bottom: var(--space-3); }
  /* Campo de texto: superfície própria, pelo token que entra no véu do papel de parede. */
  .bc-campo {
    width: 100%; box-sizing: border-box;
    padding: var(--space-2) var(--space-3);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    background: var(--surface-inset); color: var(--text-primary); font-size: var(--text-sm);
  }
  .bc-lista { list-style: none; margin: var(--space-2) 0 0; padding: 0; }
  .bc-item {
    display: flex; flex-wrap: wrap; align-items: baseline; gap: var(--space-1);
    width: 100%; padding: var(--space-2) var(--space-3);
    border: 0; border-radius: var(--radius-md);
    background: transparent; color: var(--text-primary); font-size: var(--text-sm);
    text-align: left; cursor: pointer;
  }
  @media (hover: hover) { .bc-item:not(:disabled):hover { background: var(--bg-hover); } }
  .bc-item:disabled { color: var(--text-muted); cursor: default; }
  .bc-motivo { flex-basis: 100%; font-size: var(--text-xs); color: var(--text-muted); }
  .bc-tela { color: var(--text-muted); }
  .bc-sep { color: var(--text-muted); }
  .bc-rot { flex: 1; }
  .bc-nada { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted); }
  /* Na coluna de 232px do desktop o par "Tela › Controle" não cabe lado a lado. */
  .compacta .bc-item { flex-direction: column; align-items: flex-start; gap: 0; }
  .compacta .bc-sep { display: none; }
  .compacta .bc-tela { font-size: var(--text-xs); }
</style>
