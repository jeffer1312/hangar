<script lang="ts">
  import * as m from '../../paraglide/messages';
  import { mensagemDeErro } from '../../lib/errosApi';
  import type { PathDiff, FileContent } from '../../lib/types';
  import { highlightDiff, highlightCodeLines, type DiffRow, type DiffToken } from '../../lib/highlight';
  import { dec } from '../../lib/fmt';
  import DiffView from '../git/DiffView.svelte';
  import CodeEditor from './CodeEditor.svelte';

  interface Props {
    path: string;
    diff: PathDiff | null;
    conteudo: FileContent | null;
    loading: boolean;
    onEscopo: (e: 'branch' | 'nao_commitado') => void;
    onFechar: () => void;
    // Rotulo do link de saida (B4, Task 12): no desktop o arquivo cobre a conversa e o default
    // diz exatamente isso; no modal Git do celular nao ha conversa atras — o hospedeiro passa
    // um rotulo que descreve a tela real (m.comum_voltar).
    rotuloVoltar?: string;
    // Erro da abertura (B3, Task 12): quando a leitura falha (binario, 404), o store grava o
    // aviso aqui e o visor o mostra com role="alert" em vez de desmontar. Opcional de
    // proposito: o host desktop do Chat nao passa (o fluxo de erro dele e outro).
    erro?: string | null;
    // Gravar é do hospedeiro (quem tem o store). Sem esta prop o visor é só leitura — é o que
    // acontece na aba de commit, onde o arquivo mostrado é o de um commit passado, não o do disco.
    onSalvar?: ((texto: string) => Promise<string | null>) | null;
  }
  let { path, diff, conteudo, loading, onEscopo, onFechar, rotuloVoltar = m.arq_voltar_conversa(), erro = null, onSalvar = null }: Props = $props();

  // Linhas do diff já destacadas. highlightDiff é assíncrona (import dinâmico do Shiki).
  // A flag `valida` do $effect descarta resposta velha — escopo trocado, diff novo ou o
  // componente desmontado no meio da busca; sem ela a resposta anterior sobrescreve a nova.
  let rows: DiffRow[] = $state([]);

  // Qual TEXTO de diff já está em `rows`. Derivar "está destacando" disto (em vez de um $state
  // ligado dentro do $effect) tira o quadro inicial em que rows=[] e destacando=false — nele o
  // DiffView desenhava "sem diferenças" para um arquivo que tem diff.
  let destacadoDe: string | null = $state(null);

  // Payload de OUTRO arquivo nao desenha: entre `abrir(b)` e a resposta, o store ainda tem o
  // conteudo de `a` (ele so troca quando a resposta chega), e a tela mostrava o arquivo errado
  // sob o nome certo. `path` no FileContent/PathDiff existe exatamente pra isso.
  const doArquivo = $derived(conteudo !== null && conteudo.path === path ? conteudo : null);
  const diffDoArquivo = $derived(diff !== null && diff.path === path ? diff : null);

  // Diff VAZIO nao e diff: o path_diff responde "" para arquivo sem alteracao no escopo, e nesse
  // caso quem o usuario quer ver e o arquivo.
  const temDiff = $derived(diffDoArquivo !== null && diffDoArquivo.diff.trim() !== '');

  const destacando = $derived(temDiff && diffDoArquivo !== null && destacadoDe !== diffDoArquivo.diff);

  // O que está NA TELA é o escopo_usado. Quando ele diverge do pedido, o outro escopo é
  // impossível (a base não existe) — botão desabilitado, mas com o rótulo do que está sendo
  // mostrado e o motivo em TEXTO (title de botão desabilitado não é lido por ninguém).
  const caiu = $derived(diffDoArquivo !== null && diffDoArquivo.escopo_usado !== diffDoArquivo.escopo_pedido);

  // Motivo do escopo caido chega do backend como CODIGO (chave `arq_*`) e vira texto pela MESMA
  // via dos erros (mensagemDeErro). Codigo desconhecido devolve undefined e a tela nao desenha
  // nada — nem quebra nem mostra o codigo cru. Sem isto o motivo vinha em portugues no app em ingles.
  const motivoVisivel = $derived(
    caiu && diffDoArquivo !== null && diffDoArquivo.motivo !== null
      ? (mensagemDeErro(diffDoArquivo.motivo) ?? null)
      : null,
  );

  $effect(() => {
    const d = diffDoArquivo;
    let valida = true;
    if (d === null || d.diff.trim() === '') {
      rows = [];
      destacadoDe = null;
      return () => { valida = false; };
    }
    rows = [];
    highlightDiff(d.diff, path).then((r) => {
      if (!valida) return;
      rows = r;
      destacadoDe = d.diff;
    }).catch(() => {
      if (!valida) return;
      rows = [];
      destacadoDe = d.diff;   // desiste desta versão: não fica preso em "carregando"
    });
    return () => { valida = false; };
  });

  // ── Arquivo sem diff: mesmas duas peças do diff, pro conteúdo ────────────────
  // Tokens por linha do arquivo ([] = mostra plano, que é também o estado "ainda destacando").
  let tokensArquivo: DiffToken[][] = $state([]);
  // Qual TEXTO já está em `tokensArquivo` — o arquivo pode trocar sem o componente remontar.
  let destacadoDeTexto: string | null = $state(null);

  const linhas = $derived(doArquivo !== null ? linhasDe(doArquivo.text) : []);
  // Largura da calha em dígitos (mínimo 2 pra não pular de largura em arquivo de <10 linhas).
  const digitosCalha = $derived(Math.max(2, String(linhas.length).length));

  const destacandoArquivo = $derived(!temDiff && doArquivo !== null && destacadoDeTexto !== doArquivo.text);

  $effect(() => {
    const f = doArquivo;
    let valida = true;
    if (f === null || temDiff) {
      tokensArquivo = [];
      destacadoDeTexto = null;
      return () => { valida = false; };
    }
    const texto = f.text;
    tokensArquivo = [];
    highlightCodeLines(linhasDe(texto), path).then((t) => {
      if (!valida) return;
      // null = sem destaque possível (.txt, grammar que falhou, arquivo acima do teto interno):
      // texto plano COM a calha, nunca erro na tela.
      tokensArquivo = t ?? [];
      destacadoDeTexto = texto;
    }).catch(() => {
      if (!valida) return;
      tokensArquivo = [];
      destacadoDeTexto = texto;   // desiste desta versão: não fica preso em "carregando"
    });
    return () => { valida = false; };
  });

  // +N −M contado das linhas destacadas (a mesma conta do cabeçalho interno do DiffView).
  const estat = $derived({
    add: rows.filter((r) => r.kind === 'add').length,
    del: rows.filter((r) => r.kind === 'del').length,
  });

  // Caminho quebrado em pasta + nome: a pasta sai em --text-muted, o nome no tom normal.
  // $derived de propósito: trocar de arquivo sem remontar o componente tem que trocar o cabeçalho.
  const ultimaBarra = $derived(path.lastIndexOf('/'));
  const nomeArquivo = $derived(ultimaBarra === -1 ? path : path.slice(ultimaBarra + 1));
  const dirParte = $derived(ultimaBarra === -1 ? '' : path.slice(0, ultimaBarra + 1));

  // "desde 721d1a0" chega inteiro da tradução; a barra mostra o "desde" com peso 500.
  // Split na primeira palavra cobre pt ("desde") e en ("since").
  const partesDesde = $derived(
    diffDoArquivo !== null && diffDoArquivo.base !== null && diffDoArquivo.base.trim() !== ''
      ? m.arq_escopo_desde({ base: diffDoArquivo.base }).split(/ (.+)/)
      : null,
  );

  // Tamanho binário na vírgula do idioma do app (dec usa intlLocale): "12,4 KB" casa com a
  // barra; abaixo de 1 KB mostra os bytes crus.
  function tamLegivel(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${dec(n / 1024, 1)} KB`;
    return `${dec(n / (1024 * 1024), 1)} MB`;
  }

  // Linhas do arquivo, sem a linha fantasma do \n final. Serve pra contagem da meta E pro visor
  // (calha + destaque), que precisam concordar linha a linha.
  function linhasDe(texto: string): string[] {
    return texto === '' ? [] : texto.replace(/\n$/, '').split('\n');
  }

  function linhasDoTexto(texto: string): number {
    return linhasDe(texto).length;
  }

  // Plural correto da meta: arq_meta_arquivo diz "N linhas" e o Paraglide deste projeto nao tem
  // plural ICU — a chave arq_meta_arquivo_um cobre o caso de 1 linha ("1 linhas" nao existe).
  const metaArquivo = $derived(
    doArquivo
      ? linhasDoTexto(doArquivo.text) === 1
        ? m.arq_meta_arquivo_um({ tam: tamLegivel(doArquivo.size) })
        : m.arq_meta_arquivo({ tam: tamLegivel(doArquivo.size), linhas: linhasDoTexto(doArquivo.text) })
      : null,
  );
  // ── edição ────────────────────────────────────────────────────────────────────────────────
  let editando = $state(false);
  let rascunho = $state('');
  let salvando = $state(false);
  let erroSalvar = $state<string | null>(null);
  let salvoAgora = $state(false);
  const sujo = $derived(editando && doArquivo !== null && rascunho !== doArquivo.text);
  // Sem digest não há gravação (leitura truncada): mostrar o botão seria oferecer algo que o
  // backend recusa de propósito.
  // Não depende de estar mostrando o diff: o arquivo que a pessoa quer editar é justamente o que
  // ela acabou de ver mudar. Entrando em edição, o editor toma o lugar do diff.
  const podeEditar = $derived(onSalvar !== null && doArquivo !== null && doArquivo.digest !== null);

  // A base do diff embutido. `undefined` (campo ausente, backend antigo) vira null: sem base o
  // editor mostra só o arquivo, que é o certo — nunca um diff inventado.
  const baseDoDiff = $derived(diffDoArquivo?.original ?? null);
  // O editor só assume a leitura quando temos o conteúdo do disco. Arquivo truncado continua
  // aparecendo (sem diff embutido): cortar a tela seria pior que mostrar o começo.
  const podeUsarEditor = $derived(doArquivo !== null);

  // Trocar de arquivo com edição aberta larga o rascunho: manter o texto de `a` sobre o nome de
  // `b` seria o mesmo defeito que o `doArquivo` existe pra impedir, agora com risco de gravar.
  $effect(() => {
    void path;
    editando = false;
    erroSalvar = null;
    salvoAgora = false;
  });

  async function salvar() {
    if (!onSalvar || !sujo || salvando) return;
    salvando = true;
    erroSalvar = null;
    const falha = await onSalvar(rascunho);
    salvando = false;
    if (falha) {
      // Ja vem traduzida do api.ts; o mensagemDeErro cobre o caso de vir um codigo cru.
      erroSalvar = mensagemDeErro(falha) ?? falha;
      return;
    }
    salvoAgora = true;
    editando = false;
    setTimeout(() => { salvoAgora = false; }, 2000);
  }

  function abrirEdicao() {
    if (!doArquivo) return;
    rascunho = doArquivo.text;
    erroSalvar = null;
    editando = true;
  }

  function descartar() {
    editando = false;
    erroSalvar = null;
  }

</script>

<div class="visor" role="region" tabindex="-1" aria-label={path} aria-busy={loading || destacando || destacandoArquivo}>
  <div class="cab">
    <div class="cab-l1">
      <span class="caminho">
        {#if dirParte}<span class="dir">{dirParte}</span>{/if}{nomeArquivo}
      </span>
      {#if diffDoArquivo && !loading && !destacando && (estat.add || estat.del)}
        <span class="stat"><span class="stat-add">+{estat.add}</span> <span class="stat-del">−{estat.del}</span></span>
      {/if}
      <button class="fechar" aria-label={m.arq_fechar()} onclick={onFechar}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M18 6 6 18M6 6l12 12"/></svg>
      </button>
    </div>
    <div class="cab-l2">
      {#if diffDoArquivo}
        {@const d = diffDoArquivo}
        <button
          class="escopo"
          disabled={caiu}
          onclick={() => onEscopo(d.escopo_usado === 'branch' ? 'nao_commitado' : 'branch')}
        >
          {d.escopo_usado === 'branch' ? m.arq_escopo_branch() : m.arq_escopo_nao_commitado()}
          <span class="seta">▾</span>
        </button>
        {#if motivoVisivel}<span class="motivo">{motivoVisivel}</span>{/if}
      {/if}
      <span class="meta">
        {#if partesDesde}
          <b>{partesDesde[0]}</b> {partesDesde[1]}
        {/if}
        {#if partesDesde && doArquivo}<span class="sep"> · </span>{/if}
        {#if doArquivo}{metaArquivo}{/if}
      </span>
      {#if podeEditar}
        {#if editando}
          <button class="acao primaria" disabled={!sujo || salvando} onclick={salvar}>
            {salvando ? m.arq_salvando() : m.arq_salvar()}
          </button>
          <button class="acao" onclick={descartar}>{m.arq_descartar()}</button>
          {#if sujo}<span class="sujo">{m.arq_nao_salvo()}</span>{/if}
        {:else}
          <button class="acao" onclick={abrirEdicao}>{m.arq_editar()}</button>
        {/if}
      {/if}
      {#if salvoAgora}<span class="salvo">✓ {m.arq_salvo()}</span>{/if}
      <button class="voltar" onclick={onFechar}>← {rotuloVoltar}</button>
    </div>
  </div>

  <div class="corpo">
    {#if erro}
      <!-- O erro da abertura fica NA TELA e e anunciado (B3 r2): sem isto o visor desmontava e a
           arvore voltava "normal" depois de uma leitura que falhou. role="alert" anuncia a
           chegada ao leitor de tela. O erro e EXCLUSIVO (B1 r4): com ele, nada da cadeia abaixo
           roda — senao o usuario veria "sem diferenças" junto de "arquivo binario", duas
           afirmacoes que se contradizem. -->
      <p class="aviso erro" role="alert">{erro}</p>
    {:else if editando && doArquivo}
      {#if erroSalvar}
        <p class="aviso erro" role="alert">{erroSalvar}</p>
      {/if}
      <!-- Editando: o arquivo puro. Ver o diff e digitar ao mesmo tempo brigaria — as faixas de
           texto removido são widgets, e o cursor andaria por cima delas. -->
      <CodeEditor texto={rascunho} path={path} editavel={true} original={null}
                  onChange={(t) => (rascunho = t)} onSalvar={salvar} />
    {:else if podeUsarEditor && doArquivo}
      {#if doArquivo.truncated}
        <p class="aviso">{m.arq_arquivo_cortado()}</p>
      {/if}
      <!-- Leitura: o MESMO editor, com o diff por dentro (unifiedMergeView). É o que troca o
           `diff --git`/`@@` cru por arquivo inteiro, numeração real e trechos iguais dobrados. -->
      <CodeEditor texto={doArquivo.text} path={path} editavel={false} original={baseDoDiff} />
    {:else if temDiff && diffDoArquivo}
      {@const d = diffDoArquivo}
      {#if d.truncated}
        <p class="aviso">{m.arq_diff_cortado()}</p>
      {/if}
      <DiffView path={path} rows={rows} loading={loading || destacando} />
    {:else if doArquivo}
      {#if doArquivo.truncated}
        <p class="aviso">{m.arq_arquivo_cortado()}</p>
      {/if}
      {#if erroSalvar}
        <p class="aviso erro" role="alert">{erroSalvar}</p>
      {/if}
      <!-- Calha + linhas destacadas (mesmo desenho do DiffView: cor do token INLINE, vinda do
           tema do Shiki). Sem tokens ainda (destaque em voo) ou sem tokens possíveis (null do
           highlight) a linha sai em texto plano — a calha aparece nos dois casos. -->
      <pre class="conteudo" style:--gut="{digitosCalha}ch">{#each linhas as linha, i (i)}{@const toks = tokensArquivo[i]}<span class="ln"><span class="gut">{i + 1}</span>{#if toks}{#each toks as t, j (j)}<span style={t.color ? `color: ${t.color}` : undefined}>{t.content}</span>{/each}{:else}{linha}{/if}</span>{/each}</pre>
    {:else if loading}
      <!-- Busca em voo sem nada ainda: aviso de carga, nunca a afirmação "sem diferenças". -->
      <p class="aviso">{m.git_diff_carregando()}</p>
    {:else}
      <p class="aviso">{m.git_sem_diferencas()}</p>
    {/if}
  </div>
</div>

<style>
  /* OPACO de propósito, sem seguir o slider de transparência: ler ou editar código com o chat
     de trás atravessando o texto é ilegível, e foi o que o usuário pediu pra acabar (20/08/2026).
     Cor crua e sólida, não `--surface-inset` — este é o único lugar do app que sai do véu. */
  .visor { flex: 1; min-width: 0; display: flex; flex-direction: column; overflow: hidden; background: var(--bg-base); }
  .acao { padding: 3px 10px; border-radius: var(--radius-sm); border: 1px solid var(--border-subtle); background: var(--surface-raised); color: var(--text-secondary); font-size: var(--text-xs); cursor: pointer; }
  .acao:hover:not(:disabled) { color: var(--text-primary); }
  .acao:disabled { opacity: 0.5; cursor: default; }
  .acao.primaria { border-color: var(--accent); color: var(--accent); }
  .sujo { font-size: var(--text-xs); color: var(--warning); }
  .salvo { font-size: var(--text-xs); color: var(--success); }
  .cab { padding: 11px 16px 10px; border-bottom: 1px solid var(--border-subtle); }
  .cab-l1 { display: flex; align-items: center; gap: 10px; }
  .caminho {
    font-family: var(--font-mono); font-size: 13px;
    flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap;
  }
  .caminho .dir { color: var(--text-muted); }
  .stat { font-family: var(--font-mono); font-size: 12px; margin-left: auto; flex: none; }
  .stat .stat-add { color: var(--success); }
  .stat .stat-del { color: var(--error); }
  .fechar {
    border: 0; background: none; color: var(--text-muted); cursor: pointer;
    width: 26px; height: 26px; border-radius: 6px; display: grid; place-items: center;
    flex: none;
  }
  .fechar:hover { background: var(--bg-hover); color: var(--text-primary); }
  .fechar svg { width: 15px; height: 15px; }

  .cab-l2 { display: flex; align-items: center; gap: 10px; margin-top: 9px; }
  .escopo {
    display: inline-flex; align-items: center; gap: 5px;
    background: var(--surface-raised); border: 1px solid var(--border-subtle);
    border-radius: 8px; padding: 5px 9px; font-size: 12px; color: var(--text-primary);
    cursor: pointer; font-family: inherit; flex: none;
  }
  .escopo:disabled { cursor: default; opacity: 0.55; }
  .escopo .seta { font-size: 8px; }
  .meta { font-size: 12px; color: var(--text-muted); min-width: 0; }
  .meta b { font-weight: 500; color: var(--text-secondary); }
  /* Motivo do escopo caído: TEXTO ao lado do botão (title em botão desabilitado não é lido).
     --warning é o token que o DiffView já usa para o aviso de corte — mesma família visual. */
  .motivo { font-size: 12px; color: var(--warning); min-width: 0; }
  .voltar {
    margin-left: auto; display: inline-flex; align-items: center; gap: 5px;
    background: none; border: 0; color: var(--accent); font: inherit; font-size: 12px;
    cursor: pointer; flex: none; padding: 0;
  }

  .corpo {
    padding: 14px 16px 18px;
    /* Task 14: o corpo NAO rola — quem rola e a caixa do diff/conteudo (height: fit-content +
       max-height: 100% no .git-diff e no .conteudo). Flex column para a caixa ocupar a altura
       disponivel depois do cabecalho fixo. */
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    overflow: hidden;
    flex: 1;
    min-height: 0;
  }

  /* O cabeçalho de cima (caminho, +N −M, fechar) é DESTE componente. O interno do DiffView
     (que serve o GitChangesTab intacto) fica escondido aqui, por CSS no escopo do FileViewer —
     sem tocar no DiffView, que é arquivo compartilhado deste lote. */
  .corpo :global(.git-diff-head) {
    display: none;
  }

  .aviso {
    margin: 0 0 var(--space-2); padding: 7px 9px; border-radius: 7px;
    background: var(--fill-subtle); color: var(--text-muted);
    font-size: 11.5px; line-height: 1.4;
  }
  /* Erro da abertura (B3): mesma cor de erro do resto do app (FilesPanel usa .aviso.erro). */
  .aviso.erro { color: var(--error); }

  .conteudo {
    margin: 0; padding: var(--space-2); border-radius: var(--radius-md);
    background: var(--surface-inset); border: 1px solid var(--border-subtle);
    font-family: var(--font-mono); font-size: var(--text-xs); line-height: 1.5;
    /* Task 14: mesma regra da caixa do diff — altura disponivel para arquivo grande, tamanho
       do conteudo para arquivo curto, rolagem dentro da caixa. */
    height: fit-content;
    max-height: 100%;
    overflow: auto;
    white-space: pre;
    flex-shrink: 1;
    min-height: 0;
  }
  /* A linha inteira ocupa a largura do CONTEUDO (min 100% da caixa) — sem isso a calha sticky
     para de grudar no meio da rolagem, porque ela nao passa do bloco que a contem. */
  .conteudo .ln { display: block; width: max-content; min-width: 100%; }
  /* Calha: numero fixo na esquerda enquanto o codigo rola em X (sticky). O fundo e o MESMO
     material da caixa (--surface-inset), pro codigo passar por baixo sem aparecer. */
  .conteudo .gut {
    position: sticky; left: 0;
    display: inline-block; width: var(--gut); padding-right: 1.25ch;
    text-align: right; color: var(--text-muted); user-select: none;
    background: var(--surface-inset);
  }
</style>
