<script module lang="ts">
  // O engines.json tem alfabeto próprio pro nome (minúsculas, números, '-', '_'): o nome bonito vai
  // pro `label` e o id sai daqui. Sem isto, "Meu Provedor" seria recusado com 400 e o usuário
  // levaria a culpa por ter digitado um espaço.
  export function idDe(texto: string): string {
    const base = texto.toLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '')
      .replace(/[^a-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 32);
    return base || 'chave';
  }
</script>

<script lang="ts">
  // Modelo e opções de UMA chave de API — o bloco que a tela Motores mostrava para o motor, agora
  // aberto dentro do card da credencial. Serve os dois usos: editar um motor do disco e criar um
  // novo (`criando`), quando o nome curto ainda é digitado aqui.
  //
  // Nada de catálogo chumbado: modelos e janela de contexto vêm do PRÓPRIO provedor (GET /v1/models)
  // com a chave da pessoa, porque o valor muda por faixa de assinatura.
  import { untrack } from 'svelte';
  import Select from '../Select.svelte';
  import {
    putEngine, putEngineForServer, engineModelos, engineModelosForServer,
    type Motor, type ModeloProvedor,
  } from '@hangar/core';
  import type { Server } from '../../lib/auth';
  import { sincronizarNosAgentes, type ResultadoSync } from '../../lib/credenciais';
  import * as m from '../../paraglide/messages';

  interface Props {
    apiTarget: Server | null;
    // Em edição é o id no disco; em criação, só o valor inicial do campo de nome curto.
    nome: string;
    motor: Motor | null;
    criando?: boolean;
    // Ids já no disco: o PUT é substituição, então criar com um nome ocupado apagaria o registro
    // do outro. Prop viva, lida na hora de salvar — copiá-la para um `$state` congelaria a lista.
    nomesExistentes?: string[];
    onSalvo: (motores: Record<string, Motor>, alvo: Server | null) => void;
    onFechar: () => void;
  }
  let { apiTarget, nome, motor, criando = false, nomesExistentes = [], onSalvo, onFechar }: Props = $props();

  // Atalhos de endereço: dois provedores que a pessoa desta casa usa e cujo endereço não se
  // adivinha (nem um nem outro é o domínio do produto).
  const DICAS: { label: string; base_url: string }[] = [
    { label: m.config_motores_dica_kimi(), base_url: 'https://api.kimi.com/coding' },
    { label: m.config_motores_dica_omni(), base_url: 'https://ai.omniwise.com.br' },
  ];

  // Defaults do Avançado, espelhando engines.env_de: ligado = capacidade ativa. Os que nascem
  // LIGADOS (cache e raciocínio) são os que causam dano se desligados sem motivo.
  type ChaveLiga = 'bundled_skills' | 'experimental_betas' | 'prompt_caching' | 'adaptive_thinking'
    | 'tool_search' | 'gateway_model_discovery' | 'fine_grained_tool_streaming' | 'auth_via_api_key';
  type ChaveNum = 'auto_compact_window' | 'max_output_tokens';
  const CHAVES_LIGA = ['bundled_skills', 'experimental_betas', 'prompt_caching', 'adaptive_thinking',
    'tool_search', 'gateway_model_discovery', 'fine_grained_tool_streaming', 'auth_via_api_key'] as const;
  const ehLiga = (k: ChaveLiga | ChaveNum): k is ChaveLiga => (CHAVES_LIGA as readonly string[]).includes(k);

  // Retrato do motor no momento de abrir — não ressincroniza se a lista revalidar por trás, o
  // mesmo contrato do editar() da antiga tela Motores. A chave nasce VAZIA de propósito:
  // pré-preencher com a máscara faria qualquer toque mandar o texto mascarado de volta e
  // sobrescrever a chave real. Os ligados por padrão saem só com `false` explícito (`!== false`);
  // os desligados exigem `=== true`.
  // `untrack` não é adorno: sem ele o compilador acusa cada leitura de `motor` aqui
  // (`state_referenced_locally`), e o aviso repetido esconderia um dia a leitura que for engano.
  // Sem motor (criação) os `?.` caem nos mesmos defaults do backend, sem ramo à parte: os ligados
  // por padrão nascem ligados (`undefined !== false`) e o resto desligado.
  let form = $state(untrack(() => ({
    nome,
    label: motor?.label ?? nome,
    base_url: motor?.base_url ?? '',
    base_url_original: motor?.base_url ?? '',
    api_key: '',
    api_key_definida: motor?.api_key_definida ?? false,
    model: motor?.model ?? '',
    subagent_model: motor?.subagent_model ?? '',
    context_window: motor?.context_window ? String(motor.context_window) : '',
    bundled_skills: motor?.bundled_skills === true,
    experimental_betas: motor?.experimental_betas === true,
    prompt_caching: motor?.prompt_caching !== false,
    adaptive_thinking: motor?.adaptive_thinking !== false,
    tool_search: motor?.tool_search === true,
    gateway_model_discovery: motor?.gateway_model_discovery === true,
    fine_grained_tool_streaming: motor?.fine_grained_tool_streaming === true,
    auth_via_api_key: motor?.auth_via_api_key === true,
    auto_compact_window: motor?.auto_compact_window ? String(motor.auto_compact_window) : '',
    max_output_tokens: motor?.max_output_tokens ? String(motor.max_output_tokens) : '',
  })));
  // O bloco não fecha depois de salvar (é onde o resultado da sincronização é lido), então a mesma
  // instância continua na tela com o modelo já no disco: `criando` é só o valor INICIAL. Sem isto o
  // Salvar seguinte manda o id derivado do nome digitado e cria um SEGUNDO modelo. O `untrack` é o
  // mesmo motivo do `form` acima (`state_referenced_locally`).
  let criandoAgora = $state(untrack(() => criando));
  let idNoDisco = $state(untrack(() => nome));
  // Em edição o id no disco não muda; em criação ele sai do nome curto pelo alfabeto do engines.json.
  const idAlvo = $derived(criandoAgora ? idDe(form.nome) : idNoDisco);
  const ligado = (k: ChaveLiga) => form[k];
  const setLigado = (k: ChaveLiga, v: boolean) => { form[k] = v; };
  const numero = (k: ChaveNum) => form[k];
  const setNumero = (k: ChaveNum, v: string) => { form[k] = v; };

  let modelos = $state<ModeloProvedor[]>([]);
  let buscando = $state(false);
  let erroBusca = $state('');
  let okBusca = $state('');
  let salvando = $state(false);
  let erro = $state('');
  let sincronizando = $state(false);
  let sync = $state<ResultadoSync | null>(null);
  let syncErro = $state('');
  let porQue = $state<string | null>(null);

  // Mesmo número do `@container (max-width: 620px)` lá embaixo: um limiar só para toda a
  // apresentação estreita. A medida é em JS porque `open` de <details> é atributo do DOM e
  // nenhuma container query o alcança; quem aperta é o painel, não a janela.
  const LARGURA_COMPACTA = 620;
  let raiz = $state<HTMLElement | null>(null);
  let celular = $state(false);
  $effect(() => {
    if (!raiz) return;
    const medir = (w: number) => { if (w > 0) celular = w <= LARGURA_COMPACTA; };
    // Leitura síncrona antes de observar: sem ela o bloco estreito nasceria com o Avançado
    // aberto e o fecharia no quadro seguinte.
    medir(raiz.getBoundingClientRect().width);
    const ro = new ResizeObserver((es) => medir(es[0]?.contentRect.width ?? 0));
    ro.observe(raiz);
    return () => ro.disconnect();
  });

  // Só o caso documentado da Moonshot, onde desligar o thinking rebaixa K3/K2.7 para K2.6 sem
  // avisar. Não inventar regra para provedor sem doc.
  const ehMoonshot = $derived(/moonshot|kimi/i.test(`${form.base_url} ${form.model}`));
  const descobertaComprovada = $derived(modelos.length > 0);
  const modeloAtual = $derived(modelos.find((x) => x.id === form.model));
  // Motor com chave salva, campo de chave vazio e endereço editado: o Testar usaria a chave salva
  // contra o endereço ANTIGO (o servidor só aceita nome sozinho) e ignoraria a edição calado. Em
  // criação não há chave nem endereço salvos, então não há o que avisar.
  const enderecoMudouSemChave = $derived(!criandoAgora && form.api_key_definida
    && !form.api_key.trim() && form.base_url.trim() !== form.base_url_original);

  // Fechado, o Avançado tem de dizer o que guarda — senão é um clique que ninguém dá. Sai do
  // próprio `form`, então acompanha a edição sem estado à parte.
  const resumoAvancado = $derived(m.config_motores_avancado_resumo({
    janela: form.context_window ? `${Math.round(Number(form.context_window) / 1000)}k` : m.config_motores_padrao(),
    sub: form.subagent_model.trim() || m.config_motores_mesmo_principal(),
    rac: form.adaptive_thinking ? m.comum_ligado() : m.comum_desligado(),
  }));

  async function buscarModelos() {
    buscando = true; erroBusca = ''; okBusca = '';
    try {
      // `nome` e `base_url`/`api_key` são mutuamente exclusivos no servidor (400 juntos).
      const chave = form.api_key.trim();
      const corpo = chave ? { base_url: form.base_url.trim(), api_key: chave } : { nome: idAlvo };
      const r = apiTarget ? await engineModelosForServer(apiTarget, corpo) : await engineModelos(corpo);
      modelos = r.modelos;
      okBusca = m.config_motores_modelos_ok({ n: r.modelos.length });
      const atual = modelos.find((x) => x.id === form.model) ?? modelos[0];
      if (atual) escolherModelo(atual.id);
    } catch (e) {
      erroBusca = e instanceof Error ? e.message : m.config_motores_erro_consultar();
    } finally {
      buscando = false;
    }
  }

  function escolherModelo(id: string) {
    form.model = id;
    const md = modelos.find((x) => x.id === id);
    // A janela vem do provedor: em branco o Claude Code assume 200k e compacta cedo. Modelo sem
    // context_length limpa o campo — o número do modelo ANTERIOR passaria da janela real do novo.
    form.context_window = md?.context_length ? String(md.context_length) : '';
  }

  async function salvar() {
    // Homônimo é recusado ANTES de gravar: o PUT substitui, e o servidor aceitaria calado.
    if (criandoAgora && nomesExistentes.includes(idAlvo)) {
      erro = m.config_motores_nome_em_uso();
      return;
    }
    // O bloco não fecha depois de salvar, então a mesma instância salva de novo: a área de
    // resultado nasce limpa a cada Salvar, senão a sincronização anterior fica na tela ao lado do
    // erro novo, como se fosse desta gravação.
    salvando = true; erro = ''; sync = null; syncErro = '';
    // Retrato do alvo e do callback ANTES do await: prop é getter vivo, e depois do PUT ela já
    // pode apontar pra outra máquina (o pai trocou de alvo e desmontou este bloco).
    const alvo = apiTarget;
    const aoSalvo = onSalvo;
    const id = idAlvo;
    try {
      const corpo: Record<string, unknown> = {
        label: form.label.trim() || (criando ? form.nome.trim() : nome),
        base_url: form.base_url.trim(),
        model: form.model.trim(),
      };
      if (form.api_key.trim()) corpo.api_key = form.api_key.trim();
      // Campo ausente do corpo do PUT herda o valor do disco, e `null` conta como ausente. Quem
      // LIMPA é o campo presente e vazio: `''` sai do registro. Por isso os opcionais vão SEMPRE —
      // omiti-los quando vazios fazia a limpeza voltar HTTP 200 com o valor antigo, calado.
      corpo.subagent_model = form.subagent_model.trim();
      corpo.context_window = form.context_window ? Number(form.context_window) : '';
      // Precedência da visão: valor recém-testado > valor já salvo > omitir.
      const vision = typeof modeloAtual?.vision === 'boolean' ? modeloAtual.vision : motor?.vision;
      if (typeof vision === 'boolean') corpo.vision = vision;
      for (const k of CHAVES_LIGA) corpo[k] = form[k];
      corpo.auto_compact_window = form.auto_compact_window ? Number(form.auto_compact_window) : '';
      corpo.max_output_tokens = form.max_output_tokens ? Number(form.max_output_tokens) : '';

      const r = alvo ? await putEngineForServer(alvo, id, corpo) : await putEngine(id, corpo);
      form.api_key = '';
      // Quem sabe se há chave gravada é o registro que o PUT devolveu: um motor sem chave salvo sem
      // chave nova continua sem chave, e ligar a marca aqui prometeria "definida" com o disco vazio.
      form.api_key_definida = r.motores[id]?.api_key_definida ?? (form.api_key_definida || !!corpo.api_key);
      form.base_url_original = form.base_url.trim();
      // O modelo existe no disco a partir daqui: a criação virou edição DELE. Depois do await de
      // propósito — antes, uma falha de rede deixaria o formulário se dizendo edição do que não foi
      // gravado.
      idNoDisco = id;
      criandoAgora = false;
      aoSalvo(r.motores, alvo);
      // A chave também é dos OUTROS agentes (Pi/Kimi/Codex). Passo SEPARADO, fora do try do salvar:
      // falhar aqui não desfaz o motor, que vale para o Claude Code de qualquer jeito. O bloco
      // continua aberto para o resultado ser lido — quem fecha é a pessoa.
      sincronizando = true; syncErro = '';
      try {
        sync = await sincronizarNosAgentes(alvo, `chave:${id}`);
      } catch (e) {
        syncErro = e instanceof Error && e.message ? e.message : m.config_motores_erro_sync();
      } finally {
        sincronizando = false;
      }
    } catch (e) {
      erro = e instanceof Error ? e.message : m.config_motores_erro_salvar();
    } finally {
      salvando = false;
    }
  }
</script>

<div class="mf" class:compacto={celular} bind:this={raiz}>
  <!-- Um lugar decide como a ajuda longa aparece; os quatro campos só chamam. No estreito ela vai
       para trás de um "?" — <details> nativo já traz teclado e estado, sem lib nem $state. -->
  {#snippet ajudaLonga(rotulo: string, conteudo: import('svelte').Snippet)}
    {#if celular}
      <details class="ajuda-q">
        <!-- O nome leva o campo: quatro botões chamados "Ajuda" são quatro botões idênticos para
             quem navega por teclado ou leitor de tela. -->
        <summary aria-label={m.config_motores_ajuda_campo({ campo: rotulo })}>?</summary>
        <span class="ajuda">{@render conteudo()}</span>
      </details>
    {:else}
      <span class="ajuda">{@render conteudo()}</span>
    {/if}
  {/snippet}
  {#snippet aNome()}
    <!-- Campo vazio mostra o EXEMPLO do placeholder: a frase é lida como comando de verdade, e
         `idDe('')` cai no fallback `chave`, um nome que ninguém escolheu. -->
    {m.config_motores_terminal_1()} <code>claude-engine {form.nome.trim() ? idAlvo : 'kimi'}</code>{m.config_motores_terminal_2()}
  {/snippet}
  {#snippet aEndereco()}
    <strong>{m.config_motores_sem_v1()}</strong> {m.config_motores_messages()}
  {/snippet}
  {#snippet aSubagentes()}
    {m.config_motores_subagentes_ajuda()}
  {/snippet}
  {#snippet aJanela()}
    {m.config_motores_janela_ajuda_1()} <code>/context</code>{m.comum_ponto()}
  {/snippet}

  {#if criando}
    <label class="campo">
      <span class="rot">{m.config_motores_nome_curto()}</span>
      <!-- Gravado, o nome curto é o id no disco: editá-lo aqui criaria outro modelo. Fica visível
           (é o que a pessoa escolheu) e travado. -->
      <input type="text" name="nome" placeholder="kimi" autocapitalize="off" spellcheck={false}
             readonly={!criandoAgora} class:travado={!criandoAgora}
             value={form.nome} oninput={(e) => (form.nome = e.currentTarget.value)} />
      {@render ajudaLonga(m.config_motores_nome_curto(), aNome)}
    </label>
  {/if}

  <label class="campo">
    <span class="rot">{m.config_motores_endereco()}</span>
    <input type="text" name="base_url" autocapitalize="off" spellcheck={false} placeholder="https://…"
           value={form.base_url} oninput={(e) => (form.base_url = e.currentTarget.value)} />
    {@render ajudaLonga(m.config_motores_endereco(), aEndereco)}
    <!-- Dois endereços que não se adivinham; digitá-los à mão é onde nasce o 404 do provedor. -->
    <span class="dicas">
      {#each DICAS as d (d.base_url)}
        <button type="button" class="dica" onclick={() => (form.base_url = d.base_url)}>{d.label}</button>
      {/each}
    </span>
  </label>

  <label class="campo">
    <span class="rot">{m.config_motores_chave()}</span>
    {#if form.api_key_definida}<span class="def">{m.config_motores_chave_definida()}</span>{/if}
    <input type="text" name="api_key" autocomplete="off" autocapitalize="off" spellcheck={false}
           placeholder={form.api_key_definida ? m.config_motores_colar_nova() : m.config_motores_colar()}
           value={form.api_key} oninput={(e) => (form.api_key = e.currentTarget.value)} />
  </label>

  <div class="campo">
    <button type="button" class="btn" onclick={buscarModelos}
            disabled={buscando || !form.base_url.trim() || (!form.api_key.trim() && !form.api_key_definida)}>
      {buscando ? m.config_motores_consultando() : m.config_motores_testar()}
    </button>
    {#if okBusca}<span class="ok">{okBusca}</span>{/if}
    {#if enderecoMudouSemChave}<span class="ajuda erro">{m.config_motores_endereco_mudou()}</span>{/if}
    {#if erroBusca}<span class="ajuda erro">{erroBusca}</span>{/if}
  </div>

  <label class="campo">
    <span class="rot">{m.composer_modelo()}</span>
    {#if modelos.length}
      <Select ariaLabel={m.composer_modelo()} value={form.model}
        opcoes={modelos.map((x) => ({ value: x.id, label: x.id, hint: x.context_length ? `${Math.round(x.context_length / 1000)}k` : undefined }))}
        onchange={escolherModelo} />
    {:else}
      <input type="text" name="model" placeholder={m.config_motores_id_modelo()} autocapitalize="off" spellcheck={false}
             value={form.model} oninput={(e) => (form.model = e.currentTarget.value)} />
      <span class="ajuda">{m.config_motores_testar_ids()}</span>
    {/if}
    {#if modeloAtual?.vision === false}<span class="ajuda erro">{m.config_motores_sem_visao()}</span>{/if}
  </label>

  <label class="campo">
    <span class="rot">{m.config_motores_subagentes()}</span>
    {#if modelos.length}
      <Select ariaLabel={m.config_motores_subagentes()} value={form.subagent_model}
        opcoes={[{ value: '', label: m.config_motores_mesmo_principal() }, ...modelos.map((md) => ({ value: md.id, label: md.id }))]}
        onchange={(v) => (form.subagent_model = v)} />
    {:else}
      <input type="text" name="subagent_model" placeholder={m.config_motores_vazio_principal()} autocapitalize="off" spellcheck={false}
             value={form.subagent_model} oninput={(e) => (form.subagent_model = e.currentTarget.value)} />
    {/if}
    {@render ajudaLonga(m.config_motores_subagentes(), aSubagentes)}
  </label>

  <label class="campo">
    <span class="rot">{m.config_motores_janela()}</span>
    <input type="number" name="context_window" inputmode="numeric" min="1" placeholder={m.ctx_tokens()}
           value={form.context_window} oninput={(e) => (form.context_window = e.currentTarget.value)} />
    {@render ajudaLonga(m.config_motores_janela(), aJanela)}
  </label>

  <details class="avancado" open={!celular}>
    <summary>{m.config_motores_avancado()}{#if celular}<span class="resumo">{resumoAvancado}</span>{/if}</summary>
    <p class="ajuda topo">{m.config_motores_avancado_ajuda()}</p>

    <!-- Uma linha por recurso, no MESMO vocabulário do ServerSettings (rótulo à esquerda,
         controle à direita, separador entre linhas). O motivo é um acordeão: só um aberto por
         vez, senão dez parágrafos abertos viram de novo a parede de texto que isto resolve. -->
    {#snippet linha(chave: ChaveLiga | ChaveNum, rot: string, vered: string, tom: string,
                    motivo: import('svelte').Snippet, morto = false)}
      <div class="linha" class:morta={morto}>
        <div class="txt">
          <span class="rot">{rot}</span>
          <span class="meta">
            <span class="vered {tom}">{vered}</span>
            <button type="button" class="pq" aria-expanded={porQue === chave}
                    onclick={() => (porQue = porQue === chave ? null : chave)}>
              {m.config_motores_por_que()}<span class="chev" class:aberta={porQue === chave} aria-hidden="true">▾</span>
            </button>
          </span>
        </div>
        {#if ehLiga(chave)}
          <input class="switch" type="checkbox" disabled={morto}
                 checked={ligado(chave)} aria-label={rot}
                 onchange={(e) => setLigado(chave, e.currentTarget.checked)} />
        {:else}
          <input class="num" type="number" inputmode="numeric" min="1" placeholder={m.config_motores_padrao()}
                 aria-label={rot} value={numero(chave)}
                 oninput={(e) => setNumero(chave, e.currentTarget.value)} />
        {/if}
      </div>
      {#if porQue === chave}
        <p class="motivo">{@render motivo()}</p>
      {/if}
    {/snippet}

    {#snippet mSkills()}
      {m.config_motores_motivo_skills_1()} <code>claude-api</code>{m.config_motores_motivo_skills_2()} <code>~/.claude/skills</code>{m.config_motores_motivo_skills_3()}
    {/snippet}
    {#snippet mBetas()}
      {m.config_motores_motivo_betas_1()}<code>context_management</code>{m.config_motores_motivo_betas_2()} <code>400 Extra inputs are not permitted</code>{m.comum_ponto()}
    {/snippet}
    {#snippet mCache()}
      {m.config_motores_motivo_cache()}
    {/snippet}
    {#snippet mThinking()}
      {m.config_motores_motivo_thinking_1()}
      {#if ehMoonshot}{m.config_motores_motivo_thinking_moonshot()}{/if}
      {m.config_motores_motivo_thinking_2()} <code>400</code>{m.config_motores_motivo_thinking_3()} <code>thinking</code>{m.config_motores_motivo_thinking_4()} <code>adaptive</code>{m.comum_ponto()}
    {/snippet}
    {#snippet mToolSearch()}
      {#if !form.experimental_betas}
        {m.config_motores_motivo_toolsearch_off()}
      {:else}
        {m.config_motores_motivo_toolsearch_1()} <code>tool_reference</code>{m.comum_ponto()}
      {/if}
    {/snippet}
    {#snippet mDescoberta()}
      {m.config_motores_motivo_descoberta_1()} <code>/v1/models</code>{m.config_motores_motivo_descoberta_2()} <code>/model</code>{m.config_motores_motivo_descoberta_3()}
    {/snippet}
    {#snippet mStreaming()}
      {m.config_motores_motivo_streaming()}
    {/snippet}
    {#snippet mCompactar()}
      {m.config_motores_motivo_compactar_1()} <code>exceeds the context window</code>{m.config_motores_motivo_compactar_2()}
    {/snippet}
    {#snippet mSaida()}
      {m.config_motores_motivo_saida()}
    {/snippet}
    {#snippet mAuthHeader()}
      {m.config_motores_motivo_auth_1()} <code>401 Missing API key</code>{m.config_motores_motivo_auth_2()} <code>opencode.ai/zen/go</code>{m.config_motores_motivo_auth_3()} <code>x-api-key</code>{m.config_motores_motivo_auth_4()} <code>Authorization: Bearer</code>{m.config_motores_motivo_auth_5()}
    {/snippet}

    <div class="grade">
      {@render linha('bundled_skills', m.config_motores_skills(), m.config_motores_recomendado_desligado(), '', mSkills)}
      {@render linha('experimental_betas', m.config_motores_betas(), m.config_motores_recomendado_desligado(), '', mBetas)}
      {@render linha('prompt_caching', m.config_motores_cache(), m.config_motores_recomendado_ligado(), 'sim', mCache)}
      {@render linha('adaptive_thinking', m.config_motores_raciocinio(),
        ehMoonshot ? m.config_motores_obrigatorio() : m.config_motores_recomendado_ligado(),
        ehMoonshot ? 'forte' : 'sim', mThinking)}
      {@render linha('tool_search', m.config_motores_tool_search(),
        form.experimental_betas ? m.config_motores_recomendado_desligado() : m.config_motores_sem_efeito(),
        '', mToolSearch, !form.experimental_betas)}
      {@render linha('gateway_model_discovery', m.config_motores_descoberta(),
        descobertaComprovada ? m.config_motores_ligado_n({ n: modelos.length }) : m.config_motores_teste_antes(),
        descobertaComprovada ? 'sim' : '', mDescoberta)}
      {@render linha('fine_grained_tool_streaming', m.config_motores_streaming(), m.config_motores_recomendado_desligado(), '', mStreaming)}
      {@render linha('auth_via_api_key', m.config_motores_x_api_key(),
        m.config_motores_ligue_401(), '', mAuthHeader)}
      {@render linha('auto_compact_window', m.config_motores_compactar(), m.config_motores_recomendado_branco(), '', mCompactar)}
      {@render linha('max_output_tokens', m.config_motores_teto_saida(), m.config_motores_recomendado_branco(), '', mSaida)}
    </div>
  </details>

  {#if erro}<p class="aviso erro">{erro}</p>{/if}
  {#if sincronizando}<p class="aviso">{m.novacred_salvando_sync()}</p>{/if}
  {#if syncErro}<p class="aviso erro">{syncErro}</p>{/if}
  {#if sync}
    <div class="sync-bloco">
      <span class="sync-tit">{m.novacred_sync_titulo()}</span>
      {#each Object.entries(sync.resultado) as [alvo, r] (alvo)}
        <p class="sync-linha" class:pulado={!r.ok && r.motivo === 'nao-instalado'} class:falhou={!r.ok && r.motivo !== 'nao-instalado'}>
          <b>{alvo}</b>
          {r.ok ? m.novacred_sync_ok() : (r.motivo === 'nao-instalado' ? m.novacred_sync_nao_instalado() : r.motivo)}
        </p>
      {/each}
    </div>
  {/if}

  <!-- Em criação este comando já está na ajuda do nome curto. -->
  {#if !criando}
    <p class="ajuda">{m.config_motores_terminal_1()} <code>claude-engine {nome}</code>{m.comum_ponto()}</p>
  {/if}
  <!-- Salvar reescreve o engines.json; quem já está numa sessão deste motor segue no valor antigo
       até abrir outra. Vale na criação também: o bloco continua aberto depois do primeiro Salvar, e
       o Salvar seguinte já é edição. Sem esta linha o efeito parecia não ter acontecido. -->
  <p class="ajuda">{m.config_motores_sessoes_abertas()}</p>

  <div class="acoes" class:fixo={celular}>
    <!-- Depois de salvar o botão diz Fechar: a edição já foi, o que resta na tela é o resultado. -->
    <button type="button" class="btn" onclick={onFechar} disabled={salvando || sincronizando}
      >{sync || syncErro ? m.sessao_fechar() : m.comum_cancelar()}</button>
    <button type="button" class="btn primario" onclick={salvar}
            disabled={salvando || !form.model.trim() || !form.base_url.trim() || (criandoAgora && !form.nome.trim())}>
      {salvando ? m.config_motores_salvando() : m.ctx_salvar()}
    </button>
  </div>
</div>

<style>
  /* O bloco mora DENTRO do card da credencial (largura cheia, abaixo dela), como o formulário do
     cookie: é configuração daquela credencial, não uma tela. --surface-inset porque é área de
     entrada e acompanha o slider de transparência. */
  .mf { display: flex; flex-direction: column; gap: var(--space-3); margin-top: var(--space-2);
        padding: var(--space-3); border: 1px solid var(--border-subtle); border-radius: 10px;
        background: var(--surface-inset); }
  .campo { display: flex; flex-direction: column; gap: var(--space-2); }
  .rot { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; margin: 0; }
  .ajuda.erro { color: var(--error); }
  /* O "?" é o alvo de toque da ajuda no painel estreito: bolinha com superfície própria (que
     acompanha o slider de transparência), sem o triângulo padrão do <details>. */
  .ajuda-q > summary {
    display: inline-flex; align-items: center; justify-content: center;
    width: 24px; height: 24px; min-height: 24px; box-sizing: border-box;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-full);
    background: var(--surface-raised); color: var(--text-muted);
    font-size: 12px; line-height: 1; cursor: pointer; list-style: none;
  }
  .ajuda-q > summary::-webkit-details-marker { display: none; }
  /* O dedo pega 44px, mesmo alvo que o `.btn` recebe em painel estreito; o desenho continua a
     bolinha de 24. O pseudo-elemento cresce a área SEM empurrar o layout em volta. */
  .ajuda-q > summary { position: relative; }
  .ajuda-q > summary::after {
    content: ''; position: absolute; left: 50%; top: 50%;
    width: 44px; height: 44px; transform: translate(-50%, -50%);
  }
  .ajuda-q[open] > summary { color: var(--text-primary); }
  .ajuda-q > .ajuda { display: block; margin-top: var(--space-2); }
  .def { font-size: 11px; color: var(--success); }
  .dicas { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .dica {
    background: var(--surface-raised); color: var(--text-secondary);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-full);
    padding: 2px 10px; font-size: 11px; cursor: pointer;
  }
  .ok { font-size: var(--text-xs); color: var(--success); }

  /* Avançado: nasce ABERTO — os dez controles são o miolo do que a tela de motores oferecia, e atrás
     de um clique eles não eram encontrados. <details> nativo em vez de estado próprio; o toggle já
     vem acessível e some com prefers-reduced-motion sem regra, e quem quiser fechar fecha. */
  .avancado {
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md);
    padding: var(--space-3) var(--space-4);
    display: flex; flex-direction: column; gap: var(--space-3);
  }
  .avancado > summary {
    cursor: pointer; font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary);
    /* Sem gap quando fechado: o `gap` do flex vale só entre filhos visíveis, e o summary é o único. */
    margin: calc(var(--space-3) * -1) 0;
  }
  .avancado[open] > summary { margin-bottom: 0; }
  /* Fechado, o bloco diz o que guarda: sem isto o resumo seria só mais um clique no escuro. */
  .resumo { display: block; margin-top: 2px; font-size: var(--text-xs); font-weight: 400; color: var(--text-muted); }
  .avancado .ajuda.topo { margin: 0; max-width: 68ch; }

  /* Linha de recurso: MESMO vocabulário do ServerSettings (rótulo à esquerda, controle à direita,
     separador entre linhas). Nada de card por item — dez cards viram ruído, e o separador já
     agrupa. Grid de 3 faixas pra o motivo expandido nascer alinhado sob o texto, não sob o
     controle. */
  .grade { display: grid; grid-template-columns: 1fr; }
  .linha {
    display: flex; align-items: center; justify-content: space-between; gap: var(--space-4);
    padding: var(--space-3) 0;
    border-top: 1px solid var(--border-subtle);
  }
  .linha:first-of-type { border-top: none; }
  /* Recurso inerte por dependência (tool search sem os betas): esmaece, mas o veredito continua
     legível — ele é justamente quem explica por que o controle está morto. */
  .linha.morta .rot { color: var(--text-muted); }

  .txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .avancado .rot { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .meta { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); font-size: 11px; }
  .vered { color: var(--text-muted); }
  .vered.sim { color: var(--success); }
  .vered.forte { color: var(--warning); font-weight: 600; }

  /* "por quê?" é um botão de verdade, não texto solto: cor de link, sublinhado no hover e uma seta
     que gira. Sem afordância ninguém percebe que abre. */
  .pq {
    display: inline-flex; align-items: center; gap: 3px;
    padding: 4px 0; background: none; border: none; cursor: pointer;
    font-size: 11px; color: var(--accent);
  }
  .pq:hover { text-decoration: underline; }
  .pq .chev { font-size: 8px; transition: transform 160ms var(--ease-out); }
  .pq .chev.aberta { transform: rotate(180deg); }
  .motivo {
    margin: 0 0 var(--space-3);
    font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.5; max-width: 62ch;
  }

  /* `.switch` é global (app.css) — vocabulário único de liga/desliga do app. */
  .avancado input.num { width: 132px; flex-shrink: 0; }

  /* Duas colunas quando o painel é largo: quem aperta é o PAINEL, não a janela (no celular o modal
     é estreito; no desktop o card tem a largura da coluna). `column` (multicol) em vez de grid
     porque o motivo expandido tem altura variável — no grid ele abriria um buraco na coluna
     vizinha. */
  @container (min-width: 720px) {
    .grade { display: block; columns: 2; column-gap: var(--space-7); }
    /* break-inside evita a linha ser cortada ao meio na virada de coluna. */
    .linha, .motivo { break-inside: avoid; }
    /* Na multicol, :first-of-type só acerta a primeira do documento; a primeira da 2a coluna
       ficaria com um separador solto no topo. Borda embaixo resolve nas duas colunas. */
    .linha { border-top: none; border-bottom: 1px solid var(--border-subtle); }
  }

  input[type='text'], input[type='number'] {
    height: 40px; background: var(--surface-raised); border: 1px solid var(--border-default);
    border-radius: var(--radius-sm); color: var(--text-primary); font-family: var(--font-mono);
    font-size: 16px; padding: 0 var(--space-3); outline: none; min-width: 0;
  }
  input:focus { border-color: var(--accent); }
  /* Só-leitura tem que se distinguir do editável, senão a pessoa digita e nada acontece. O
     seletor leva o `input[type=...]` junto: só `.travado` perde na cascata para a regra acima —
     medido, a classe entrava e a cor não mudava. */
  input[type='text'].travado { color: var(--text-muted); cursor: default; }
  .aviso { font-size: var(--text-sm); color: var(--text-muted); margin: 0; }
  .aviso.erro { color: var(--error); }
  .sync-bloco { margin: 0; }
  .sync-tit { font-size: 11.5px; color: var(--text-secondary); }
  .sync-linha { margin: 3px 0 0; font-size: var(--text-xs); color: var(--text-secondary); }
  .sync-linha b { color: var(--text-primary); font-weight: 600; margin-right: 6px; }
  .sync-linha.pulado { color: var(--text-muted); }
  .sync-linha.falhou { color: var(--error); }
  /* No painel largo os botões ficam no fim do bloco; no estreito eles grudam no pé enquanto o
     formulário rola (`.fixo` abaixo). */
  .acoes { display: flex; justify-content: flex-end; gap: var(--space-2); }
  /* CHROME FUNCIONAL, sólido de propósito (mesmo caso do `.rodape` do ServerSettings/VozSettings):
     com token de véu o texto que rola por baixo atravessaria os botões. As margens negativas
     cancelam o respiro do `.mf` para a faixa ir de ponta a ponta, e o raio de baixo acompanha o
     bloco — `overflow` no `.mf` viraria o scrollport e mataria o sticky. */
  .acoes.fixo {
    position: sticky; bottom: calc(-1 * var(--space-3));
    margin: 0 calc(-1 * var(--space-3)) calc(-1 * var(--space-3));
    padding: var(--space-3);
    padding-bottom: calc(var(--space-3) + env(safe-area-inset-bottom));
    background: rgb(var(--glass-panel-rgb));
    border-top: 1px solid var(--border-subtle);
    border-radius: 0 0 9px 9px;
  }
  /* O sticky cobre o que o navegador encosta na borda de baixo ao dar foco; a margem faz a
     rolagem parar acima da faixa (69px medidos + folga). Só no compacto: no largo não há faixa.
     `:global` porque o gatilho do <Select> é elemento de outro componente. */
  .mf.compacto :global(:is(input, button, summary)) { scroll-margin-bottom: 84px; }
  .btn { height: 36px; min-height: 0; padding: 0 var(--space-4); border-radius: var(--radius-sm);
         border: 1px solid var(--border-subtle); background: var(--surface-raised);
         color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; cursor: pointer; }
  .btn.primario { background: var(--accent); border-color: var(--accent); color: #fff; }
  .btn:disabled { opacity: 0.45; cursor: default; }
  @container (max-width: 620px) { .btn { height: 44px; } }
</style>
