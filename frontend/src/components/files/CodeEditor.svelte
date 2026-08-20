<script lang="ts">
  import { onDestroy } from 'svelte';
  import * as m from '../../paraglide/messages';
  import type { EditorView } from '@codemirror/view';

  interface Props {
    texto: string;
    path: string;
    editavel: boolean;
    // Texto da base. Com ele o editor desenha o DIFF por dentro (unifiedMergeView): arquivo
    // inteiro, mudanças embutidas, trechos iguais dobrados. `null` = mostra só o arquivo.
    original?: string | null;
    onChange?: (texto: string) => void;
    // Ctrl/Cmd+S dentro do editor: salvar é do hospedeiro, a tecla é daqui.
    onSalvar?: () => void;
  }
  let { texto, path, editavel, original = null, onChange, onSalvar }: Props = $props();

  let caixa = $state<HTMLDivElement | null>(null);
  let view: EditorView | null = null;
  // Qual texto ESTE componente colocou no editor por último. Sem isso, cada tecla digitada
  // dispara o onChange, o hospedeiro reatribui `texto`, e o efeito de sincronia devolveria o
  // documento inteiro — cursor no começo, digitação impossível.
  let ultimoTexto = '';
  let montando = false;

  // O CodeMirror inteiro (com as gramáticas) é import DINÂMICO: são centenas de KB que só quem
  // abre um arquivo precisa, e o app abre no chat.
  async function montar(el: HTMLDivElement, doc: string, base: string | null) {
    montando = true;
    const [{ EditorView: EV, keymap, lineNumbers, highlightActiveLine, drawSelection, rectangularSelection, crosshairCursor },
           { EditorState },
           { defaultKeymap, history, historyKeymap, indentWithTab },
           { searchKeymap, highlightSelectionMatches, search },
           { syntaxHighlighting, defaultHighlightStyle, foldGutter, foldKeymap, indentOnInput, bracketMatching },
           { oneDarkHighlightStyle },
           { unifiedMergeView }] = await Promise.all([
      import('@codemirror/view'),
      import('@codemirror/state'),
      import('@codemirror/commands'),
      import('@codemirror/search'),
      import('@codemirror/language'),
      import('@codemirror/theme-one-dark'),
      import('@codemirror/merge'),
    ]);
    const linguagem = await gramatica(path);

    const tema = EV.theme({
      '&': { height: '100%', fontSize: '12.5px', backgroundColor: 'transparent', color: 'var(--text-primary)' },
      '.cm-scroller > .cm-content': { paddingBottom: '12px' },
      '.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.7', overflow: 'auto' },
      '.cm-line': { padding: '0 16px' },
      '.cm-gutterElement': { paddingLeft: '10px' },
      '.cm-gutters': { backgroundColor: 'transparent', border: 'none', color: 'var(--text-muted)' },
      '.cm-activeLine': { backgroundColor: 'var(--bg-hover)' },
      '.cm-activeLineGutter': { backgroundColor: 'transparent', color: 'var(--text-secondary)' },
      '.cm-content': { caretColor: 'var(--accent)' },
      '.cm-cursor, .cm-dropCursor': { borderLeftColor: 'var(--accent)' },
      '.cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection':
        { backgroundColor: 'var(--accent-dim)' },
      '.cm-panels': { backgroundColor: 'var(--surface-raised)', color: 'var(--text-primary)' },
      '.cm-searchMatch': { backgroundColor: 'var(--accent-dim)' },
      '.cm-searchMatch.cm-searchMatch-selected': { backgroundColor: 'var(--accent)', color: 'var(--bg-base)' },
      // Diff embutido: as mesmas famílias de cor do resto do app (sucesso/erro), não o verde e o
      // vermelho que o pacote traz. Os seletores levam o `&dark`/`&light` porque é assim que o
      // próprio pacote escreve os dele — sem isso a regra tem menos especificidade e perde, e a
      // marca de palavra continua saindo como aquela faixa de 2px que parece sublinhado.
      // Faixa de 2px na borda ALÉM da cor de fundo: cor sozinha não informa quem não distingue
      // verde de vermelho.
      '.cm-changedLine': { backgroundColor: 'color-mix(in srgb, var(--success) 12%, transparent)',
                           boxShadow: 'inset 2px 0 0 var(--success)' },
      '.cm-deletedChunk': { backgroundColor: 'color-mix(in srgb, var(--error) 12%, transparent)',
                            boxShadow: 'inset 2px 0 0 var(--error)' },
      // `&.cm-merge-b` é o seletor VÁLIDO em `theme()` (o `&dark`/`&light` que o pacote usa só
      // existe no baseTheme dele — passá-lo aqui levanta "Unsupported selector" e o editor não
      // monta). Precisa dessa especificidade pra vencer a faixa de 2px que parece sublinhado.
      '&.cm-merge-b .cm-changedText, .cm-changedText':
        { background: 'color-mix(in srgb, var(--success) 24%, transparent)', borderRadius: '2px' },
      '&.cm-merge-a .cm-changedText, .cm-deletedChunk .cm-deletedText':
        { background: 'color-mix(in srgb, var(--error) 24%, transparent)', borderRadius: '2px' },
      // `ins`/`del` são as tags que o pacote usa; sem isto o navegador risca e sublinha o código.
      '.cm-insertedLine, .cm-deletedLine': { textDecoration: 'none' },
      // A dobra vem clara por padrão (o estilo do pacote assume tema claro). Sem isto ela vira
      // uma faixa branca no meio do código escuro.
      '.cm-collapsedLines': {
        color: 'var(--text-muted)', backgroundColor: 'var(--fill-subtle)',
        padding: '4px 8px 4px 64px', fontSize: '11px',
        borderTop: '1px solid var(--border-subtle)', borderBottom: '1px solid var(--border-subtle)',
      },
      '.cm-collapsedLines:hover': { backgroundColor: 'var(--bg-hover)' },
      '.cm-collapsedLines::before, .cm-collapsedLines::after': { content: "'⋯'", opacity: '0.6' },
    });

    const teclas = [...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab, ...foldKeymap];
    if (onSalvar) {
      teclas.unshift({
        key: 'Mod-s',
        run: () => { onSalvar(); return true; },   // true = a tecla foi consumida: não salva a página
      });
    }

    const v = new EV({
      parent: el,
      state: EditorState.create({
        doc,
        extensions: [
          lineNumbers(),
          foldGutter(),
          history(),
          drawSelection(),
          rectangularSelection(),
          crosshairCursor(),
          highlightActiveLine(),
          highlightSelectionMatches(),
          search({ top: true }),
          indentOnInput(),
          bracketMatching(),
          // SÓ as cores de sintaxe do one-dark, nunca o tema inteiro: o `oneDark` pinta o fundo
          // do editor e da calha de #282c34 e ganha do nosso tema, e era isso que punha o código
          // dentro de uma segunda moldura, com a cor errada, sobre a superfície do app.
          syntaxHighlighting(oneDarkHighlightStyle),
          syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
          ...(linguagem ? [linguagem] : []),
          // O diff mora DENTRO do editor. `mergeControls: false` porque aceitar/rejeitar trecho
          // é operação de merge, não de leitura — quem resolve conflito aqui é o git, não a tela.
          // `collapseUnchanged` é o que tira o "parede de código igual" de arquivo grande.
          ...(base !== null ? [unifiedMergeView({
            original: base,
            mergeControls: false,
            highlightChanges: true,
            gutter: true,
            allowInlineDiffs: true,
            collapseUnchanged: { margin: 3, minSize: 4 },
          })] : []),
          // O rótulo da dobra vem do pacote em inglês e passa por `phrase` — é o gancho de i18n
          // dele, e é o que mantém a regra do projeto (texto de interface sai de `m.*`).
          EditorState.phrases.of({ '$ unchanged lines': m.arq_linhas_sem_mudanca() }),
          keymap.of(teclas),
          tema,
          EV.lineWrapping,
          EV.editable.of(editavel),
          EV.updateListener.of((u) => {
            if (!u.docChanged) return;
            ultimoTexto = u.state.doc.toString();
            onChange?.(ultimoTexto);
          }),
        ],
      }),
    });
    ultimoTexto = doc;
    montando = false;
    return v;
  }

  // Gramática por extensão. Só o que o app realmente encontra — o resto cai em texto plano com
  // numeração e busca, que já é melhor do que um `<pre>`.
  async function gramatica(p: string) {
    const ext = p.slice(p.lastIndexOf('.') + 1).toLowerCase();
    switch (ext) {
      case 'ts': case 'tsx': case 'mts': case 'cts':
        return (await import('@codemirror/lang-javascript')).javascript({ typescript: true, jsx: ext === 'tsx' });
      case 'js': case 'jsx': case 'mjs': case 'cjs':
        return (await import('@codemirror/lang-javascript')).javascript({ jsx: ext === 'jsx' });
      case 'py': return (await import('@codemirror/lang-python')).python();
      case 'json': return (await import('@codemirror/lang-json')).json();
      case 'md': case 'markdown': return (await import('@codemirror/lang-markdown')).markdown();
      case 'css': return (await import('@codemirror/lang-css')).css();
      case 'html': case 'htm': case 'svelte': case 'vue':
        return (await import('@codemirror/lang-html')).html();
      case 'sql': return (await import('@codemirror/lang-sql')).sql();
      case 'rs': return (await import('@codemirror/lang-rust')).rust();
      case 'c': case 'h': case 'cc': case 'cpp': case 'hpp':
        return (await import('@codemirror/lang-cpp')).cpp();
      case 'java': case 'cs': case 'kt':
        // O modo Java cobre a família de chaves (C#, Kotlin) bem o bastante pra leitura; não há
        // gramática oficial de C# no CodeMirror e escrever uma não é escopo desta tela.
        return (await import('@codemirror/lang-java')).java();
      case 'php': return (await import('@codemirror/lang-php')).php();
      case 'xml': case 'xsd': case 'xsl': return (await import('@codemirror/lang-xml')).xml();
      case 'yml': case 'yaml': return (await import('@codemirror/lang-yaml')).yaml();
      default: return null;
    }
  }

  // Monta/remonta quando a CAIXA ou o ARQUIVO trocam. Trocar de arquivo remonta de propósito: a
  // gramática e o histórico de desfazer são daquele arquivo, e carregar o desfazer de um arquivo
  // dentro de outro seria pior que remontar.
  $effect(() => {
    const el = caixa;
    const p = path;
    // `original` na lista de dependências: trocar de escopo (branch ↔ não commitado) muda a base
    // e a extensão do diff só entra na CRIAÇÃO do estado — sem remontar, a tela mostraria o
    // diff do escopo anterior.
    void original;
    if (!el) return;
    let vivo = true;
    const doc = texto;
    const base = original;
    montar(el, doc, base).then((v) => {
      if (!vivo) { v.destroy(); return; }
      view?.destroy();
      view = v;
      void p;
    });
    return () => { vivo = false; };
  });

  // Texto novo vindo de fora (recarregar depois de um conflito, por exemplo) entra no documento.
  // Comparar com `ultimoTexto` é o que impede o laço com o próprio onChange.
  $effect(() => {
    const t = texto;
    const v = view;
    if (!v || montando || t === ultimoTexto) return;
    ultimoTexto = t;
    v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: t } });
  });

  // Ligar/desligar a edição sem remontar. `contentEditable` direto: reconfigurar a extensão
  // exigiria um Compartment e mais um estado pra manter, e o efeito visível é o mesmo.
  $effect(() => {
    const podeEditar = editavel;
    view?.contentDOM.setAttribute('contenteditable', podeEditar ? 'true' : 'false');
  });

  onDestroy(() => { view?.destroy(); view = null; });
</script>

<div class="editor" bind:this={caixa} data-editavel={editavel}></div>

<style>
  .editor {
    flex: 1;
    min-height: 0;
    overflow: hidden;
    /* Herda a superfície do hospedeiro (o `.visor` define `--cp-editor-surface`), pra aba ativa,
       sub-barra e código serem a MESMA cor — é isso que faz o cabeçalho e o código lerem como
       uma peça só. Sem o hospedeiro, cai no fundo opaco do app. */
    background: var(--cp-editor-surface, var(--bg-base));
  }
</style>
