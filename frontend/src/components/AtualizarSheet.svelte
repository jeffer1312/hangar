<script lang="ts">
  /**
   * A caixa de Atualizar — quatro estados: em dia, versão nova, atualizando, deu erro.
   *
   * **Só desktop.** Celular é acesso remoto: quem atualiza é quem está na máquina onde o repo vive
   * (decisão do usuário, 25/08/2026). Por isso ela é montada pelo DesktopShell, e não pelo App.
   *
   * **Não lista os passos que a atualização executa.** O que precisa ser feito é problema do app; a
   * pessoa lê o que a versão traz e aperta um botão. Passo com texto próprio entra junto das
   * novidades, como texto — nunca como tarefa dela.
   *
   * **O progresso vem de um ARQUIVO, por polling.** O backend reinicia no meio da atualização,
   * então a conexão cai por definição: erro de rede aqui é ESPERADO e a caixa continua mostrando
   * "atualizando…". Ler a conexão faria a tela dizer "desconectado", que é a mesma frase que ela usa
   * quando o servidor caiu de verdade — e é justamente a confusão que este desenho evita.
   */
  import * as m from '../paraglide/messages';
  import BottomSheet from './BottomSheet.svelte';
  import { getAtualizacao, iniciarAtualizacao } from '../lib/api';
  import { renderMarkdown } from '../lib/markdown';
  import type { Atualizacao } from '../lib/types';

  interface Props {
    open: boolean;
    onClose: () => void;
    /** Sessões trabalhando agora: a caixa avisa antes de reiniciar, mas não impede. */
    trabalhando?: number;
  }
  let { open, onClose, trabalhando = 0 }: Props = $props();

  let dados = $state<Atualizacao | null>(null);
  let carregando = $state(false);
  let erroDeRede = $state('');
  let enviando = $state(false);
  /** Quantas novidades a lista mostra antes de resumir o resto numa linha. */
  const TETO_MUDANCAS = 5;
  let verTudo = $state(false);

  const INTERVALO = 2000;
  let timer: ReturnType<typeof setTimeout> | null = null;
  /** Esta tela mandou começar. Verdade a partir do POST, não do próximo fetch que der certo. */
  let lancamos = $state(false);

  const estado = $derived(dados?.estado ?? {});
  const rodando = $derived(estado.fase === 'rodando');
  const falhou = $derived(estado.fase === 'pronto' && estado.ok === false);
  const temNovidade = $derived(!!dados?.atualizacao_disponivel);
  const mudancas = $derived(dados?.mudancas ?? []);
  const visiveis = $derived(verTudo ? mudancas : mudancas.slice(0, TETO_MUDANCAS));
  const escondidas = $derived(Math.max(0, mudancas.length - visiveis.length));
  /** As duas versões só aparecem quando DIVERGEM — quem só usa não precisa de diagnóstico. */
  const versoesDivergem = $derived(!!dados && dados.versoes.repo !== dados.versoes.backend);

  async function carregar(silencioso = false) {
    if (!silencioso) carregando = true;
    try {
      dados = await getAtualizacao();
      erroDeRede = '';
    } catch (e) {
      // Durante o restart isto acontece por DESENHO. Só vira mensagem quando não há atualização em
      // curso — aí sim é o servidor fora do ar, e a pessoa precisa saber.
      //
      // `lancamos` e não só `rodando`: `rodando` sai do ÚLTIMO fetch que deu certo, então se o
      // servidor cair antes de responder um `fase: rodando` sequer, ele ainda diz "ocioso" e a
      // falha vira "desconectado" na tela — a frase que este desenho existe pra evitar, na hora
      // exata em que ela é mais confusa.
      if (!rodando && !lancamos) erroDeRede = e instanceof Error ? e.message : String(e);
    } finally {
      carregando = false;
    }
  }

  function agendar() {
    if (timer) clearTimeout(timer);
    timer = setTimeout(async () => {
      await carregar(true);
      if (open && rodando) agendar();
      else {
        lancamos = false;   // acabou: erro de rede daqui pra frente é servidor fora do ar mesmo
        if (open && estado.fase === 'pronto' && estado.ok === true && !faltaReiniciar) concluir();
      }
    }, INTERVALO);
  }

  /** Terminou bem, mas o servidor não reiniciou sozinho (Windows, instalação na mão). */
  const faltaReiniciar = $derived(estado.fase === 'pronto' && estado.ok === true
                                  && !!estado.reiniciar_manual);

  /** Terminou bem: recarrega a página para sair do bundle antigo (o do build anterior, em cache). */
  async function concluir() {
    try {
      const reg = await navigator.serviceWorker?.getRegistration();
      await reg?.update();
      reg?.waiting?.postMessage({ type: 'SKIP_WAITING' });
    } catch {
      // Sem service worker (aba comum, navegador antigo) o reload sozinho já basta.
    }
    window.location.reload();
  }

  async function atualizar() {
    enviando = true;
    erroDeRede = '';
    try {
      await iniciarAtualizacao();
      lancamos = true;
      await carregar(true);
      agendar();
    } catch (e) {
      erroDeRede = e instanceof Error ? e.message : String(e);
    } finally {
      enviando = false;
    }
  }

  $effect(() => {
    if (open) carregar();
    // Limpeza DEVOLVIDA, não feita no ramo `!open`: o `return` de lá só sai da função e não
    // registra teardown nenhum, então o timer sobrevivia à DESTRUIÇÃO do componente. O
    // `DesktopShell` some da árvore ao navegar pra Custos/Arquivo, e o polling continuava rodando
    // solto — ao terminar a atualização ele chamava `location.reload()` na cara de quem já estava
    // noutra tela.
    return () => {
      if (timer) clearTimeout(timer);
      timer = null;
    };
  });

  // Reabrir a caixa com uma atualização já em curso (outra aba a começou, ou a página recarregou no
  // meio) tem que voltar a acompanhar — senão a barra fica parada num estado antigo.
  $effect(() => {
    if (open && rodando && !timer) agendar();
  });

  const passosComTexto = $derived((dados?.passos ?? []).filter((p) => p.texto.trim()));

  // Duas frases, não uma com `{n}`: com uma só a tela escrevia "1 sessões estão trabalhando".
  const aviso_sessoes = $derived(
    trabalhando === 1
      ? m.atualizar_sessao_trabalhando()
      : m.atualizar_sessoes_trabalhando({ n: trabalhando }),
  );
  const desfecho = $derived(
    !estado.voltou
      ? m.atualizar_falhou_parou()
      : estado.no_ar === false
        ? m.atualizar_falhou_voltou_fora()
        : m.atualizar_falhou_voltou(),
  );
  const resumo_mudancas = $derived(
    mudancas.length === 1
      ? m.atualizar_disponivel_sub_uma()
      : m.atualizar_disponivel_sub({ n: mudancas.length }),
  );
</script>

<BottomSheet {open} {onClose} wide centered ariaLabel={m.atualizar_titulo()} persistent={rodando}>
  <div class="cx">
    {#if carregando && !dados}
      <p class="msg">{m.atualizar_carregando()}</p>

    {:else if rodando}
      <h2 class="titulo">{m.atualizar_rodando_titulo()}</h2>
      <p class="sub">
        {m.atualizar_rodando_sub({ passo: estado.passo ?? 0, total: estado.total ?? 0 })}
      </p>
      <div class="barra">
        <div class="fill" style:width="{((estado.passo ?? 0) / (estado.total || 1)) * 100}%"></div>
      </div>
      <p class="etapa">{estado.texto ?? ''}</p>
      {#if trabalhando > 0}
        <p class="aviso">{aviso_sessoes}</p>
      {/if}

    {:else if faltaReiniciar}
      <!-- Não recarrega a página: o servidor ainda serve o código antigo, e um reload aqui só
           traria de volta a mesma versão, dando a impressão de que nada aconteceu. -->
      <h2 class="titulo">{m.atualizar_em_dia_titulo()}</h2>
      <p class="sub">{m.atualizar_pronto_reiniciar()}</p>
      <div class="acoes">
        <button class="bt primario" onclick={onClose}>{m.atualizar_fechar()}</button>
      </div>

    {:else if falhou}
      <h2 class="titulo">{m.atualizar_falhou_titulo()}</h2>
      <div class="erro-caixa">
        <!-- Erro do servidor é DADO, não chave de idioma: vai como veio. -->
        <p class="erro-log">{estado.erro}</p>
      </div>
      <!-- Três desfechos, não dois: "revertido e no ar", "revertido mas o servidor não subiu" e
           "parou antes de mexer em nada". Com duas frases só, o caso do meio era descrito por uma
           das outras duas — e as duas mentiam. -->
      <p class="estado-final">{desfecho}</p>
      {#if estado.resgate}
        <p class="resgate">{m.atualizar_resgate({ branch: estado.resgate })}</p>
      {/if}
      <div class="acoes">
        <button class="bt secundario" onclick={onClose}>{m.atualizar_fechar()}</button>
        <button class="bt primario" onclick={atualizar} disabled={enviando}>
          {m.atualizar_tentar_de_novo()}
        </button>
      </div>

    {:else if temNovidade}
      <h2 class="titulo">{m.atualizar_disponivel_titulo()}</h2>
      <p class="sub">{resumo_mudancas}</p>

      <p class="rotulo">{m.atualizar_o_que_vem()}</p>
      <div class="novidades">
        <ul>
          {#each visiveis as mud (mud.sha)}
            <li>{mud.titulo}</li>
          {/each}
        </ul>
        {#if escondidas > 0}
          <button class="mais" onclick={() => (verTudo = true)}>
            {m.atualizar_e_mais({ n: escondidas })}
          </button>
        {/if}
        {#each passosComTexto as passo (passo.id)}
          <div class="passo-texto">{@html renderMarkdown(passo.texto)}</div>
        {/each}
      </div>

      {#if trabalhando > 0}
        <p class="aviso">{aviso_sessoes}</p>
      {/if}
      {#if erroDeRede}<p class="erro-linha">{erroDeRede}</p>{/if}
      <div class="acoes">
        <button class="bt secundario" onclick={onClose}>{m.atualizar_agora_nao()}</button>
        <button class="bt primario" onclick={atualizar} disabled={enviando}>
          {m.atualizar_botao()}
        </button>
      </div>

    {:else}
      <h2 class="titulo">{m.atualizar_em_dia_titulo()}</h2>
      <p class="sub">{m.atualizar_em_dia_sub()}</p>
      <div class="linha">
        <span class="rot">{m.atualizar_versao()}</span>
        <span class="val">{dados?.versoes.repo ?? '—'}</span>
      </div>
      {#if versoesDivergem}
        <!-- Divergiu: o disco tem uma versão e o processo tem outra. Acontece entre um `git pull`
             feito na mão e o restart, e é a única hora em que este detalhe importa pra alguém. -->
        <div class="linha">
          <span class="rot">{m.atualizar_versao_servidor()}</span>
          <span class="val">{dados?.versoes.backend ?? '—'}</span>
        </div>
        <p class="aviso">{m.atualizar_precisa_reiniciar()}</p>
      {/if}
      {#if erroDeRede}<p class="erro-linha">{erroDeRede}</p>{/if}
      <div class="acoes">
        <button class="bt secundario" onclick={() => carregar()} disabled={carregando}>
          {m.atualizar_procurar()}
        </button>
      </div>
    {/if}
  </div>
</BottomSheet>

<style>
  /* Sem superfície própria: quem carrega o material é o BottomSheet, que já anda com o slider de
     Transparência. Uma cor de fundo aqui viraria retângulo chapado sobre o papel de parede. */
  .cx { padding: var(--space-5); }

  .titulo { margin: 0 0 var(--space-1); font-size: var(--text-lg); font-weight: 600;
            color: var(--text-primary); }
  .sub { margin: 0 0 var(--space-4); font-size: var(--text-sm); color: var(--text-secondary);
         line-height: 1.5; }
  .msg { margin: 0; font-size: var(--text-sm); color: var(--text-secondary); }
  .rotulo { margin: 0 0 var(--space-2); color: var(--text-muted); font-size: var(--text-xs);
            text-transform: uppercase; letter-spacing: 0.05em; }

  .novidades { background: var(--surface-inset); border: 1px solid var(--border-subtle);
               border-radius: var(--radius-md); padding: var(--space-3);
               max-height: 260px; overflow-y: auto; margin-bottom: var(--space-4); }
  .novidades ul { margin: 0; padding-left: 1.1em; }
  .novidades li { font-size: var(--text-sm); color: var(--text-primary); line-height: 1.6; }
  .mais { margin-top: var(--space-2); border: 0; background: transparent; color: var(--accent);
          font-family: inherit; font-size: var(--text-sm); padding: 0; }
  .passo-texto { margin-top: var(--space-3); padding-top: var(--space-3);
                 border-top: 1px solid var(--border-subtle);
                 font-size: var(--text-sm); color: var(--text-secondary); line-height: 1.55; }

  .barra { height: 6px; border-radius: var(--radius-full); background: var(--surface-inset);
           overflow: hidden; margin-bottom: var(--space-3); }
  .fill { height: 100%; background: var(--accent); border-radius: var(--radius-full);
          transition: width 0.4s var(--ease-out); }
  .etapa { margin: 0 0 var(--space-4); font-size: var(--text-sm); font-weight: 600;
           color: var(--text-primary); }

  .aviso { margin: 0 0 var(--space-4); font-size: var(--text-xs); color: var(--text-muted);
           line-height: 1.5; }

  .erro-caixa { background: var(--surface-inset); border: 1px solid var(--border-subtle);
                border-left: 3px solid var(--erro, #d97070); border-radius: var(--radius-md);
                padding: var(--space-3); margin-bottom: var(--space-3); }
  .erro-log { margin: 0; font-family: var(--font-mono, monospace); font-size: var(--text-xs);
              color: var(--text-muted); line-height: 1.5; white-space: pre-wrap;
              word-break: break-word; }
  .erro-linha { margin: 0 0 var(--space-3); font-size: var(--text-sm); color: var(--erro, #d97070); }
  .estado-final { margin: 0 0 var(--space-2); font-size: var(--text-sm);
                  color: var(--text-secondary); line-height: 1.55; }
  .resgate { margin: 0 0 var(--space-4); font-size: var(--text-xs); color: var(--text-muted);
             line-height: 1.5; }

  .linha { display: flex; align-items: baseline; justify-content: space-between;
           gap: var(--space-3); padding: var(--space-2) 0;
           border-bottom: 1px solid var(--border-subtle); }
  .rot { font-size: var(--text-sm); color: var(--text-secondary); }
  .val { font-family: var(--font-mono, monospace); font-size: var(--text-xs);
         color: var(--text-primary); }

  .acoes { display: flex; align-items: center; justify-content: flex-end; gap: var(--space-2);
           margin-top: var(--space-4); }
  .bt { border-radius: var(--radius-md); padding: var(--space-2) var(--space-4);
        font-family: inherit; font-size: var(--text-sm); font-weight: 600; border: 0; }
  .primario { background: var(--accent); color: var(--text-inverse); }
  .secundario { background: var(--surface-raised); color: var(--text-secondary);
                border: 1px solid var(--border-subtle); font-weight: 500; }
  .bt:disabled { opacity: 0.5; }
</style>
