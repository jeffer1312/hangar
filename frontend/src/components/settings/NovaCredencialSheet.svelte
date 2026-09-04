<script lang="ts">
  // Modal de "+ Nova conta" da aba Contas, em dois passos: escolher DE ONDE, e só então preencher.
  //
  // Por que modal e não a fileira de botões que estava no rodapé (18/08/2026): com a escolha inline,
  // "+ Nova conta · O que você quer adicionar? · Conta do Claude (login) · Chave de API · Cancelar"
  // virava uma linha de cinco controles competindo pela largura, e a pergunta ficava do mesmo tamanho
  // dos botões. O molde é o catálogo de provedores do OpenCode Desktop, que o usuário indicou como
  // referência: uma linha por provedor, com nome, o que é, e um botão de conectar.
  //
  // O passo 2 busca os MODELOS da própria URL (`POST /api/engines/modelos`, que já existia pra tela
  // de Motores) e mostra o que voltou. Isso não é enfeite: uma chave só serve se algum modelo
  // responder, então a lista é a prova de que a credencial funciona — e é o que dispensa o usuário de
  // digitar id de modelo à mão, como o formulário da referência exige.
  import BottomSheet from '../BottomSheet.svelte';
  import ProvedorIcone from '../icons/ProvedorIcone.svelte';
  import * as m from '../../paraglide/messages';
  import { engineModelos, engineModelosForServer, putEngine, putEngineForServer,
           criarConta, type ModeloProvedor } from '../../lib/api';
  import { sincronizarNosAgentes, codexLoginIniciar, codexLoginPasso, codexLoginCancelar,
           type ResultadoSync, type PassoCodex } from '../../lib/credenciais';
  import type { Server } from '../../lib/auth';
  import { onDestroy } from 'svelte';

  interface Props {
    apiTarget: Server | null;
    onFechar: () => void;
    // Chamado depois de criar — quem abriu recarrega a lista (fonte única, não insere item na mão).
    onCriada: () => void;
  }
  let { apiTarget, onFechar, onCriada }: Props = $props();

  // Catálogo. `url` vazia = o usuário digita (provedor personalizado); `login` = conta do Claude por
  // assinatura, que não tem URL nem chave. Só entra aqui provedor que a gente sabe usar de verdade —
  // uma linha bonita que não conecta é pior que uma lista curta.
  // `url` é a RAIZ do provedor, sem `/v1`: quem consome acrescenta o dialeto (o backend monta
  // `{base}/v1/models` pra descobrir os modelos, e o Claude Code monta `{base}/v1/...`). Colar a
  // URL como o provedor documenta ("…/coding/v1") virava `/v1/v1/models` — 404 e zero modelo. O
  // backend também tira o `/v1` sozinho agora, então o campo aceita as duas formas.
  // `login: 'codex'` = conta do ChatGPT por código de dispositivo, que o servidor espalha pro
  // Codex, Pi e omp (o mesmo OAuth nos três) — nem URL nem chave nem nome.
  type Item = { id: string; nome: string; desc: string; url: string; login?: 'claude' | 'codex' };
  const CATALOGO: Item[] = [
    { id: 'claude', nome: m.novacred_claude_nome(), desc: m.novacred_claude_desc(), url: '', login: 'claude' },
    { id: 'codex', nome: m.novacred_codex_nome(), desc: m.novacred_codex_desc(), url: '', login: 'codex' },
    { id: 'opencode', nome: 'OpenCode Zen', desc: m.novacred_opencode_desc(), url: 'https://opencode.ai/zen' },
    { id: 'kimi', nome: 'Kimi Code', desc: m.novacred_kimi_desc(), url: 'https://api.kimi.com/coding' },
    { id: 'anthropic', nome: 'Anthropic', desc: m.novacred_anthropic_desc(), url: 'https://api.anthropic.com' },
    { id: 'openrouter', nome: 'OpenRouter', desc: m.novacred_openrouter_desc(), url: 'https://openrouter.ai/api' },
    { id: 'groq', nome: 'Groq', desc: m.novacred_groq_desc(), url: 'https://api.groq.com/openai' },
    { id: 'deepseek', nome: 'DeepSeek', desc: m.novacred_deepseek_desc(), url: 'https://api.deepseek.com' },
    { id: 'custom', nome: m.novacred_custom_nome(), desc: m.novacred_custom_desc(), url: '' },
  ];

  let escolhido = $state<Item | null>(null);
  let nome = $state('');
  let url = $state('');
  let chave = $state('');
  let salvando = $state(false);
  let erro = $state('');
  // Resultado da gravação nos OUTROS agentes. Fica na tela depois de salvar, porque tem uma coisa
  // que o usuário precisa fazer à mão: o Codex guarda só o NOME da variável de ambiente, então a
  // chave só passa a valer lá quando ele exportar a variável. Esconder isso seria prometer uma
  // integração que não acontece.
  let sync = $state<ResultadoSync | null>(null);
  let sincronizando = $state(false);

  // Modelos lidos da própria URL. `null` = ainda não buscou; `[]` = buscou e não veio nada.
  let modelos = $state<ModeloProvedor[] | null>(null);
  let buscando = $state(false);
  let erroModelos = $state('');
  let modeloEscolhido = $state('');

  // Login do Codex: o servidor pede o código e faz o poll; aqui só o passo e o intervalo de leitura.
  let codex = $state<PassoCodex>({ etapa: 'idle' });
  let codexPoll: ReturnType<typeof setInterval> | null = null;
  // Geração da tentativa: cancelar/fechar com o iniciar ainda em voo invalida a resposta que
  // chega depois — senão ela rearmava o poll num componente já desmontado.
  let codexGer = 0;

  function pararCodex() {
    if (codexPoll) { clearInterval(codexPoll); codexPoll = null; }
  }

  async function iniciarCodex() {
    const alvo = apiTarget;
    const g = ++codexGer;
    erro = '';
    codex = { etapa: 'idle' };
    let passo: PassoCodex;
    try {
      passo = await codexLoginIniciar(alvo);
    } catch (e) {
      if (g === codexGer) erro = e instanceof Error && e.message ? e.message : String(e);
      return;
    }
    if (g !== codexGer) {
      // Cancelado enquanto o servidor criava a tentativa: ela existe lá e precisa morrer.
      codexLoginCancelar(alvo).catch(() => {});
      return;
    }
    codex = passo;
    codexPoll = setInterval(async () => {
      try {
        const p = await codexLoginPasso(alvo);
        if (g !== codexGer) return;
        codex = p;
      } catch { /* erro de rede no poll: a próxima leitura tenta de novo */ }
      if (codex.etapa !== 'aguardando') {
        pararCodex();
        if (codex.etapa === 'concluido') onCriada();
      }
    }, 2000);
  }

  function cancelarCodex() {
    pararCodex();
    if (codex.etapa === 'aguardando') codexLoginCancelar(apiTarget).catch(() => {});
    codexGer++;
    codex = { etapa: 'idle' };
  }
  onDestroy(cancelarCodex);

  function abrir(item: Item) {
    escolhido = item;
    nome = item.login ? '' : item.nome;
    url = item.url;
    chave = '';
    modelos = null;
    modeloEscolhido = '';
    erro = '';
    erroModelos = '';
    sync = null;
    if (item.login === 'codex') iniciarCodex();
  }

  function voltar() {
    cancelarCodex();
    escolhido = null;
    erro = '';
  }

  const podeBuscar = $derived(!!url.trim() && !!chave.trim() && !buscando);

  async function buscarModelos() {
    if (!podeBuscar) return;
    buscando = true;
    erroModelos = '';
    modelos = null;
    // Alvo capturado AGORA. Buscar com a URL/chave do provedor A e editar os campos pro B antes da
    // resposta chegar fazia a lista de A aterrissar no formulário de B — e salvar mandaria um
    // `model` que só existe no catálogo de A. Descarta em vez de desabilitar o campo: quem digitou
    // errado tem que poder corrigir sem esperar a resposta do errado.
    const alvoUrl = url.trim();
    const alvoChave = chave.trim();
    try {
      const corpo = { base_url: alvoUrl, api_key: alvoChave };
      const r = apiTarget ? await engineModelosForServer(apiTarget, corpo) : await engineModelos(corpo);
      if (alvoUrl !== url.trim() || alvoChave !== chave.trim()) return;
      modelos = r.modelos;
      // Primeiro modelo como padrão: quem cadastra uma chave quase sempre quer o primeiro que o
      // provedor lista, e deixar vazio faria o salvar nascer sem modelo nenhum.
      if (r.modelos.length && !modeloEscolhido) modeloEscolhido = r.modelos[0].id;
    } catch (e) {
      if (alvoUrl !== url.trim() || alvoChave !== chave.trim()) return;
      // A mensagem do provedor é o dado mais útil aqui (401, host errado, chave de outro serviço):
      // trocá-la por texto genérico deixaria o usuário sem pista do que corrigir.
      erroModelos = e instanceof Error && e.message ? e.message : String(e);
    } finally {
      buscando = false;
    }
  }

  // O engines.json tem alfabeto próprio pro nome (minúsculas, números, '-', '_'): o nome bonito vai
  // pro `label` e o id sai daqui. Sem isto, "Meu Provedor" seria recusado com 400 e o usuário
  // levaria a culpa por ter digitado um espaço.
  function idDe(texto: string): string {
    const base = texto.toLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '')
      .replace(/[^a-z0-9_-]+/g, '-').replace(/^-+|-+$/g, '').slice(0, 32);
    return base || 'chave';
  }

  async function salvar() {
    if (salvando || !escolhido) return;
    salvando = true;
    erro = '';
    try {
      if (escolhido.login) {
        await criarConta(apiTarget, idDe(nome));
      } else {
        const dados = {
          label: nome.trim() || escolhido.nome,
          base_url: url.trim(),
          api_key: chave.trim(),
          model: modeloEscolhido,
        };
        const id = idDe(nome.trim() || escolhido.nome);
        if (apiTarget) await putEngineForServer(apiTarget, id, dados);
        else await putEngine(id, dados);
        // A chave já está no servidor: zerar aqui tira o segredo do estado do componente, que
        // continua montado mostrando o resultado da sincronização. A sincronização abaixo não
        // precisa dela (manda só o id — o servidor já tem a chave).
        chave = '';
        onCriada();
        // Gravar nos outros agentes é o ponto do cadastro único — sem isso o usuário volta a
        // cadastrar a mesma chave no Pi, no Kimi e no Codex à mão. Falha aqui NÃO desfaz a
        // credencial (ela vale pro Claude Code de qualquer jeito) e não fecha o modal: o resultado
        // por agente fica na tela, inclusive a variável que o Codex espera.
        sincronizando = true;
        try {
          sync = await sincronizarNosAgentes(apiTarget, `chave:${id}`);
        } catch (e) {
          erro = e instanceof Error && e.message ? e.message : String(e);
        } finally {
          sincronizando = false;
        }
        return;
      }
      onCriada();
      onFechar();
    } catch (e) {
      erro = e instanceof Error && e.message ? e.message : String(e);
    } finally {
      salvando = false;
    }
  }

  const podeSalvar = $derived(
    !!escolhido && !salvando &&
    (escolhido.login ? !!nome.trim() : !!nome.trim() && !!url.trim() && !!chave.trim()),
  );

  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });
</script>

<BottomSheet open={true} onClose={onFechar} ariaLabel={m.contas_nova()}
             wide={isDesktop} centered={isDesktop}>
  <div class="nc">
    <div class="nc-topo">
      {#if escolhido}
        <button type="button" class="nc-voltar" onclick={voltar} aria-label={m.comum_voltar()}>←</button>
      {/if}
      {#if escolhido}
        <ProvedorIcone tipo={escolhido.login === 'claude' ? 'claude' : 'chave'} baseUrl={escolhido.url}
          iniciais={escolhido.nome.slice(0, 2).toUpperCase()} size={26} />
      {/if}
      <h2 class="nc-titulo">{escolhido ? escolhido.nome : m.contas_nova_escolha()}</h2>
    </div>

    {#if !escolhido}
      <div class="nc-lista">
        {#each CATALOGO as item (item.id)}
          <div class="nc-item">
            <ProvedorIcone tipo={item.login === 'claude' ? 'claude' : 'chave'} baseUrl={item.url}
              iniciais={item.nome.slice(0, 2).toUpperCase()} size={30} />
            <span class="nc-item-txt">
              <span class="nc-item-nome">{item.nome}</span>
              <span class="nc-item-desc">{item.desc}</span>
            </span>
            <button type="button" class="nc-conectar" onclick={() => abrir(item)}
              >+ {m.novacred_conectar()}</button>
          </div>
        {/each}
      </div>
    {:else if escolhido.login === 'codex'}
      <p class="nc-leg">{escolhido.desc}</p>
      {#if codex.etapa === 'aguardando' || codex.etapa === 'concluido'}
        <div class="nc-modelos">
          <p class="nc-sync-linha"><b>1</b>{m.novacred_codex_passo1()}</p>
          <a class="nc-codex-link" href={codex.url} target="_blank" rel="noopener noreferrer">{codex.url}</a>
          <p class="nc-sync-linha"><b>2</b>{m.novacred_codex_passo2()}</p>
          <p class="nc-codex-codigo">{codex.user_code}</p>
          <p class="nc-sync-linha" class:pulado={codex.etapa !== 'concluido'}>
            <b>3</b>{codex.etapa === 'concluido' ? m.novacred_codex_concluido() : m.novacred_codex_aguardando()}
          </p>
        </div>
      {/if}
      {#if codex.etapa === 'concluido' && codex.resultado}
        <div class="nc-modelos">
          <span class="nc-modelos-tit">{m.novacred_sync_titulo()}</span>
          {#each Object.entries(codex.resultado) as [alvo, r] (alvo)}
            <p class="nc-sync-linha" class:pulado={!r.ok && r.motivo === 'nao-instalado'}
               class:falhou={!r.ok && r.motivo !== 'nao-instalado'}>
              <b>{alvo}</b>
              {r.ok ? (r.motivo === 'ja-logado' ? m.novacred_codex_ja_logado() : m.novacred_sync_ok())
                    : (r.motivo === 'nao-instalado' ? m.novacred_sync_nao_instalado() : r.motivo)}
            </p>
          {/each}
        </div>
      {/if}
      {#if codex.etapa === 'falhou'}<p class="nc-erro" role="alert">{codex.erro}</p>
      {:else if erro}<p class="nc-erro" role="alert">{erro}</p>{/if}
      <div class="nc-rodape">
        {#if codex.etapa === 'falhou' || (codex.etapa === 'idle' && erro)}
          <button type="button" class="nc-btn primario" onclick={iniciarCodex}>{m.novacred_codex_tentar()}</button>
        {/if}
        <button type="button" class="nc-btn" onclick={() => { cancelarCodex(); onFechar(); }}
          >{codex.etapa === 'concluido' ? m.sessao_fechar() : m.comum_cancelar()}</button>
      </div>
    {:else}
      <p class="nc-leg">{escolhido.desc}</p>

      <label class="nc-campo">
        <span>{escolhido.login ? m.novacred_nome_conta() : m.contas_chave_nome()}</span>
        <!-- svelte-ignore a11y_autofocus -->
        <input type="text" autofocus bind:value={nome} disabled={salvando}
          placeholder={escolhido.login ? m.criar_conta_placeholder() : escolhido.nome} />
      </label>

      {#if !escolhido.login}
        <label class="nc-campo">
          <span>{m.contas_chave_url()}</span>
          <input type="url" bind:value={url} disabled={salvando}
            placeholder="https://api.exemplo.com" />
        </label>
        <label class="nc-campo">
          <span>{m.contas_chave_segredo()}</span>
          <input type="password" autocomplete="off" bind:value={chave} disabled={salvando} />
        </label>

        <div class="nc-modelos">
          <div class="nc-modelos-topo">
            <span class="nc-modelos-tit">{m.novacred_modelos()}</span>
            <button type="button" class="nc-btn" onclick={buscarModelos} disabled={!podeBuscar}
              >{buscando ? '…' : m.novacred_buscar_modelos()}</button>
          </div>
          {#if erroModelos}
            <!-- A mensagem CRUA do provedor: é ela que diz "401", "host desconhecido" ou "chave de
                 outro serviço", e é o que faz o usuário saber o que corrigir. -->
            <p class="nc-erro" role="alert">{erroModelos}</p>
          {:else if modelos === null}
            <p class="nc-modelos-vazio">{m.novacred_modelos_dica()}</p>
          {:else if modelos.length === 0}
            <p class="nc-modelos-vazio">{m.novacred_modelos_nenhum()}</p>
          {:else}
            <!-- Lista preenchida pelo PROVEDOR, não digitada: o formulário da referência pede id de
                 modelo à mão, e id errado só aparece como erro na primeira mensagem. -->
            <select class="nc-select" bind:value={modeloEscolhido} disabled={salvando}>
              {#each modelos as mo (mo.id)}
                <option value={mo.id}>{mo.id}{mo.context_length ? ` · ${Math.round(mo.context_length / 1000)}k` : ''}</option>
              {/each}
            </select>
            <p class="nc-modelos-ok">{m.novacred_modelos_achados({ n: String(modelos.length) })}</p>
          {/if}
        </div>
      {:else}
        <p class="nc-modelos-vazio">{m.novacred_login_depois()}</p>
      {/if}

      {#if sincronizando}<p class="nc-modelos-vazio">{m.novacred_salvando_sync()}</p>{/if}
      {#if sync}
        <div class="nc-modelos">
          <span class="nc-modelos-tit">{m.novacred_sync_titulo()}</span>
          {#each Object.entries(sync.resultado) as [alvo, r] (alvo)}
            <p class="nc-sync-linha" class:pulado={!r.ok && r.motivo === 'nao-instalado'}
               class:falhou={!r.ok && r.motivo !== 'nao-instalado'}>
              <b>{alvo}</b>
              {r.ok ? m.novacred_sync_ok() : (r.motivo === 'nao-instalado' ? m.novacred_sync_nao_instalado() : r.motivo)}
            </p>
          {/each}
          {#if sync.resultado.codex?.ok}
            {@const varNome = (sync.resultado.codex.motivo.match(/exporte (\S+)/) ?? [])[1]}
            {#if varNome}<p class="nc-modelos-ok">{m.novacred_sync_codex_var({ v: varNome })}</p>{/if}
          {/if}
        </div>
      {/if}
      {#if erro}<p class="nc-erro" role="alert">{erro}</p>{/if}

      <div class="nc-rodape">
        {#if !sync}
          <button type="button" class="nc-btn primario" onclick={salvar} disabled={!podeSalvar}
            >{salvando ? '…' : m.ctx_salvar()}</button>
        {/if}
        <button type="button" class="nc-btn" onclick={onFechar} disabled={salvando || sincronizando}
          >{sync ? m.sessao_fechar() : m.comum_cancelar()}</button>
      </div>
    {/if}
  </div>
</BottomSheet>

<style>
  /* Container query e não media query: quem aperta a linha é a largura do PAINEL (no desktop a
     folha centrada tem largura própria), não a da janela. */
  .nc { container-type: inline-size; padding: var(--space-2) 0 var(--space-3); }
  .nc-topo { display: flex; align-items: center; gap: var(--space-2); margin-bottom: var(--space-2); }
  .nc-titulo { margin: 0; font-size: var(--text-base); font-weight: 600; color: var(--text-primary); }
  .nc-voltar {
    background: transparent; border: none; color: var(--text-secondary);
    font-size: 16px; line-height: 1; cursor: pointer; padding: 0 var(--space-1);
  }
  .nc-leg { color: var(--text-muted); font-size: var(--text-xs); margin: 0 0 var(--space-3); }

  /* Catálogo: uma linha por provedor, separada por régua e não por caixa — é lista, não cartão. */
  .nc-lista { border: 1px solid var(--border-subtle); border-radius: 10px; overflow: hidden; }
  .nc-item {
    display: flex; align-items: center; gap: var(--space-3);
    padding: var(--space-3); border-bottom: 1px solid var(--border-subtle);
    background: var(--surface-raised);
  }
  .nc-item:last-child { border-bottom: none; }
  .nc-item-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .nc-item-nome { color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .nc-item-desc { color: var(--text-muted); font-size: var(--text-xs); }
  .nc-conectar {
    flex-shrink: 0; white-space: nowrap;
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    color: var(--text-primary); border-radius: 8px; padding: 6px 12px;
    font: inherit; font-size: var(--text-xs); cursor: pointer;
  }
  .nc-conectar:hover { border-color: var(--border-strong); }

  .nc-campo { display: flex; flex-direction: column; gap: 4px; margin-bottom: var(--space-3); }
  .nc-campo > span { font-size: 11.5px; color: var(--text-secondary); }
  /* Campo é superfície de ENTRADA: --surface-inset (não --bg-base cru), que acompanha o slider de
     transparência quando há papel de parede. */
  .nc-campo input, .nc-select {
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    border-radius: 8px; color: var(--text-primary); padding: 7px 9px;
    font: inherit; font-size: var(--text-sm); width: 100%;
  }
  .nc-modelos {
    border: 1px solid var(--border-subtle); border-radius: 10px;
    padding: var(--space-3); margin-bottom: var(--space-3);
    background: var(--surface-inset);
  }
  .nc-modelos-topo { display: flex; align-items: center; justify-content: space-between; gap: var(--space-2); margin-bottom: var(--space-2); }
  .nc-modelos-tit { font-size: 11.5px; color: var(--text-secondary); }
  .nc-modelos-vazio { color: var(--text-muted); font-size: var(--text-xs); margin: 0; }
  .nc-modelos-ok { color: var(--text-muted); font-size: 11px; margin: var(--space-1) 0 0; }
  .nc-erro { color: var(--error); font-size: var(--text-xs); margin: var(--space-1) 0 0; }
  .nc-sync-linha { margin: 3px 0 0; font-size: var(--text-xs); color: var(--text-secondary); }
  .nc-sync-linha b { color: var(--text-primary); font-weight: 600; margin-right: 6px; }
  /* "não instalado aqui" é esperado e fica apagado; falha de GRAVAÇÃO usa a cor de erro, igual ao
     resto do formulário. Com a mesma cor pros dois, uma recusa real ("já existe um provedor com
     esse nome fora do nosso bloco") ficava tão discreta quanto "o Codex não existe nesta máquina". */
  .nc-sync-linha.pulado { color: var(--text-muted); }
  .nc-codex-link { display: block; margin: 2px 0 var(--space-2) 18px; font-size: var(--text-xs);
                   color: var(--accent); word-break: break-all; }
  .nc-codex-codigo { margin: 2px 0 var(--space-2) 18px; font-family: var(--font-mono);
                     font-size: var(--text-lg); letter-spacing: 0.15em; color: var(--text-primary);
                     user-select: all; }
  .nc-sync-linha.falhou { color: var(--error); }
  .nc-rodape { display: flex; gap: var(--space-2); }
  .nc-btn {
    background: var(--surface-raised); border: 1px solid var(--border-subtle);
    color: var(--text-primary); border-radius: 8px; padding: 7px 13px;
    font: inherit; font-size: var(--text-sm); cursor: pointer;
  }
  .nc-btn.primario { background: var(--accent); border-color: var(--accent); color: #0b0e13; }
  .nc-btn:disabled { opacity: 0.5; cursor: default; }
</style>
