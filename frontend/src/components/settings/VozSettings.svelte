<script lang="ts">
  import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
  import LinhaConfig from './LinhaConfig.svelte';
  import Select from '../Select.svelte';
  import SegmentedPicker from '../SegmentedPicker.svelte';
  import { lerMaosLivres, setMaosLivres } from '../../lib/maosLivres';
  import { ditadoEstilo, estilosDitado, type EstiloDitado } from '../../lib/ditadoEstilo.svelte';
  import { listarVozesTts, saldoTts, type TtsVoz } from '../../lib/api';
  import { ttsPlayer } from '../../lib/ttsPlayer.svelte';
  import { ouvirAmostra } from '../../lib/ouvir';
  import { cortarAmostra } from '../../lib/ttsFormat';
  import { intlLocale } from '../../lib/locale';
  import * as m from '../../paraglide/messages';

  // A tela reúne o caminho inteiro do áudio, hoje partido em três telas (Ditado, Anexos, Avançado):
  // ditar -> transcrever -> limpar o texto -> ler em voz alta. A ordem das seções é o fluxo real.
  interface Props {
    store: ConfigServidorStore;
  }
  let { store }: Props = $props();

  // --- Ditar ---------------------------------------------------------------------------------
  // Mãos-livres é preferência do APARELHO (localStorage), não do servidor — mesmo store que o
  // antigo DictationSettings usava.
  let maosLivres = $state(lerMaosLivres());

  // Estilo do ditado: revalida ao ABRIR a tela, pelo mesmo motivo do DitadoEstiloPopover — o valor
  // pode ter mudado noutro aparelho e a tela não pode mostrar valor de horas atrás.
  $effect(() => { void ditadoEstilo.revalidar(); });

  let estiloErro = $state('');
  async function escolherEstilo(v: EstiloDitado) {
    estiloErro = '';
    try {
      await ditadoEstilo.trocar(v);
    } catch (e) {
      estiloErro = e instanceof Error ? e.message : m.config_motores_erro_salvar();
    }
  }
  const OPCOES_ESTILO = $derived(estilosDitado().map((e) => ({ v: e.valor, label: e.rotulo, aria: e.hint })));

  const CAMPO_VOCABULARIO = {
    chave: 'ditado_vocabulario', tipo: 'texto' as const,
    rotulo: m.config_server_vocabulario(), ajuda: m.config_server_vocabulario_ajuda(),
  };

  // --- Transcrever -----------------------------------------------------------------------------
  const transcreverOk = $derived(store.campos['groq_api_key']?.definido === true);
  const CAMPO_GROQ = {
    chave: 'groq_api_key', tipo: 'segredo' as const,
    rotulo: m.config_server_groq(), ajuda: m.voz_transcrever_ajuda(),
  };

  // --- Limpar o texto ----------------------------------------------------------------------------
  // Sozinho, sem chave nenhuma: usa Groq com o padrão do app. O acordeão só existe pra quem quer
  // trocar de provedor. `{#if avancadoAberto}` (não só o `open` do <details>) tira o conteúdo do
  // DOM de verdade quando fechado — o próprio elemento nativo mantém os filhos montados mesmo
  // colapsado, e isso deixaria a tela "dizendo" o endpoint do LLM mesmo com o acordeão fechado.
  let avancadoAberto = $state(false);
  const CAMPOS_LLM = [
    { chave: 'llm_base_url', tipo: 'texto' as const, rotulo: m.config_server_endpoint_llm(), ajuda: m.config_server_endpoint_llm_ajuda() },
    { chave: 'llm_api_key', tipo: 'segredo' as const, rotulo: m.config_server_chave_llm(), ajuda: m.config_server_chave_llm_ajuda() },
    { chave: 'llm_model', tipo: 'texto' as const, rotulo: m.config_server_modelo_llm(), ajuda: m.config_server_modelo_llm_ajuda() },
    { chave: 'llm_reasoning_effort', tipo: 'escolha' as const, rotulo: m.config_server_raciocinio_llm(), ajuda: m.config_server_raciocinio_llm_ajuda(),
      opcoes: [{ value: '', label: m.config_server_raciocinio_padrao() },
               { value: 'none', label: 'none' }, { value: 'low', label: 'low' },
               { value: 'medium', label: 'medium' }, { value: 'high', label: 'high' }] },
    { chave: 'llm_briefing_base_url', tipo: 'texto' as const, rotulo: m.config_server_endpoint_llm_briefing(), ajuda: m.config_server_endpoint_llm_briefing_ajuda() },
    { chave: 'llm_briefing_api_key', tipo: 'segredo' as const, rotulo: m.config_server_chave_llm_briefing(), ajuda: m.config_server_chave_llm_briefing_ajuda() },
    { chave: 'llm_briefing_model', tipo: 'texto' as const, rotulo: m.config_server_modelo_llm_briefing(), ajuda: m.config_server_modelo_llm_briefing_ajuda() },
  ];

  // --- Ler em voz alta -----------------------------------------------------------------------
  const lerOk = $derived(store.campos['elevenlabs_api_key']?.definido === true);
  const CAMPO_ELEVEN = {
    chave: 'elevenlabs_api_key', tipo: 'segredo' as const,
    rotulo: m.config_server_elevenlabs(), ajuda: m.config_server_elevenlabs_ajuda(),
  };
  const CAMPO_MAX_CHARS = {
    chave: 'tts_max_chars', tipo: 'numero' as const, sufixo: m.config_server_car(),
    rotulo: m.config_server_confirmar_leitura(), ajuda: m.config_server_confirmar_leitura_ajuda(),
  };
  const CAMPO_CMD_LOCAL = {
    chave: 'tts_local_cmd', tipo: 'texto' as const,
    rotulo: m.config_server_comando_voz(), ajuda: m.config_server_comando_voz_ajuda(),
  };

  // Vozes/saldo/naturalidade/amostra: movidos de ServerSettings.svelte, inclusive o carregamento
  // SOB DEMANDA — abrir esta tela não pode disparar rede pra fora só por estar aberta.
  let vozes = $state<TtsVoz[]>([]);
  let vozErro = $state('');
  let carregandoVozes = $state(false);
  let saldo = $state<{ usados: number | null; limite: number | null } | null>(null);
  let saldoErro = $state('');

  function carregarVozes() {
    vozErro = '';
    carregandoVozes = true;
    listarVozesTts()
      .then((v) => { vozes = v; })
      .catch((e: Error) => { vozErro = e.message; })
      .finally(() => { carregandoVozes = false; });
    saldoErro = '';
    saldoTts().then((s) => { saldo = s; }).catch((e: Error) => { saldoErro = e.message; });
  }

  const amostraTexto = $derived(cortarAmostra(ttsPlayer.ultimoTexto));
  function ouvirAmostraDaVoz() {
    const voz = (store.valorAtual('elevenlabs_voice_id') as string) || '';
    ouvirAmostra(amostraTexto, voz);
  }

  interface AjusteSlider {
    chave: string;
    rotulo: string;
    padrao: number;
    min: number;
    max: number;
    esquerda: string;
    direita: string;
    ajuda: string;
  }

  const AJUSTES_VOZ: AjusteSlider[] = [
    { chave: 'tts_stability', rotulo: m.config_server_estabilidade(), padrao: 50, min: 0, max: 100,
      esquerda: m.config_server_mais_emotiva(), direita: m.config_server_mais_constante(),
      ajuda: m.config_server_estabilidade_ajuda() },
    { chave: 'tts_similarity_boost', rotulo: m.config_server_aderencia(), padrao: 75, min: 0, max: 100,
      esquerda: m.config_server_mais_livre(), direita: m.config_server_mais_fiel(),
      ajuda: m.config_server_aderencia_ajuda() },
    { chave: 'tts_style', rotulo: m.config_server_exagero(), padrao: 0, min: 0, max: 100,
      esquerda: m.config_server_neutro(), direita: m.config_server_marcante(),
      ajuda: m.config_server_exagero_ajuda() },
    { chave: 'tts_speed', rotulo: m.config_server_velocidade(), padrao: 100, min: 70, max: 120,
      esquerda: m.config_server_mais_devagar(), direita: m.config_server_mais_rapido(),
      ajuda: m.config_server_velocidade_ajuda() },
  ];

  function ajusteValor(a: AjusteSlider): number {
    const bruto = store.valorAtual(a.chave);
    const n = typeof bruto === 'number' ? bruto : parseInt(String(bruto), 10);
    return Number.isFinite(n) ? n : a.padrao;
  }
  function ajusteDefinir(a: AjusteSlider, n: number) {
    store.setRascunho(a.chave, n);
  }
  function ajusteResetar(a: AjusteSlider) {
    store.setRascunho(a.chave, a.padrao);
  }
</script>

<div class="voz">
  {#if store.carregando}
    <p class="aviso">{m.comum_carregando()}</p>
  {:else if store.erro && !Object.keys(store.campos).length}
    <p class="aviso erro">{store.erro}</p>
    <button class="btn" onclick={() => void store.carregar()}>{m.config_server_tentar_de_novo()}</button>
  {:else}
    <!-- Ditar -->
    <section class="secao">
      <h3>{m.voz_ditar()}</h3>

      <div class="linha-maos-livres">
        <div class="txt">
          <label class="rot" for="voz-maos-livres">{m.config_ditado_titulo()}</label>
          <span class="ajuda">{m.config_ditado_desc()}</span>
        </div>
        <input id="voz-maos-livres" class="switch" type="checkbox" bind:checked={maosLivres}
          onchange={() => setMaosLivres(maosLivres)} />
      </div>
      <p class="nota">{m.voz_so_neste_aparelho()}</p>

      <div class="estilo">
        <div class="txt">
          <span class="rot">{m.voz_estilo()}</span>
          <span class="ajuda">{m.voz_estilo_ajuda()}</span>
        </div>
        <SegmentedPicker value={ditadoEstilo.valor} options={OPCOES_ESTILO}
          ariaLabel={m.voz_estilo()} onPick={(v) => void escolherEstilo(v)} />
      </div>
      {#if estiloErro}<p class="aviso erro">{estiloErro}</p>{/if}

      <LinhaConfig campo={CAMPO_VOCABULARIO} {store} />
    </section>

    <!-- Transcrever -->
    <section class="secao">
      <h3>{m.voz_transcrever()}</h3>
      {#if !transcreverOk}
        <p class="aviso">{m.voz_transcrever_sem_chave()}</p>
      {/if}
      <LinhaConfig campo={CAMPO_GROQ} {store} />
      <a class="link" href="https://console.groq.com/keys" target="_blank" rel="noopener noreferrer">
        {m.voz_criar_chave()}
      </a>
    </section>

    <!-- Limpar o texto -->
    <section class="secao">
      <h3>{m.voz_limpar()}</h3>
      <p class="ajuda">{m.voz_limpar_ajuda()}</p>
      <details class="detalhes" bind:open={avancadoAberto}>
        <summary>{m.voz_usar_outro_servico()}</summary>
        {#if avancadoAberto}
          <div class="detalhes-corpo">
            {#each CAMPOS_LLM as c (c.chave)}
              <LinhaConfig campo={c} {store} />
            {/each}
          </div>
        {/if}
      </details>
    </section>

    <!-- Ler em voz alta -->
    <section class="secao">
      <h3>{m.voz_ler()}</h3>
      <LinhaConfig campo={CAMPO_ELEVEN} {store} />
      {#if !lerOk}
        <p class="aviso">{m.voz_ler_sem_chave()}</p>
      {:else}
        <div class="tts-extra">
          {#if vozErro}
            <p class="aviso erro">{vozErro}</p>
            <button class="btn" onclick={carregarVozes} disabled={carregandoVozes}>{m.config_server_tentar_de_novo()}</button>
          {:else if vozes.length}
            <Select
              class="campo-select"
              ariaLabel={m.config_server_voz()}
              value={String(store.valorAtual('elevenlabs_voice_id') ?? '')}
              opcoes={[{ value: '', label: m.config_server_padrao_servidor() },
                       ...vozes.map((v) => ({ value: v.id, label: v.nome }))]}
              onchange={(v) => store.setRascunho('elevenlabs_voice_id', v)}
            />
          {:else}
            <button class="btn" onclick={carregarVozes} disabled={carregandoVozes}>
              {carregandoVozes ? m.comum_carregando() : m.config_server_carregar_vozes()}
            </button>
          {/if}

          <div class="naturalidade">
            {#each AJUSTES_VOZ as a (a.chave)}
              {@const valor = ajusteValor(a)}
              <div class="ajuste">
                <div class="ajuste-cabeca">
                  <span class="ajuste-rot">{a.rotulo} <em>{valor}</em></span>
                  {#if valor !== a.padrao}
                    <button class="ajuste-reset" onclick={() => ajusteResetar(a)}>{m.config_server_voltar_padrao()}</button>
                  {/if}
                </div>
                <span class="ajuda">{a.ajuda}</span>
                <div class="ajuste-slider">
                  <span class="ponta">{a.esquerda}</span>
                  <input type="range" aria-label={a.rotulo} min={a.min} max={a.max} step="1" value={valor}
                    oninput={(e) => ajusteDefinir(a, +e.currentTarget.value)} />
                  <span class="ponta">{a.direita}</span>
                </div>
              </div>
            {/each}
          </div>

          <div class="amostra">
            <button class="btn" onclick={ouvirAmostraDaVoz} disabled={!ttsPlayer.ultimoTexto}>
              {m.config_server_ouvir_amostra()}{ttsPlayer.ultimoTexto ? m.config_server_caracteres({ n: amostraTexto.length.toLocaleString(intlLocale()) }) : ''}
            </button>
            {#if !ttsPlayer.ultimoTexto}
              <span class="ajuda">{m.config_server_ouca_antes()}</span>
            {/if}
            {#if ttsPlayer.error}<p class="aviso erro">{ttsPlayer.error}</p>{/if}
          </div>

          {#if saldo}
            <p class="sub">{m.config_server_consumo({ usados: saldo.usados ?? '?', limite: saldo.limite ?? '?' })}</p>
          {/if}
          {#if saldoErro}<p class="aviso erro">{saldoErro}</p>{/if}
        </div>
      {/if}
      <LinhaConfig campo={CAMPO_MAX_CHARS} {store} />
      <LinhaConfig campo={CAMPO_CMD_LOCAL} {store} />
    </section>
  {/if}

  {#if store.erro && Object.keys(store.campos).length}<p class="aviso erro">{store.erro}</p>{/if}
</div>

{#if !store.carregando && Object.keys(store.campos).length}
  <div class="rodape">
    {#if store.salvo}<span class="ok">{m.config_server_salvo()}</span>{/if}
    <button class="btn primario" onclick={store.salvar} disabled={!store.temMudanca || store.salvando}>
      {store.salvando ? m.config_motores_salvando() : m.ctx_salvar()}
    </button>
  </div>
{/if}

<style>
  /* Container query, nunca media query: quem aperta a linha e a largura do PAINEL. */
  .voz { container-type: inline-size; padding: var(--space-2) var(--space-4) var(--space-4); display: flex; flex-direction: column; gap: var(--space-5); }

  .secao h3 { margin: 0 0 var(--space-2); font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary); }

  .txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .rot { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; min-width: 0; }
  .nota { margin: var(--space-1) 0 0; font-size: var(--text-xs); color: var(--text-muted); }

  .linha-maos-livres { display: flex; align-items: center; justify-content: space-between; gap: var(--space-4); padding: var(--space-2) 0; }
  .estilo { display: flex; flex-direction: column; gap: var(--space-2); padding: var(--space-3) 0; border-top: 1px solid var(--border-subtle); }

  .link { display: inline-block; margin-top: var(--space-2); font-size: var(--text-xs); color: var(--accent); }

  /* Acordeão nativo, fechado por padrão — o próprio marcador (▶/▼) já diz que há mais coisa dentro. */
  .detalhes { margin-top: var(--space-2); }
  .detalhes summary { cursor: pointer; font-size: var(--text-sm); font-weight: 600; color: var(--accent); }
  .detalhes-corpo { margin-top: var(--space-2); }

  .tts-extra { container-type: inline-size; margin-top: var(--space-2); }
  .tts-extra :global(.campo-select) { width: 100%; font-family: var(--font-ui); font-size: var(--text-sm); }
  @container (min-width: 360px) { .tts-extra :global(.campo-select) { width: auto; min-width: 220px; } }

  .amostra { display: flex; flex-direction: column; gap: var(--space-2); margin-top: var(--space-3); align-items: flex-start; }

  .naturalidade { display: flex; flex-direction: column; gap: var(--space-3); margin: var(--space-3) 0; }
  .ajuste { display: flex; flex-direction: column; gap: 2px; }
  .ajuste-cabeca { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: var(--space-2); }
  .ajuste-rot { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .ajuste-rot em { margin-left: var(--space-2); font-style: normal; color: var(--text-muted); font-size: var(--text-xs); }
  .ajuste-reset { flex-shrink: 0; font-size: var(--text-xs); color: var(--accent); background: none; border: none; padding: 0; }
  .ajuste-slider { display: flex; align-items: center; gap: var(--space-2); margin-top: var(--space-1); }
  .ajuste-slider input { flex: 1; min-width: 100px; accent-color: var(--accent); }
  .ajuste-slider .ponta { font-size: var(--text-xs); color: var(--text-muted); white-space: nowrap; flex-shrink: 0; }

  .aviso { font-size: var(--text-sm); color: var(--text-muted); margin: var(--space-2) 0; }
  .aviso.erro { color: var(--error); }
  .sub { margin: var(--space-2) 0 0; font-size: var(--text-xs); color: var(--text-muted); }

  /* CHROME FUNCIONAL, sólido de propósito: grudado no fim da folha — mesma exceção que o
     .rodape do ServerSettings/EnginesSettings já documenta. NÃO converter pra token de véu. */
  .rodape {
    position: sticky; bottom: 0;
    display: flex; align-items: center; justify-content: flex-end; gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    padding-bottom: calc(var(--space-3) + env(safe-area-inset-bottom));
    background: var(--bg-surface);
    border-top: 1px solid var(--border-subtle);
  }
  .ok { font-size: var(--text-xs); color: var(--success); }
  .btn {
    height: 40px; padding: 0 var(--space-4);
    border-radius: var(--radius-md);
    background: var(--surface-raised); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 600;
    transition: transform 160ms ease-out;
  }
  .btn:not(:disabled):active { transform: scale(0.97); }
  .btn.primario { background: var(--accent); color: #fff; }
  .btn:disabled { opacity: 0.45; }
</style>
