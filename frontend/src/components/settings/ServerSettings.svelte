<script lang="ts">
  import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
  import { criarSeletorNativo } from '../../lib/pastaNativa.svelte';
  import LinhaConfig from './LinhaConfig.svelte';
  import type { Server } from '../../lib/auth';
  import PushQuiet from '../PushQuiet.svelte';
  import { pushSupported } from '../../lib/push';
  import type { PushTarget } from '../../lib/quietHours';
  import * as m from '../../paraglide/messages';

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
    // Só a seção notificacoes lê: onde as horas silenciosas são gravadas. null = servidor ativo.
    apiTarget?: Server | null;
  }
  let { store, secao, apiTarget = null }: Props = $props();

  const pushTarget = $derived<PushTarget>(apiTarget ? { mode: 'server', server: apiTarget } : { mode: 'global' });

  const TITULOS: Record<Props['secao'], string> = {
    notificacoes: m.config_modal_notificacoes(),
    anexos: m.config_modal_anexos_curto(),
    avancado: m.config_modal_avancado(),
  };

  interface Campo {
    chave: string;
    rotulo: string;
    ajuda: string;
    tipo: 'texto' | 'segredo' | 'numero' | 'liga' | 'escolha';
    sufixo?: string;
    secao: Props['secao'];
    opcoes?: { value: string; label: string }[];
  }

  const CAMPOS: Campo[] = [
    { chave: 'upload_retention_days', rotulo: m.config_server_guardar_anexos(), tipo: 'numero', sufixo: m.config_server_dias(), secao: 'anexos',
      ajuda: m.config_server_guardar_ajuda() },
    { chave: 'automations', rotulo: m.config_server_automacoes(), tipo: 'liga', secao: 'avancado',
      ajuda: m.config_server_automacoes_ajuda() },
    // Não mora no runtime-config.json como os outros: escreve `showThinkingSummaries` no
    // settings.json do Claude Code (app/pensamento.py). Vale só pra sessão NOVA.
    { chave: 'mostrar_pensamento', rotulo: m.config_server_pensamento(), tipo: 'liga', secao: 'avancado',
      ajuda: m.config_server_pensamento_ajuda() },
    { chave: 'notify_finished', rotulo: m.config_server_avisar_terminar(), tipo: 'liga', secao: 'notificacoes',
      ajuda: m.config_server_avisar_terminar_ajuda() },
    { chave: 'finish_min_seconds', rotulo: m.config_server_turno_curto(), tipo: 'numero', sufixo: m.config_server_seg(), secao: 'notificacoes',
      ajuda: m.config_server_turno_curto_ajuda() },
    { chave: 'notify_dead', rotulo: m.config_server_avisar_cair(), tipo: 'liga', secao: 'notificacoes',
      ajuda: m.config_server_avisar_cair_ajuda() },
    { chave: 'stall_seconds', rotulo: m.config_server_marcar_travada(), tipo: 'numero', sufixo: m.config_server_seg(), secao: 'notificacoes',
      ajuda: m.config_server_marcar_travada_ajuda() },
    { chave: 'editor', rotulo: m.config_server_editor(), tipo: 'texto', secao: 'avancado',
      ajuda: m.config_server_editor_ajuda() },
  ];

  const visiveis = $derived(CAMPOS.filter((c) => c.secao === secao));

  // O que a tela Máquinas já mostra (identificador editável; porta, IP e URL na lista de endereços)
  // não se repete aqui — um dado, um lugar.
  const LEITURA_EM_MAQUINAS = new Set(['port', 'lan_bind_ip', 'server_id', 'public_url']);
  const ROTULO_LEITURA: Record<string, string> = {
    terminal_panel: m.config_server_painel_terminal(),
  };
  const leituraVisivel = $derived(Object.entries(store.leitura).filter(([k]) => !LEITURA_EM_MAQUINAS.has(k)));

  // Pastas mapeadas do seletor de pasta (scan_roots): o valor no runtime_config é a string "a,b"
  // (mesmo formato do CP_SCAN_ROOTS); a tela edita como lista de linhas.
  const raizes = $derived(String(store.valorAtual('scan_roots') ?? '')
    .split(',').map((s) => s.trim()).filter(Boolean));
  let novaRaiz = $state('');
  function raizesGravar(lista: string[]) {
    store.setRascunho('scan_roots', lista.join(','));
  }
  /** Devolve se ADICIONOU: caminho vazio ou repetido é no-op, e só quem adicionou limpa o campo
   *  (digitar uma raiz que já está na lista não pode apagar o que a pessoa escreveu). */
  function raizAdicionar(caminho: string): boolean {
    const p = caminho.trim();
    if (!p || raizes.includes(p)) return false;
    raizesGravar([...raizes, p]);
    return true;
  }
  function raizRemover(p: string) {
    raizesGravar(raizes.filter((r) => r !== p));
  }
  // Seletor NATIVO de pasta, o mesmo do modal de "Nova sessão" (lib/pastaNativa.svelte). Aqui ele
  // ADICIONA direto, sem passar pelo campo: quem acabou de apontar a pasta num diálogo do sistema
  // já respondeu qual é, e exigir um segundo clique em "Adicionar" seria perguntar de novo. Fora do
  // shell Electron o botão não existe e a tela fica exatamente como era — o campo de texto continua
  // sendo o caminho de quem usa pelo navegador e pelo celular.
  const nativo = criarSeletorNativo();

  // O rodapé só existe quando há o que salvar — e, quando existe, a tela reserva a altura dele:
  // grudado no pé, ele cobria o último campo de quem estava editando.
  const rodapeVisivel = $derived(
    !store.carregando && Object.keys(store.campos).length > 0
      && (store.temMudanca || store.salvando || store.salvo),
  );
</script>

<div class="cfg" class:com-rodape={rodapeVisivel && secao !== 'notificacoes'}>
  <header class="cfg-head">
    <h2>{TITULOS[secao]}</h2>
    <p class="sub">{m.config_server_valem()}</p>
  </header>

  {#if store.carregando}
    <p class="aviso">{m.comum_carregando()}</p>
  {:else if store.erro && !Object.keys(store.campos).length}
    <p class="aviso erro">{store.erro}</p>
    <button class="btn" onclick={() => void store.carregar()}>{m.config_server_tentar_de_novo()}</button>
  {:else}
    <div class="lista">
      {#each visiveis as c (c.chave)}
        <LinhaConfig campo={c} {store} />
      {/each}
    </div>

    {#if secao === 'avancado'}
      <div class="raizes">
        <h3>
          {m.config_server_raizes()}
          {#if store.campos['scan_roots']?.origem === 'app'}<span class="tag">{m.config_server_editado()}</span>{/if}
        </h3>
        <p class="ajuda">{m.config_server_raizes_ajuda()}</p>
        {#if raizes.length === 0}
          <p class="aviso">{m.config_server_raizes_vazio()}</p>
        {/if}
        {#each raizes as r (r)}
          <div class="raiz-linha">
            <span class="raiz-caminho">{r}</span>
            <button class="raiz-x" onclick={() => raizRemover(r)} aria-label={m.config_server_raiz_remover({ p: r })}>✕</button>
          </div>
        {/each}
        <form class="raiz-add" onsubmit={(e) => { e.preventDefault(); if (raizAdicionar(novaRaiz)) novaRaiz = ''; }}>
          <input
            type="text"
            autocomplete="off"
            autocapitalize="off"
            spellcheck={false}
            placeholder={m.config_server_raiz_placeholder()}
            aria-label={m.config_server_raiz_nova_aria()}
            bind:value={novaRaiz}
          />
          <button type="submit" class="btn" disabled={!novaRaiz.trim()}>{m.config_server_raiz_adicionar()}</button>
          {#if nativo.disponivel}
            <!-- Só no shell Electron (window.hangar): diálogo nativo de diretório do sistema.
                 `type="button"` porque ele vive DENTRO do form — sem isso o clique submeteria. -->
            <button type="button" class="btn" onclick={() => nativo.escolher((p) => raizAdicionar(p))} disabled={nativo.ocupado}>{m.criar_pasta_computador()}</button>
          {/if}
        </form>
        {#if nativo.erro}<p class="aviso erro" role="alert">{nativo.erro}</p>{/if}
      </div>

      {#if leituraVisivel.length}
        <div class="somente-leitura">
          <h3>{m.config_server_so_servidor()}</h3>
          <p class="ajuda">
            {m.config_server_so_servidor_1()} <code>.env</code>{m.config_server_so_servidor_2()}
          </p>
          {#each leituraVisivel as [k, v] (k)}
            <div class="ro-linha">
              <span class="ro-rot">{ROTULO_LEITURA[k] ?? k}</span>
              <span class="ro-val">{v === '' ? '—' : typeof v === 'boolean' ? (v ? m.config_server_sim() : m.config_server_nao()) : v}</span>
            </div>
          {/each}
        </div>
      {/if}
    {/if}
  {/if}

  {#if store.erro && Object.keys(store.campos).length}<p class="aviso erro">{store.erro}</p>{/if}
</div>

{#if secao === 'notificacoes'}
  <div class="cfg push" class:com-rodape={rodapeVisivel}>
    {#if pushSupported()}
      <p class="ajuda">{m.notif_push_legenda()}</p>
      <PushQuiet target={pushTarget} open={true} />
    {:else}
      <p class="aviso">{m.config_servidores_sem_push()}</p>
    {/if}
  </div>
{/if}

<!-- O rascunho e UM so pras tres fatias, entao Salvar grava tudo que foi mexido, inclusive fora
     desta tela. E o unico significado honesto: com rascunho compartilhado, um Salvar que gravasse so
     a propria fatia faria o MESMO botao significar coisas diferentes conforme a tela.
     So aparece quando ha o que salvar: um botao apagado permanente ocupava uma faixa inteira do
     painel pra nao oferecer nada. -->
{#if rodapeVisivel}
  <div class="rodape">
    {#if store.salvo}<span class="ok">{m.config_server_salvo()}</span>{/if}
    {#if store.temMudanca || store.salvando}
      <button class="btn primario" onclick={store.salvar} disabled={store.salvando}>
        {store.salvando ? m.config_motores_salvando() : m.ctx_salvar()}
      </button>
    {/if}
  </div>
{/if}

<style>
  .cfg { padding: var(--space-2) var(--space-4) var(--space-4); }
  /* O respiro de baixo é a altura do rodapé grudado: sem ele o último campo fica escondido atrás
     do botão Salvar, e a pessoa nem sabe que ele existe. */
  .cfg.com-rodape { padding-bottom: calc(84px + env(safe-area-inset-bottom)); }
  /* Bloco de push é um `.cfg` próprio (mesmo respiro lateral), sem repetir o padding-top que já
     veio do bloco de campos acima dele. */
  .push { padding-top: 0; }
  .cfg-head h2 { margin: 0; font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); }
  .cfg-head .sub { margin: 2px 0 var(--space-4); font-size: var(--text-xs); color: var(--text-muted); }

  .lista { display: flex; flex-direction: column; }
  /* "editado" = veio de override, não do .env — sem isso não dá pra saber de onde o valor vem.
     Usada nas raízes mapeadas (badge de scan_roots, abaixo), fora da lista de campos — por isso
     continua aqui mesmo com `LinhaConfig` tendo a própria cópia (CSS de Svelte é por componente). */
  .tag {
    font-size: 10px; font-weight: 700; letter-spacing: 0.04em; text-transform: uppercase;
    color: var(--accent); background: var(--accent-dim);
    padding: 1px 6px; border-radius: var(--radius-full);
  }
  /* min-width:0 e o que importa aqui, nao so cosmetica: `.ajuda` e um <span>, e um <span> dentro de
     um flex column tem `min-width:auto` por padrao — o navegador reserva a largura do texto INTEIRO
     sem quebrar, e a frase corta na borda do painel em vez de quebrar linha. Usada na ajuda das
     raízes mapeadas e do bloco somente-leitura, abaixo. */
  .ajuda { font-size: var(--text-xs); color: var(--text-muted); line-height: 1.45; min-width: 0; }

  /* Ainda usada pelo campo de texto de "Adicionar raiz" (:327), fora da lista de campos. O
     number saiu com o campo numérico movido pra LinhaConfig — não sobra nenhum aqui. */
  input[type='text'] {
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

  .raizes { margin-top: var(--space-5); }
  .raizes h3 {
    display: flex; align-items: center; gap: var(--space-2);
    margin: 0 0 4px; font-size: var(--text-sm); font-weight: 600; color: var(--text-secondary);
  }
  .raiz-linha {
    display: flex; align-items: center; justify-content: space-between; gap: var(--space-3);
    padding: var(--space-2) 0; border-bottom: 1px solid var(--border-subtle);
  }
  .raiz-caminho {
    font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-secondary);
    overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0;
  }
  .raiz-x {
    flex-shrink: 0; background: none; border: none; padding: 2px 6px;
    color: var(--text-muted); font-size: var(--text-sm);
  }
  .raiz-x:hover { color: var(--error); }
  /* Com o botão do seletor nativo a linha tem TRÊS itens, e ela vale nas duas views: no celular e
     no modal estreito o campo espremeria os dois botões até virar um traço. Quebra em vez de
     espremer — a base de 12rem é o ponto em que o campo ainda mostra um caminho. */
  .raiz-add { display: flex; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-3); }
  .raiz-add input { flex: 1 1 12rem; min-width: 0; }

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
  /* De ponta a ponta e na COR do painel: as margens negativas cancelam o respiro de quem envolve
     o rodapé, e esse respiro muda por modo (a folha no celular soma --space-5 + faixa segura
     embaixo, a coluna do modal dividido usa --space-4). Base calibrada pra folha; o override
     abaixo troca pra coluna — sem ele a faixa morria antes da borda, de outro tom, como um remendo. */
  .rodape {
    position: sticky; bottom: calc(env(safe-area-inset-bottom) * -1 - var(--space-5));
    display: flex; align-items: center; justify-content: flex-end; gap: var(--space-3);
    margin: 0 calc(-1 * var(--space-5)) calc(env(safe-area-inset-bottom) * -1 - var(--space-5));
    padding: var(--space-3) var(--space-4);
    padding-bottom: calc(var(--space-3) + var(--space-4) + env(safe-area-inset-bottom));
    background: rgb(var(--glass-panel-rgb));
    border-top: 1px solid var(--border-subtle);
  }
  :global(.st-conteudo) .rodape {
    bottom: calc(-1 * var(--space-4));
    margin: 0 calc(-1 * var(--space-4)) calc(-1 * var(--space-4));
  }
  .ok { font-size: var(--text-xs); color: var(--success); animation: st-row-in-ok 200ms var(--ease-out); }
  @keyframes st-row-in-ok {
    from { opacity: 0; transform: translateY(3px); }
    to   { opacity: 1; transform: translateY(0); }
  }
  .btn {
    height: 40px; padding: 0 var(--space-4);
    border-radius: var(--radius-md);
    background: var(--bg-elevated); color: var(--text-primary);
    font-size: var(--text-sm); font-weight: 600;
    transition: transform 160ms ease-out;
  }
  .btn:not(:disabled):active { transform: scale(0.97); }
  .btn.primario { background: var(--accent); color: #fff; }
  .btn:disabled { opacity: 0.45; }
</style>
