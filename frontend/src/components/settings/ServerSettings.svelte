<script lang="ts">
  import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
  import { listarVozesTts, saldoTts, type TtsVoz } from '../../lib/api';

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
          <select
            class="campo-select"
            aria-label="Voz"
            value={store.valorAtual('elevenlabs_voice_id') || ''}
            onchange={(e) => store.setRascunho('elevenlabs_voice_id', e.currentTarget.value)}
          >
            <!-- Sem fallback pra vozes[0]: o servidor usa VOZ_PADRAO (tts.py) quando o campo esta
                 vazio, que NAO e a primeira voz da conta — mostrar a 1a aqui mentia sobre o que
                 toca. A opcao explicita tambem torna a 1a voz da lista escolhivel: com o fallback,
                 escolhe-la deixava o value igual ao que ja estava e o onchange nunca disparava. -->
            <option value="">Padrão do servidor</option>
            {#each vozes as v (v.id)}<option value={v.id}>{v.nome}</option>{/each}
          </select>
        {:else}
          <button class="btn" onclick={carregarVozes} disabled={carregandoVozes}>
            {carregandoVozes ? 'Carregando…' : 'Carregar vozes da conta'}
          </button>
        {/if}
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
            <span class="ro-val">{v === '' ? '—' : v}</span>
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
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; }

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
  .campo-select {
    width: 100%; height: 40px;
    background: var(--surface-inset);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-size: var(--text-sm);
    padding: 0 var(--space-3);
  }
  @container (min-width: 360px) { .campo-select { width: auto; min-width: 220px; } }

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
