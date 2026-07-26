<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import ConfirmDialog from './ConfirmDialog.svelte';
  import {
    getEngines, putEngine, deleteEngine, engineModelos,
    type Motor, type ModeloProvedor,
  } from '../lib/api';

  // Motores de modelo: rodar uma sessão em Kimi, num gateway próprio, ou em qualquer endpoint que
  // fale a Messages API — sem perder skills, hooks nem histórico, e sem tocar a conta Anthropic.
  //
  // Nada de catálogo chumbado: os modelos e a janela de contexto vêm do PRÓPRIO provedor
  // (GET /v1/models), com a chave do usuário. O valor muda por faixa de assinatura, então tabela
  // estática mentiria (o plano Moderato do Kimi dá 256k no k3, onde a doc fala de "até 1M").
  interface Props {
    open: boolean;
    onClose: () => void;
  }
  let { open, onClose }: Props = $props();

  const DICAS: { label: string; base_url: string }[] = [
    { label: 'Kimi Code', base_url: 'https://api.kimi.com/coding' },
    { label: 'OmniRoute (o seu)', base_url: 'https://ai.omniwise.com.br' },
  ];

  let motores = $state<Record<string, Motor>>({});
  let carregando = $state(false);
  let erro = $state('');
  let salvando = $state(false);
  // engines.json existe mas não pôde ser lido: bate em `motores: {}` igual a "nunca configurou
  // nada" (o backend não pode derrubar sessão/SSE por um hand-edit ruim), mas a tela TEM que dizer
  // a diferença — senão o usuário vê "nenhum motor ainda", re-adiciona um, e a próxima gravação
  // apaga os outros motores que estavam escondidos atrás do arquivo quebrado.
  let arquivoCorrompido = $state(false);
  let arquivoCaminho = $state('');

  let form = $state<null | {
    nome: string;
    label: string;
    base_url: string;
    // Endereço como veio do disco, pra saber se o usuário mexeu nele (ver `enderecoMudou`).
    base_url_original: string;
    api_key: string;
    api_key_definida: boolean;
    model: string;
    subagent_model: string;
    context_window: string;
    existente: boolean;
  }>(null);

  let modelos = $state<ModeloProvedor[]>([]);
  let buscando = $state(false);
  let erroBusca = $state('');
  let okBusca = $state('');

  $effect(() => {
    if (!open) return;
    carregar();
  });

  async function carregar() {
    carregando = true;
    erro = '';
    try {
      const r = await getEngines();
      motores = r.motores;
      arquivoCorrompido = r.arquivo_corrompido;
      arquivoCaminho = r.arquivo_caminho;
    } catch (e) {
      erro = e instanceof Error ? e.message : 'Falha ao carregar';
    } finally {
      carregando = false;
    }
  }

  function novo() {
    form = {
      nome: '', label: '', base_url: '', base_url_original: '', api_key: '', api_key_definida: false,
      model: '', subagent_model: '', context_window: '', existente: false,
    };
    modelos = []; erroBusca = ''; okBusca = '';
  }

  function editar(nome: string) {
    const m = motores[nome];
    form = {
      nome,
      label: m.label ?? nome,
      base_url: m.base_url,
      base_url_original: m.base_url,
      // Nasce VAZIO de propósito: pré-preencher com a máscara faz qualquer toque mandar o texto
      // mascarado de volta e sobrescrever a chave real.
      api_key: '',
      api_key_definida: m.api_key_definida,
      model: m.model,
      subagent_model: m.subagent_model ?? '',
      context_window: m.context_window ? String(m.context_window) : '',
      existente: true,
    };
    modelos = []; erroBusca = ''; okBusca = '';
  }

  // Motor salvo, key vazia, mas o endereço no formulário já não é o que está no disco: testar
  // agora usaria a key salva contra o endereço ANTIGO (o servidor só aceita nome sozinho), o que
  // silenciosamente ignora a edição. Melhor avisar do que fingir que testou o novo endereço.
  const enderecoMudouSemChave = $derived(
    !!form && form.existente && !form.api_key.trim() && form.base_url.trim() !== form.base_url_original,
  );

  // Busca os modelos no provedor. Dobra de "Testar": 200 = chave boa, erro = mensagem do provedor.
  async function buscarModelos() {
    if (!form) return;
    buscando = true; erroBusca = ''; okBusca = '';
    try {
      // `nome` e `base_url`/`api_key` são mutuamente exclusivos no servidor (400 se vierem juntos)
      // — misturar mandaria a chave salva pra um endereço que o cliente digitou.
      const chave = form.api_key.trim();
      const corpo = chave
        ? { base_url: form.base_url.trim(), api_key: chave }
        : { nome: form.nome };
      const r = await engineModelos(corpo);
      modelos = r.modelos;
      okBusca = `${r.modelos.length} modelo(s) — conexão e chave OK`;
      const atual = modelos.find((m) => m.id === form!.model) ?? modelos[0];
      if (atual) escolherModelo(atual.id);
    } catch (e) {
      erroBusca = e instanceof Error ? e.message : 'Falha ao consultar o provedor';
    } finally {
      buscando = false;
    }
  }

  function escolherModelo(id: string) {
    if (!form) return;
    form.model = id;
    const m = modelos.find((x) => x.id === id);
    // A janela vem do provedor: errar aqui custa capacidade real (em branco o Claude Code assume
    // 200k e compacta cedo, mesmo num modelo de 500k). Modelo sem context_length limpa o campo —
    // deixar o número do modelo ANTERIOR salvo faria a var passar da janela real do modelo novo, e
    // o provedor erra no meio do turno em vez do Claude Code compactar.
    form.context_window = m?.context_length ? String(m.context_length) : '';
  }

  const modeloAtual = $derived(form ? modelos.find((m) => m.id === form!.model) : undefined);

  async function salvar() {
    if (!form) return;
    salvando = true; erro = '';
    try {
      const corpo: Record<string, unknown> = {
        label: form.label.trim() || form.nome.trim(),
        base_url: form.base_url.trim(),
        model: form.model.trim(),
      };
      if (form.api_key.trim()) corpo.api_key = form.api_key.trim();
      if (form.subagent_model.trim()) corpo.subagent_model = form.subagent_model.trim();
      if (form.context_window) corpo.context_window = Number(form.context_window);

      // O PUT é substituição TOTAL do registro (backend/app/engines.py salvar()/_normalizar()):
      // omitir um campo aqui não é "deixar como estava", é apagá-lo no disco. `modeloAtual` só
      // existe depois de um Testar NESTA sessão do form — editar só o rótulo/janela e salvar sem
      // retestar não pode perder vision/tool_search calado. Precedência: valor recém-testado >
      // valor já salvo pro motor > omitir (motor novo, nunca testado).
      const salvo = motores[form.nome];
      const vision = typeof modeloAtual?.vision === 'boolean' ? modeloAtual.vision : salvo?.vision;
      if (typeof vision === 'boolean') corpo.vision = vision;
      // Nada na UI seta tool_search hoje; isto só preserva o que o motor já tinha gravado.
      if (typeof salvo?.tool_search === 'boolean') corpo.tool_search = salvo.tool_search;

      motores = (await putEngine(form.nome.trim(), corpo)).motores;
      form = null;
    } catch (e) {
      erro = e instanceof Error ? e.message : 'Falha ao salvar';
    } finally {
      salvando = false;
    }
  }

  // Confirmação de remoção: sheet temática, não o confirm() nativo do navegador (quebra o tema
  // escuro). ConfirmDialog (não ConfirmSheet) de propósito — EnginesSheet já É um BottomSheet
  // aberto, e ConfirmSheet também embrulha um BottomSheet, cujo <svelte:window onkeydown> de Escape
  // registraria um SEGUNDO handler global disputando com o desta sheet (a mesma race de foco que
  // motivou tirar o EnginesSheet de dentro do ConfigSheet). ConfirmDialog trata Escape só no próprio
  // elemento do diálogo — é o padrão que o LoopSheet já usa pro mesmo caso (confirm dentro de sheet
  // aberta).
  let confirmRemoverNome = $state<string | null>(null);

  function remover(nome: string) {
    confirmRemoverNome = nome;
  }

  async function removerConfirmado() {
    const nome = confirmRemoverNome;
    confirmRemoverNome = null;
    if (!nome) return;
    erro = '';
    try {
      await deleteEngine(nome);
      const { [nome]: _fora, ...resto } = motores;
      motores = resto;
    } catch (e) {
      erro = e instanceof Error ? e.message : 'Falha ao remover';
    }
  }
</script>

<BottomSheet {open} {onClose} ariaLabel="Motores de modelo">
  <div class="mot">
    <header class="mot-head">
      <h2>Motores de modelo</h2>
      <p class="sub">
        Rode uma sessão em outro modelo sem perder skills, hooks nem histórico. Sua conta Anthropic
        fica intocada — o motor vale só para a sessão que você abrir com ele.
      </p>
    </header>

    {#if carregando}
      <p class="aviso">Carregando…</p>
    {:else if form}
      <div class="form">
        <label class="campo">
          <span class="rot">Nome curto</span>
          <input type="text" placeholder="kimi" autocapitalize="off" spellcheck={false}
                 disabled={form.existente}
                 value={form.nome} oninput={(e) => (form!.nome = e.currentTarget.value)} />
          <span class="ajuda">
            No terminal você usa <code>claude-engine {form.nome || 'kimi'}</code>. Minúsculas, sem espaço.
          </span>
        </label>

        <label class="campo">
          <span class="rot">Endereço da API</span>
          <input type="text" autocapitalize="off" spellcheck={false} placeholder="https://…"
                 value={form.base_url} oninput={(e) => (form!.base_url = e.currentTarget.value)} />
          <span class="ajuda">
            <strong>Sem o /v1 no fim</strong> — o Claude Code monta o caminho sozinho. Precisa falar a
            Messages API da Anthropic; gateway só-OpenAI exige um proxy tradutor em 127.0.0.1.
            Endereço público tem que ser https, senão a chave viaja em claro.
          </span>
          <span class="dicas">
            {#each DICAS as d (d.base_url)}
              <button class="dica" onclick={() => (form!.base_url = d.base_url)}>{d.label}</button>
            {/each}
          </span>
        </label>

        <label class="campo">
          <span class="rot">Chave da API</span>
          {#if form.api_key_definida}
            <span class="def">chave configurada — deixe vazio para manter</span>
          {/if}
          <input type="text" autocomplete="off" autocapitalize="off" spellcheck={false}
                 placeholder={form.api_key_definida ? 'colar nova chave para trocar' : 'colar a chave'}
                 value={form.api_key} oninput={(e) => (form!.api_key = e.currentTarget.value)} />
        </label>

        <div class="campo">
          <button class="btn" onclick={buscarModelos}
                  disabled={buscando || !form.base_url.trim() || (!form.api_key.trim() && !form.api_key_definida)}>
            {buscando ? 'Consultando…' : 'Testar e listar modelos'}
          </button>
          {#if okBusca}<span class="ok">{okBusca}</span>{/if}
          {#if enderecoMudouSemChave}
            <!-- Testar sem chave nova reusa a key salva, mas o servidor só aceita isso com o
                 endereço TAMBÉM salvo — o endereço que você acabou de editar aqui não entra no
                 teste até você colar a chave de novo. -->
            <span class="ajuda erro">Você mudou o endereço: para testar um endereço novo, cole a chave também.</span>
          {/if}
          <!-- A mensagem crua do provedor é a informação útil ("401 Invalid Authentication"). -->
          {#if erroBusca}<span class="ajuda erro">{erroBusca}</span>{/if}
        </div>

        <label class="campo">
          <span class="rot">Modelo</span>
          {#if modelos.length}
            <select value={form.model} onchange={(e) => escolherModelo(e.currentTarget.value)}>
              {#each modelos as m (m.id)}
                <option value={m.id}>
                  {m.id}{m.context_length ? ` · ${Math.round(m.context_length / 1000)}k` : ''}
                </option>
              {/each}
            </select>
          {:else}
            <input type="text" placeholder="id do modelo" autocapitalize="off" spellcheck={false}
                   value={form.model} oninput={(e) => (form!.model = e.currentTarget.value)} />
            <span class="ajuda">Use "Testar" acima para listar os ids do seu provedor.</span>
          {/if}
          {#if modeloAtual?.vision === false}
            <!-- O app manda foto do celular; motor cego quebra esse fluxo e o usuário tem que saber
                 ANTES de abrir a sessão, não quando a foto for ignorada. -->
            <span class="ajuda erro">Este modelo não enxerga imagem: anexar foto não vai funcionar.</span>
          {/if}
        </label>

        <label class="campo">
          <span class="rot">Modelo dos subagentes</span>
          {#if modelos.length}
            <select value={form.subagent_model}
                    onchange={(e) => (form!.subagent_model = e.currentTarget.value)}>
              <option value="">mesmo que o principal</option>
              {#each modelos as m (m.id)}
                <option value={m.id}>{m.id}</option>
              {/each}
            </select>
          {:else}
            <input type="text" placeholder="vazio = mesmo que o principal" autocapitalize="off" spellcheck={false}
                   value={form.subagent_model} oninput={(e) => (form!.subagent_model = e.currentTarget.value)} />
          {/if}
          <span class="ajuda">
            Subagentes fazem busca mecânica repetitiva; um modelo mais barato aqui é economia real,
            sem tocar no modelo principal da sessão.
          </span>
        </label>

        <label class="campo">
          <span class="rot">Janela de contexto</span>
          <input type="number" inputmode="numeric" min="1" placeholder="tokens"
                 value={form.context_window}
                 oninput={(e) => (form!.context_window = e.currentTarget.value)} />
          <span class="ajuda">
            Vem do provedor ao testar. Em branco, o Claude Code assume 200k e compacta cedo mesmo num
            modelo maior — capacidade jogada fora. Confira na sessão com <code>/context</code>.
          </span>
        </label>

        {#if erro}<p class="aviso erro">{erro}</p>{/if}

        <div class="acoes">
          <button class="btn" onclick={() => (form = null)}>Cancelar</button>
          <button class="btn primario" onclick={salvar}
                  disabled={salvando || !form.nome.trim() || !form.model.trim() || !form.base_url.trim()}>
            {salvando ? 'Salvando…' : 'Salvar'}
          </button>
        </div>
      </div>
    {:else}
      {#if arquivoCorrompido}
        <!-- Não é "nenhum motor configurado": o arquivo existe mas está com erro de formato, e
             pode estar escondendo motores reais (com as keys deles) atrás do erro de leitura.
             Adicionar um motor agora falha ao salvar de propósito (o backend recusa sobrescrever
             um arquivo que não conseguiu ler) — melhor isso do que apagar o que já está lá. -->
        <p class="aviso erro">
          Não consegui ler <code>{arquivoCaminho}</code> — o arquivo existe mas está com erro de
          formato (JSON inválido). Se você já tinha motores configurados, eles continuam no
          arquivo: corrija-o à mão (ou restaure um backup) antes de adicionar um novo.
        </p>
      {:else if !Object.keys(motores).length}
        <p class="aviso">
          Nenhum motor ainda. Adicione um e ele aparece no seletor ao criar sessão, e no terminal como
          <code>claude-engine &lt;nome&gt;</code>.
        </p>
      {/if}
      <div class="lista">
        {#each Object.entries(motores) as [nome, m] (nome)}
          <div class="card">
            <div class="card-txt">
              <span class="card-nome">{m.label ?? nome}</span>
              <span class="card-sub">{m.model}{m.context_window ? ` · ${Math.round(m.context_window / 1000)}k` : ''}</span>
              <span class="card-url">{m.base_url}</span>
              <span class="card-key">{m.api_key}</span>
            </div>
            <div class="card-acoes">
              <button class="btn" onclick={() => editar(nome)}>Editar</button>
              <button class="btn perigo" onclick={() => remover(nome)}>Remover</button>
            </div>
          </div>
        {/each}
      </div>
      {#if erro}<p class="aviso erro">{erro}</p>{/if}
      <button class="btn primario largo" onclick={novo}>Adicionar motor</button>
    {/if}
  </div>
</BottomSheet>

{#if confirmRemoverNome}
  <ConfirmDialog title={`Remover o motor "${confirmRemoverNome}"?`} aria="Remover motor"
    onClose={() => (confirmRemoverNome = null)}
    actions={[
      { label: 'Cancelar', onClick: () => (confirmRemoverNome = null) },
      { label: 'Remover', kind: 'danger', onClick: removerConfirmado },
    ]}>
    <p class="ajuda">Sessões abertas nele continuam rodando.</p>
  </ConfirmDialog>
{/if}

<style>
  .mot { padding: var(--space-2) var(--space-4) var(--space-4); }
  .mot-head h2 { margin: 0; font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); }
  .mot-head .sub { margin: 2px 0 var(--space-4); font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; }

  .lista { display: flex; flex-direction: column; gap: var(--space-3); }
  .card {
    display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-3);
    padding: var(--space-3);
    background: var(--bg-elevated);
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
  }
  .card-txt { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .card-nome { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .card-sub { font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-secondary); }
  .card-url, .card-key {
    font-family: var(--font-mono); font-size: 11px; color: var(--text-muted);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; max-width: 100%;
  }
  .card-acoes { display: flex; flex-direction: column; gap: var(--space-2); flex-shrink: 0; }

  .form { display: flex; flex-direction: column; gap: var(--space-4); }
  .campo { display: flex; flex-direction: column; gap: var(--space-2); }
  .rot { font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; }
  .ajuda.erro { color: var(--error); }
  .def { font-size: 11px; color: var(--success); }
  .ok { font-size: var(--text-xs); color: var(--success); }
  .dicas { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .dica {
    background: var(--bg-elevated); color: var(--text-secondary);
    border: 1px solid var(--border-subtle); border-radius: var(--radius-full);
    padding: 2px 10px; font-size: 11px;
  }

  input[type='text'], input[type='number'], select {
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
  input:focus, select:focus { border-color: var(--accent); }
  input:disabled { opacity: 0.6; }

  .acoes { display: flex; justify-content: flex-end; gap: var(--space-3); }
  .aviso { font-size: var(--text-sm); color: var(--text-muted); margin: var(--space-3) 0; }
  .aviso.erro { color: var(--error); }
  .btn {
    height: 40px; padding: 0 var(--space-4);
    border-radius: var(--radius-md);
    background: var(--bg-elevated); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 600;
  }
  .btn.primario { background: var(--accent); color: #fff; }
  .btn.perigo { color: var(--error); }
  .btn.largo { width: 100%; margin-top: var(--space-4); }
  .btn:disabled { opacity: 0.45; }
</style>
