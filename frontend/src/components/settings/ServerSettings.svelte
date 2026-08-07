<script lang="ts">
  import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
  import { listarVozesTts, saldoTts, type TtsVoz } from '../../lib/api';
  import Select from '../Select.svelte';
  import { ttsPlayer } from '../../lib/ttsPlayer.svelte';
  import { ouvirAmostra } from '../../lib/ouvir';
  import { cortarAmostra } from '../../lib/ttsFormat';

  // Configuração do servidor pelo app. Até aqui tudo vinha só de env/.env: pra mudar a chave da
  // transcrição ou a retenção de anexos era preciso editar arquivo no servidor e reiniciar o
  // serviço — do celular, impossível.
  //
  // O segredo entra mas não sai: o backend devolve mascarado (gsk_••••1234). Dá pra conferir QUAL
  // chave está lá e trocá-la; não dá pra copiar de volta.
  interface Props {
    store: ConfigServidorStore;
    /** Qual fatia mostrar. O ESTADO e um so, compartilhado pelas tres. */
    secao: 'notificacoes' | 'anexos' | 'avancado';
  }
  let { store, secao }: Props = $props();

  const TITULOS: Record<Props['secao'], string> = {
    notificacoes: 'Notificações',
    anexos: 'Anexos e transcrição',
    avancado: 'Avançado do servidor',
  };

  interface Campo {
    chave: string;
    rotulo: string;
    ajuda: string;
    tipo: 'texto' | 'segredo' | 'numero' | 'liga';
    sufixo?: string;
    secao: Props['secao'];
  }

  const CAMPOS: Campo[] = [
    { chave: 'groq_api_key', rotulo: 'Chave da Groq', tipo: 'segredo', secao: 'anexos',
      ajuda: 'Transcreve áudio gravado e a fala dos vídeos anexados. Vazia = transcrição desligada.' },
    { chave: 'upload_retention_days', rotulo: 'Guardar anexos por', tipo: 'numero', sufixo: 'dias', secao: 'anexos',
      ajuda: 'Anexo mais velho que isso é apagado no próximo upload. 0 = nunca apagar.' },
    { chave: 'automations', rotulo: 'Automações', tipo: 'liga', secao: 'avancado',
      ajuda: 'Chave mestra do que roda sem você olhar: encadeamento de sessão e auto-resume.' },
    { chave: 'notify_finished', rotulo: 'Avisar quando terminar', tipo: 'liga', secao: 'notificacoes',
      ajuda: 'Notificação quando um turno longo acaba.' },
    { chave: 'finish_min_seconds', rotulo: 'Turno curto não avisa', tipo: 'numero', sufixo: 'seg', secao: 'notificacoes',
      ajuda: 'Turno mais rápido que isso não gera notificação.' },
    { chave: 'notify_dead', rotulo: 'Avisar quando cair', tipo: 'liga', secao: 'notificacoes',
      ajuda: 'Notificação quando uma sessão morre.' },
    { chave: 'stall_seconds', rotulo: 'Marcar travada após', tipo: 'numero', sufixo: 'seg', secao: 'notificacoes',
      ajuda: 'Sessão "trabalhando" e calada por mais que isso ganha o aviso de travada.' },
    { chave: 'editor', rotulo: 'Editor', tipo: 'texto', secao: 'avancado',
      ajuda: 'Binário que abre a pasta da sessão no desktop (ex: code, subl).' },
    { chave: 'llm_base_url', rotulo: 'Endpoint do LLM', tipo: 'texto', secao: 'avancado',
      ajuda: 'Serviço compatível com a API da OpenAI que trata o texto do ditado e da leitura em voz. Vazio = Groq.' },
    { chave: 'llm_api_key', rotulo: 'Chave do LLM', tipo: 'segredo', secao: 'avancado',
      ajuda: 'Chave do serviço acima. Obrigatória quando o endpoint não é o padrão.' },
    { chave: 'llm_model', rotulo: 'Modelo do LLM', tipo: 'texto', secao: 'avancado',
      ajuda: 'Nome do modelo no serviço escolhido. Vazio = llama-3.3-70b-versatile.' },
    { chave: 'elevenlabs_api_key', rotulo: 'Chave da ElevenLabs', tipo: 'segredo', secao: 'anexos',
      ajuda: 'Gera a voz que lê os trechos do chat. Vazia = leitura em voz desligada.' },
    { chave: 'tts_max_chars', rotulo: 'Confirmar leitura acima de', tipo: 'numero', sufixo: 'car.', secao: 'anexos',
      ajuda: 'Seleção maior que isso pede confirmação antes de gerar áudio. 0 = usa o padrão de 5000.' },
    { chave: 'tts_local_cmd', rotulo: 'Comando de voz local', tipo: 'texto', secao: 'avancado',
      ajuda: 'Opcional. Programa que recebe o texto na entrada e devolve WAV na saída (ex: Kokoro, piper). Vazio = só ElevenLabs.' },
  ];

  const ROTULO_LEITURA: Record<string, string> = {
    port: 'Porta', lan_bind_ip: 'IP de bind', server_id: 'ID deste servidor',
    public_url: 'URL pública', scan_roots: 'Raízes do scanner',
    terminal_panel: 'Painel de terminal',
  };

  // Vozes e saldo: sob demanda, no botao. As duas chamadas batem no provedor (ElevenLabs) e custam
  // latencia — abrir a tela de config nao pode disparar rede pra fora so por estar aberta.
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
    // Saldo e extra, nao pode quebrar a TELA (sem botao de retry, sem bloquear a voz, que e o
    // principal) — mas a falha ainda tem que aparecer, no mesmo lugar e pelo mesmo motivo que a
    // das vozes: sumir calada e o que a regra do projeto proibe.
    saldoErro = '';
    saldoTts().then((s) => { saldo = s; }).catch((e: Error) => { saldoErro = e.message; });
  }

  // Amostra (Feature A): o proprio ultimo trecho que o usuario mandou ouvir, cortado em 200
  // caracteres — comparar vozes com o texto real do usuario diz mais que uma frase fixa.
  const amostraTexto = $derived(cortarAmostra(ttsPlayer.ultimoTexto));

  function ouvirAmostraDaVoz() {
    const voz = (store.valorAtual('elevenlabs_voice_id') as string) || '';
    ouvirAmostra(amostraTexto, voz);
  }

  // Naturalidade da voz (Feature B): quatro deslizantes que espelham voice_settings da ElevenLabs.
  // O valor e SEMPRE real — o slider nasce no padrao do proprio provedor, nunca "vazio". Backend
  // (tts.py:_ajustes_efetivos) so manda a chave quando o valor foge desse padrao.
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
    { chave: 'tts_stability', rotulo: 'Estabilidade', padrao: 50, min: 0, max: 100,
      esquerda: 'mais emotiva', direita: 'mais constante',
      ajuda: 'Voz mais constante lê igual do começo ao fim; mais emotiva varia o tom, e às vezes erra.' },
    { chave: 'tts_similarity_boost', rotulo: 'Aderência à voz original', padrao: 75, min: 0, max: 100,
      esquerda: 'mais livre', direita: 'mais fiel',
      ajuda: 'Mais fiel gruda na voz gravada original; mais livre dá margem pro modelo variar.' },
    { chave: 'tts_style', rotulo: 'Exagero de estilo', padrao: 0, min: 0, max: 100,
      esquerda: 'neutro', direita: 'marcante',
      ajuda: 'Acentua o jeito característico da voz — passado do ponto, a fala fica exagerada.' },
    { chave: 'tts_speed', rotulo: 'Velocidade da fala', padrao: 100, min: 70, max: 120,
      esquerda: 'mais devagar', direita: 'mais rápido',
      ajuda: 'Ajusta o ritmo da leitura sem mudar o tom da voz.' },
  ];

  // Le o rascunho/salvo como numero; sem valor nenhum (nunca tocou o slider) cai no padrao do
  // PROVEDOR, nao em 0 — e o que faz o slider nascer na posicao certa em vez de encostado na ponta.
  function ajusteValor(a: AjusteSlider): number {
    const bruto = store.valorAtual(a.chave);
    const n = typeof bruto === 'number' ? bruto : parseInt(String(bruto), 10);
    return Number.isFinite(n) ? n : a.padrao;
  }

  function ajusteDefinir(a: AjusteSlider, n: number) {
    store.setRascunho(a.chave, n);
  }

  // "Voltar ao padrao": grava o proprio numero padrao (nao ha como "apagar" a chave do runtime_config
  // — aplicar() so soma/sobrescreve). Gravar o padrao e o backend omitir voice_settings quando o
  // valor == padrao dao no mesmo audio, entao isto e inocuo.
  function ajusteResetar(a: AjusteSlider) {
    store.setRascunho(a.chave, a.padrao);
  }
</script>

<div class="cfg">
  <header class="cfg-head">
    <h2>{TITULOS[secao]}</h2>
    <p class="sub">Valem para este servidor, na hora — sem reiniciar.</p>
  </header>

  {#if store.carregando}
    <p class="aviso">Carregando…</p>
  {:else if store.erro && !Object.keys(store.campos).length}
    <p class="aviso erro">{store.erro}</p>
    <button class="btn" onclick={store.carregar}>Tentar de novo</button>
  {:else}
    <div class="lista">
      {#each CAMPOS.filter((c) => c.secao === secao) as c (c.chave)}
        {@const estado = store.campos[c.chave]}
        <div class="linha" class:liga={c.tipo === 'liga'}>
          <div class="txt">
            <label class="rot" for={`cfg-${c.chave}`}>
              {c.rotulo}
              {#if estado?.origem === 'app'}<span class="tag">editado</span>{/if}
            </label>
            <span class="ajuda">{c.ajuda}</span>
          </div>

          {#if c.tipo === 'liga'}
            <input
              id={`cfg-${c.chave}`}
              class="switch"
              type="checkbox"
              checked={store.valorAtual(c.chave) === true}
              onchange={(e) => store.setRascunho(c.chave, e.currentTarget.checked)}
            />
          {:else if c.tipo === 'numero'}
            <span class="campo-num">
              <input
                id={`cfg-${c.chave}`}
                type="number"
                inputmode="numeric"
                min="0"
                value={store.valorAtual(c.chave)}
                oninput={(e) => store.setRascunho(c.chave, e.currentTarget.value)}
              />
              {#if c.sufixo}<span class="sufixo">{c.sufixo}</span>{/if}
            </span>
          {:else if c.tipo === 'segredo'}
            <!-- O segredo ENTRA mas não sai. O campo fica VAZIO: pré-preencher com a máscara faz
                 qualquer toque no input mandar o texto mascarado de volta e sobrescrever a chave
                 real. A máscara aparece ao lado, como informação, não como valor editável. -->
            {#if estado?.definido}
              <span class="mascara" title="A chave não volta inteira do servidor">
                {estado.valor} <span class="mascara-nota">configurada</span>
              </span>
            {/if}
            <input
              id={`cfg-${c.chave}`}
              class="campo-txt"
              type="text"
              autocomplete="off"
              autocapitalize="off"
              spellcheck={false}
              placeholder={estado?.definido ? 'colar nova chave para trocar' : 'colar a chave'}
              value={store.rascunhoDe(c.chave)}
              oninput={(e) => store.setRascunho(c.chave, e.currentTarget.value)}
            />
          {:else}
            <input
              id={`cfg-${c.chave}`}
              class="campo-txt"
              type="text"
              autocomplete="off"
              autocapitalize="off"
              spellcheck={false}
              value={store.valorAtual(c.chave)}
              oninput={(e) => store.setRascunho(c.chave, e.currentTarget.value)}
            />
          {/if}
        </div>
      {/each}
    </div>

    {#if secao === 'anexos'}
      <div class="tts-extra">
        <h3>Voz da leitura</h3>
        {#if vozErro}
          <p class="aviso erro">{vozErro}</p>
          <button class="btn" onclick={carregarVozes} disabled={carregandoVozes}>Tentar de novo</button>
        {:else if vozes.length}
          <!-- Sem fallback pra vozes[0]: o servidor usa VOZ_PADRAO (tts.py) quando o campo esta
               vazio, que NAO e a primeira voz da conta — mostrar a 1a aqui mentia sobre o que toca.
               A opcao explicita tambem torna a 1a voz da lista escolhivel: com o fallback,
               escolhe-la deixava o value igual ao que ja estava e o onchange nunca disparava. -->
          <Select
            class="campo-select"
            ariaLabel="Voz"
            value={String(store.valorAtual('elevenlabs_voice_id') ?? '')}
            opcoes={[{ value: '', label: 'Padrão do servidor' },
                     ...vozes.map((v) => ({ value: v.id, label: v.nome }))]}
            onchange={(v) => store.setRascunho('elevenlabs_voice_id', v)}
          />
        {:else}
          <button class="btn" onclick={carregarVozes} disabled={carregandoVozes}>
            {carregandoVozes ? 'Carregando…' : 'Carregar vozes da conta'}
          </button>
        {/if}

        <div class="naturalidade">
          {#each AJUSTES_VOZ as a (a.chave)}
            {@const valor = ajusteValor(a)}
            <div class="ajuste">
              <div class="ajuste-cabeca">
                <span class="ajuste-rot">{a.rotulo} <em>{valor}</em></span>
                {#if valor !== a.padrao}
                  <button class="ajuste-reset" onclick={() => ajusteResetar(a)}>voltar ao padrão</button>
                {/if}
              </div>
              <span class="ajuda">{a.ajuda}</span>
              <div class="ajuste-slider">
                <span class="ponta">{a.esquerda}</span>
                <input
                  type="range"
                  aria-label={a.rotulo}
                  min={a.min}
                  max={a.max}
                  step="1"
                  value={valor}
                  oninput={(e) => ajusteDefinir(a, +e.currentTarget.value)}
                />
                <span class="ponta">{a.direita}</span>
              </div>
            </div>
          {/each}
        </div>

        <div class="amostra">
          <button class="btn" onclick={ouvirAmostraDaVoz} disabled={!ttsPlayer.ultimoTexto}>
            🔊 Ouvir amostra desta voz{ttsPlayer.ultimoTexto ? ` · ${amostraTexto.length.toLocaleString('pt-BR')} car.` : ''}
          </button>
          {#if !ttsPlayer.ultimoTexto}
            <span class="ajuda">ouça algum trecho primeiro pra comparar vozes com ele</span>
          {/if}
          <!-- A falha da amostra PRECISA aparecer aqui, e nao so na TtsBar. Esta tela vive dentro de
               um modal cujo veu esta em z-index 100; a barra do player fica em 39 de proposito, pra
               nunca cobrir modal aberto. Sem esta linha, chave invalida ou credito esgotado sao
               desenhados ATRAS do proprio modal e o toque no botao parece nao ter feito nada. -->
          {#if ttsPlayer.error}<p class="aviso erro">{ttsPlayer.error}</p>{/if}
        </div>

        {#if saldo}
          <p class="sub">Consumo do mês: {saldo.usados ?? '?'} de {saldo.limite ?? '?'} caracteres.</p>
        {/if}
        {#if saldoErro}<p class="aviso erro">{saldoErro}</p>{/if}
      </div>
    {/if}

    {#if secao === 'avancado'}
      <div class="somente-leitura">
        <h3>Só pelo servidor</h3>
        <p class="ajuda">
          Mudar qualquer uma exige editar o <code>.env</code> e reiniciar o serviço — por isso não
          são editáveis daqui.
        </p>
        {#each Object.entries(store.leitura) as [k, v] (k)}
          <div class="ro-linha">
            <span class="ro-rot">{ROTULO_LEITURA[k] ?? k}</span>
            <span class="ro-val">{v === '' ? '—' : typeof v === 'boolean' ? (v ? 'sim' : 'não') : v}</span>
          </div>
        {/each}
      </div>
    {/if}
  {/if}

  {#if store.erro && Object.keys(store.campos).length}<p class="aviso erro">{store.erro}</p>{/if}
</div>

{#if !store.carregando && Object.keys(store.campos).length}
  <!-- O rascunho e UM so pras tres fatias, entao Salvar grava tudo que foi mexido, inclusive fora
       desta tela. E o unico significado honesto: com rascunho compartilhado, um Salvar que gravasse so
       a propria fatia faria o MESMO botao significar coisas diferentes conforme a tela. -->
  <div class="rodape">
    {#if store.salvo}<span class="ok">salvo</span>{/if}
    <button class="btn primario" onclick={store.salvar} disabled={!store.temMudanca || store.salvando}>
      {store.salvando ? 'Salvando…' : 'Salvar'}
    </button>
  </div>
{/if}

<style>
  .cfg { padding: var(--space-2) var(--space-4) var(--space-4); }
  .cfg-head h2 { margin: 0; font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); }
  .cfg-head .sub { margin: 2px 0 var(--space-4); font-size: var(--text-xs); color: var(--text-muted); }

  .lista { display: flex; flex-direction: column; }
  .linha {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border-subtle);
  }
  /* Liga/desliga fica na MESMA linha do rótulo: o controle é pequeno e o texto manda. */
  .linha.liga { flex-direction: row; align-items: center; justify-content: space-between; gap: var(--space-4); }

  .txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .rot {
    display: flex; align-items: center; gap: var(--space-2);
    font-size: var(--text-base); font-weight: 600; color: var(--text-primary);
  }
  /* "editado" = veio de override, não do .env — sem isso não dá pra saber de onde o valor vem. */
  .tag {
    font-size: 10px; font-weight: 700; letter-spacing: 0.04em; text-transform: uppercase;
    color: var(--accent); background: var(--accent-dim);
    padding: 1px 6px; border-radius: var(--radius-full);
  }
  /* min-width:0 e o que importa aqui, nao so cosmetica: `.ajuda` e um <span>, e um <span> dentro de
     um flex column (`.txt`, `.ajuste`) tem `min-width:auto` por padrao — o navegador reserva a
     largura do texto INTEIRO sem quebrar, e a frase corta na borda do painel em vez de quebrar linha.
     Vale pra toda ajuda do arquivo (o bug ja existia antes dos sliders, so nao tinha aparecido com
     texto longo o bastante numa tela estreita). */
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; min-width: 0; }

  input[type='text'], input[type='number'] {
    height: 40px;
    background: var(--bg-base);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 16px;                 /* 16px evita o zoom automático do iOS ao focar */
    padding: 0 var(--space-3);
    outline: none;
    min-width: 0;
  }
  input:focus { border-color: var(--accent); }
  .campo-num { display: flex; align-items: center; gap: var(--space-2); }
  .campo-num input { width: 100px; }
  .sufixo { font-size: var(--text-xs); color: var(--text-muted); }

  .mascara {
    display: inline-flex; align-items: baseline; gap: var(--space-2);
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-secondary);
  }
  .mascara-nota { font-family: var(--font-ui); color: var(--success); font-size: 11px; }

  /* `.switch` é global (app.css) — vocabulário único de liga/desliga do app. */

  /* container-type pra o select de voz encolher sem depender da largura da JANELA — quem aperta a
     linha aqui e a largura do PAINEL (dock desktop tem ~530px), nao a viewport. */
  .tts-extra { container-type: inline-size; margin-top: var(--space-2); padding-top: var(--space-3); border-top: 1px solid var(--border-subtle); }
  .tts-extra h3 { margin: 0 0 var(--space-2); font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary); }
  /* :global: o campo é o <button> do Select.svelte. Fonte de UI (nome de voz não é identificador) e
     a mesma regra de container: acima de 360px encolhe pra caber ao lado do resto. */
  .tts-extra :global(.campo-select) {
    width: 100%;
    font-family: var(--font-ui);
    font-size: var(--text-sm);
  }
  @container (min-width: 360px) { .tts-extra :global(.campo-select) { width: auto; min-width: 220px; } }

  .amostra { display: flex; flex-direction: column; gap: var(--space-2); margin-top: var(--space-3); align-items: flex-start; }

  /* Naturalidade da voz: mesmo vocabulario de slider do AppearanceSettings (rotulo + range + valor),
     com as pontas da escala em palavra em vez de numero, e um "voltar ao padrao" por controle. */
  .naturalidade { display: flex; flex-direction: column; gap: var(--space-3); margin: var(--space-3) 0; }
  .ajuste { display: flex; flex-direction: column; gap: 2px; }
  .ajuste-cabeca { display: flex; flex-wrap: wrap; align-items: baseline; justify-content: space-between; gap: var(--space-2); }
  .ajuste-rot { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .ajuste-rot em { margin-left: var(--space-2); font-style: normal; color: var(--text-muted); font-size: var(--text-xs); }
  .ajuste-reset {
    flex-shrink: 0;
    font-size: var(--text-xs); color: var(--accent); background: none; border: none; padding: 0;
  }
  .ajuste-slider { display: flex; align-items: center; gap: var(--space-2); margin-top: var(--space-1); }
  .ajuste-slider input { flex: 1; min-width: 100px; accent-color: var(--accent); }
  .ajuste-slider .ponta {
    font-size: var(--text-xs); color: var(--text-muted); white-space: nowrap; flex-shrink: 0;
  }

  .somente-leitura { margin-top: var(--space-5); }
  .somente-leitura h3 {
    margin: 0 0 4px; font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary);
  }
  .ro-linha {
    display: flex; align-items: baseline; justify-content: space-between; gap: var(--space-4);
    padding: var(--space-2) 0; border-bottom: 1px solid var(--border-subtle);
  }
  .ro-rot { font-size: var(--text-sm); color: var(--text-secondary); }
  .ro-val {
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 60%;
  }

  .aviso { font-size: var(--text-sm); color: var(--text-muted); margin: var(--space-3) 0; }
  .aviso.erro { color: var(--error); }

  /* CHROME FUNCIONAL, sólido de propósito: esta faixa fica GRUDADA no fim da folha enquanto o
     formulário rola por baixo. `--bg-surface` cru aqui não é esquecimento da regra de Transparência
     (CLAUDE.md) — com token de véu o texto do formulário atravessaria os botões. Mesmo caso do
     `.acoes` dos Motores. NÃO converter. */
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
    background: var(--bg-elevated); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 600;
  }
  .btn.primario { background: var(--accent); color: #fff; }
  .btn:disabled { opacity: 0.45; }
</style>
