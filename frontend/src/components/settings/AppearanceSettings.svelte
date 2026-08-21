<script lang="ts">
  import ThemeToggle from '../ThemeToggle.svelte';
  import BackgroundToggle from '../BackgroundToggle.svelte';
  import SegmentedPicker from '../SegmentedPicker.svelte';
  import AparenciaAmostra from './AparenciaAmostra.svelte';
  import {
    getReadMode, setReadMode, getPanelStyle, setPanelStyle,
    getReadAlpha, setReadAlpha, getTextBoost, setTextBoost,
    getFontPref, setFontPref, getMedidaTexto, setMedidaTexto,
    getSurfaceSolid, setSurfaceSolid,
    getBackdropBlur, setBackdropBlur,
    getBgPref, getDesktopGlass, setDesktopGlass,
    READ_ALPHA_PADRAO, TEXT_BOOST_PADRAO, SURFACE_SOLID_PADRAO,
    type ReadMode, type PanelStyle, type FontPref, type MedidaTexto, type BackdropBlurPref, type BgPref,
  } from '../../lib/background';
  import { getThemePref, getTextoDoDesktop, setTextoDoDesktop, type ThemePref } from '../../lib/theme';
  import { buscarPaleta, aplicarPaleta, paletaEmCache } from '../../lib/desktopTheme';
  import { temCorTema, limparCorTema } from '../../lib/corTema';
  import CorTemaSettings from './CorTemaSettings.svelte';
  import { sidebarPrefs, type SidebarHeight } from '../../lib/sidebarPrefs.svelte';
  import { navMode, type NavMode } from '../../lib/navMode.svelte';
  import { toolLook, type ToolLook } from '../../lib/toolLook.svelte';
  import { taskRows, type TaskRowsPref } from '../../lib/taskRows.svelte';
  import { tableChartPref, type TableChartPref } from '../../lib/tableChartPref.svelte';
  import * as m from '../../paraglide/messages';

  interface Props {
    /** Desktop: oferece o botao "Ver ao vivo", que troca o painel pela caixinha flutuante. */
    podeAoVivo?: boolean;
    onVerAoVivo?: () => void;
    /** Ja estamos DENTRO da caixinha: a previa embutida sai (o chat de verdade esta a vista atras). */
    semPrevia?: boolean;
  }
  let { podeAoVivo = false, onVerAoVivo, semPrevia = false }: Props = $props();

  let leitura = $state<ReadMode>(getReadMode());
  let paineis = $state<PanelStyle>(getPanelStyle());
  let solidez = $state(getReadAlpha());
  let contraste = $state(getTextBoost());
  let fonte = $state<FontPref>(getFontPref());
  let desfoque = $state<BackdropBlurPref>(getBackdropBlur());
  let texto = $state<Record<MedidaTexto, number>>({
    size: getMedidaTexto('size'), lh: getMedidaTexto('lh'), width: getMedidaTexto('width'),
  });
  // Breakpoint desktop, reativo e nao um retrato do boot: atravessar os 820px (girar o tablet,
  // redimensionar a janela) precisa aparecer/sumir com o slider de largura na hora.
  let isDesktop = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(min-width: 820px)');
    const on = () => (isDesktop = mq.matches); on();
    mq.addEventListener('change', on);
    return () => mq.removeEventListener('change', on);
  });

  const medidasTexto: { v: MedidaTexto; label: string; aria: string }[] = [
    { v: 'size', label: m.config_aparencia_tamanho(), aria: m.config_aparencia_tamanho_aria() },
    { v: 'lh', label: m.config_aparencia_entrelinha(), aria: m.config_aparencia_entrelinha_aria() },
    { v: 'width', label: m.config_aparencia_largura(), aria: m.config_aparencia_largura_aria() },
  ];

  const opcoesLeitura: { v: ReadMode; label: string; aria: string }[] = [
    { v: 'auto', label: m.config_aparencia_automatica(), aria: m.config_aparencia_automatica_aria() },
    { v: 'glass', label: m.lista_agrupar_nenhum(), aria: m.config_aparencia_nenhum_aria() },
    { v: 'text', label: m.config_aparencia_texto(), aria: m.config_aparencia_texto_aria() },
    { v: 'solid', label: m.config_aparencia_folha(), aria: m.config_aparencia_folha_aria() },
  ];
  const opcoesFonte: { v: FontPref; label: string; aria: string }[] = [
    { v: 'system', label: m.config_aparencia_sistema(), aria: m.config_aparencia_sistema_aria() },
    { v: 'mono', label: m.config_aparencia_mono(), aria: m.config_aparencia_mono_aria() },
  ];
  const opcoesDesfoque: { v: BackdropBlurPref; label: string; aria: string }[] = [
    { v: 'off', label: m.lista_agrupar_nenhum(), aria: m.config_aparencia_desfoque_off_aria() },
    { v: 'light', label: m.config_aparencia_leve(), aria: m.config_aparencia_leve_aria() },
    { v: 'strong', label: m.config_aparencia_forte(), aria: m.config_aparencia_forte_aria() },
  ];
  const opcoesVidroDesktop: { v: 'janela' | 'vidro'; label: string; aria: string }[] = [
    { v: 'janela', label: m.config_aparencia_janela(), aria: m.config_aparencia_janela_aria() },
    { v: 'vidro', label: m.config_aparencia_vidro(), aria: m.config_aparencia_vidro_aria() },
  ];
  const opcoesPaineis: { v: PanelStyle; label: string; aria: string }[] = [
    { v: 'card', label: m.config_aparencia_caixa_solta(), aria: m.config_aparencia_caixa_solta_aria() },
    { v: 'edge', label: m.config_aparencia_colados(), aria: m.config_aparencia_colados_aria() },
  ];
  let resetSeq = $state(0);
  // Espelho da "Solidez das caixas" (o slider vive no BackgroundToggle) so pra o botao saber se ha o
  // que desfazer. Reler no reset basta: quem edita o valor e o proprio BackgroundToggle, que remonta
  // junto pelo `{#key resetSeq}`.
  let caixas = $state(getSurfaceSolid());
  $effect(() => { resetSeq; caixas = getSurfaceSolid(); });
  // Mesmo espelho do `caixas`: temCorTema() lê localStorage sem sinal reativo; o $state é
  // atualizado pelo onMudanca da seção e relido no reset (efeito acima do resetSeq não cobre —
  // mantido separado porque a fonte da mudança é outra).
  let temCor = $state(temCorTema());
  // Mesma ideia pro fundo: getBgPref() le o localStorage, entao um {#if} direto nao rastreia nada e
  // nunca re-executa quando o BackgroundToggle troca a escolha por dentro. Espelhado em $state e
  // atualizado pelo callback onEscolha (ver BackgroundToggle) — nao por effect, porque a mudanca so
  // acontece por clique, nunca sozinha.
  let fundo = $state<BgPref>(getBgPref());
  let vidroDesktop = $state(getDesktopGlass());
  // `tema` e $state e nao `getThemePref()` direto: aquela funcao le localStorage por chamada comum,
  // sem sinal reativo, entao o bloco nunca reavaliaria e o controle so apareceria ao reabrir a
  // folha. Mesmo remedio do `caixas` (linhas 71-76) e do `onEscolha` do BackgroundToggle.
  let tema = $state(getThemePref());
  let textoDesktop = $state(getTextoDoDesktop());
  // Repinta na hora com o que ja se sabe (cache do Fix 3): sem isto, um fetch que devolve null
  // (backend piscou) gravava a preferencia e deixava as letras na cor antiga ate o proximo foco —
  // o controle mostrando "App" com o texto ainda no tom do desktop. So vai pra rede se nunca houve
  // paleta nesta aba. Devolve se REALMENTE repintou: quem chama usa isso pra decidir se persiste a
  // escolha ou desfaz o segmentado — sem o retorno, o controle mostrava a escolha nova com as letras
  // na cor velha, porque nada aqui avisava o chamador que o repaint nao aconteceu.
  function reaplicar(): Promise<boolean> {
    const cache = paletaEmCache();
    if (cache) { aplicarPaleta(cache, textoDesktop); return Promise.resolve(true); }
    return buscarPaleta().then((p) => {
      if (!p) return false;
      aplicarPaleta(p, textoDesktop);
      return true;
    });
  }

  // VOLTAR AO PADRAO: so as medidas que a amostra mostra. NAO mexe em tema, fonte, papel de parede
  // nem paineis — quem arrasta sliders e se perde quer desfazer os sliders, nao perder a foto que
  // escolheu.
  //
  // Os valores vem das CONSTANTES exportadas por lib/background, nunca de numero escrito aqui: a
  // primeira versao chutou 50 pro contraste e 12 pra solidez da folha, quando os de fabrica sao 10 e
  // 92. O estrago era silencioso e dobrado — o botao nascia habilitado numa instalacao virgem (92 !=
  // 12) e, clicado, DERRUBAVA a solidez da folha de 92 pra 12, uma mudanca visual grande que ninguem
  // pediu.
  const temAjuste = $derived(
    texto.size !== 100 || texto.lh !== 100 || texto.width !== 100 ||
    contraste !== TEXT_BOOST_PADRAO || solidez !== READ_ALPHA_PADRAO || leitura !== 'auto' ||
    caixas !== SURFACE_SOLID_PADRAO || temCor,
  );
  function voltarAoPadrao() {
    (['size', 'lh', 'width'] as MedidaTexto[]).forEach((m) => { texto[m] = 100; setMedidaTexto(m, 100); });
    contraste = TEXT_BOOST_PADRAO; setTextBoost(TEXT_BOOST_PADRAO);
    solidez = READ_ALPHA_PADRAO; setReadAlpha(READ_ALPHA_PADRAO);
    leitura = 'auto'; setReadMode('auto');
    caixas = SURFACE_SOLID_PADRAO; setSurfaceSolid(SURFACE_SOLID_PADRAO);
    // Cor do tema entra no reset: quem mistura tinta/destaque e se perde quer voltar ao neutro de
    // fábrica pelo MESMO botão — e o {#key resetSeq} remonta a seção desmarcando os swatches.
    limparCorTema(); temCor = false;
    // O slider "Solidez das caixas" vive no BackgroundToggle, que guarda o PROPRIO $state. Sem
    // remontar, o valor aplicado voltava ao padrao mas o slider de la seguia mostrando o numero
    // antigo ate reabrir a tela — a tela mentindo sobre o proprio estado.
    resetSeq += 1;
  }

  const opcoesAltura: { v: SidebarHeight; label: string; aria: string }[] = [
    { v: 'full', label: m.config_aparencia_altura_total(), aria: m.config_aparencia_altura_total_aria() },
    { v: 'content', label: m.config_aparencia_so_conteudo(), aria: m.config_aparencia_so_conteudo_aria() },
  ];

  // Follow-up visual: MODO da navegação com a sidebar recolhida. O rail (Barra lateral) é o
  // padrão — decisão do usuário; 'Abas no topo' é a faixa horizontal da SessionTabs.
  const opcoesModo: { v: NavMode; label: string; aria: string }[] = [
    { v: 'rail', label: m.config_aparencia_barra_lateral(), aria: m.config_aparencia_barra_lateral_aria() },
    { v: 'tabs', label: m.config_aparencia_abas_topo(), aria: m.config_aparencia_abas_topo_aria() },
  ];
</script>

<div class="ap-sheet">
  <!-- Amostra GRUDADA no topo: todo slider daqui muda a conversa, que fica atras deste painel (e no
       celular nao ha "atras" nenhum — o painel e a tela). Sticky pra ela continuar a vista enquanto
       se rola ate os sliders la embaixo. -->
  <!-- Grudado no topo SO quando ha previa: na caixinha do "ao vivo" a previa nao existe (o chat de
       verdade esta a vista atras), e um botao sozinho preso no topo com fundo translucido virava uma
       tarja por cima do texto que rolava embaixo. -->
  <div class="ap-amostra" class:ap-amostra--solta={semPrevia}>
    {#if !semPrevia}<AparenciaAmostra />{/if}
    <div class="ap-acoes">
      {#if podeAoVivo}
        <!-- Desktop: a previa embutida e uma amostra; isto revela a conversa DE VERDADE atras,
             encolhendo o painel numa caixinha no canto. -->
        <button class="ap-padrao" onclick={onVerAoVivo}>{m.config_aparencia_ver_ao_vivo()}</button>
      {/if}
      <button class="ap-padrao" onclick={voltarAoPadrao} disabled={!temAjuste}>
        {m.config_aparencia_voltar_padrao()}
      </button>
    </div>
  </div>

  <div class="ap-row">
    <div class="ap-label">
      <strong>{m.config_tema_curto()}</strong>
      <span>{m.config_aparencia_tema_desc()}</span>
    </div>
    <ThemeToggle onEscolha={(p) => (tema = p)} />
  </div>

  <!-- Cor manual do tema (destaque + tinta de fundo), referência do "Customize Theme" do
       super.engineering. Fora do tema Desktop: lá a paleta Material You já é a dona dos tokens e
       um picker aqui escreveria por cima dela (gate invertido ao da "Cor do texto" abaixo). O
       `{#key}` remonta no "Voltar ao padrão", que limpa as cores gravadas — senão os swatches
       continuariam marcados numa cor que já não está aplicada. -->
  {#if tema !== 'desktop'}
    <div class="ap-row ap-row--stack">
      <div class="ap-label">
        <strong>{m.config_aparencia_cor_tema()}</strong>
        <span>{m.config_aparencia_cor_tema_desc()}</span>
      </div>
      {#key resetSeq}<CorTemaSettings onMudanca={() => (temCor = temCorTema())} />{/key}
    </div>
  {/if}

  {#if tema === 'desktop'}
    <div class="ap-row">
      <div class="ap-label">
        <strong>{m.config_aparencia_cor_texto()}</strong>
        <span>{m.config_aparencia_cor_texto_desc()}</span>
      </div>
      <SegmentedPicker
        value={textoDesktop ? 'desktop' : 'app'}
        options={[
          { v: 'desktop', label: m.config_aparencia_desktop(), aria: m.config_aparencia_cor_texto_desktop_aria() },
          { v: 'app', label: m.config_aparencia_app(), aria: m.config_aparencia_cor_texto_app_aria() },
        ]}
        ariaLabel={m.config_aparencia_cor_texto()}
        onPick={(v) => {
          // So persiste (e so deixa a escolha nova visivel) se o repaint realmente aconteceu — senao
          // o segmentado mostraria "Desktop" com as letras ainda na cor do app (ou vice-versa), o
          // controle mentindo sobre o que esta na tela.
          const anterior = textoDesktop;
          textoDesktop = v === 'desktop';
          reaplicar().then((ok) => {
            if (ok) setTextoDoDesktop(textoDesktop);
            else textoDesktop = anterior;
          });
        }}
      />
    </div>
  {/if}

  <div class="ap-row ap-row--stack">
    <div class="ap-label">
      <strong>{m.config_fundo_curto()}</strong>
      <span>{m.config_aparencia_fundo_desc()}</span>
    </div>
    <!-- `{#key}`: o "Voltar ao padrao" grava a solidez das caixas, mas o slider vive aqui dentro com
         estado proprio — remontar e o que faz o numero na tela bater com o valor aplicado. -->
    {#key resetSeq}<BackgroundToggle onEscolha={(p) => (fundo = p)} />{/key}
  </div>

  <!-- Só no fundo Desktop: é a escolha entre janela transparente de verdade (vê o que está atrás do
       app) e uma cópia do papel de parede dentro da página. A cópia é o que devolve vidro e cor às
       caixas — `backdrop-filter` só borra o que a própria página pintou, e atrás de transparência
       não há pixel nenhum (ver GLASS_KEY em lib/background.ts). -->
  {#if fundo === 'desktop'}
    <div class="ap-row">
      <div class="ap-label">
        <strong>{m.config_aparencia_papel_parede()}</strong>
        <span>{m.config_aparencia_papel_parede_desc()}</span>
      </div>
      <SegmentedPicker
        value={vidroDesktop ? 'vidro' : 'janela'}
        options={opcoesVidroDesktop}
        ariaLabel={m.config_aparencia_papel_parede()}
        onPick={(v) => { vidroDesktop = v === 'vidro'; setDesktopGlass(vidroDesktop); }}
      />
    </div>
  {/if}

  <!-- Só faz sentido com foto de fundo — sem imagem não há o que embaçar. Aparecer aqui ensina que
       a opção existe, e desligada NÃO muda nada no resto da tela: o scrim, a leitura e a solidez
       das caixas ficam intocados. -->
  <div class="ap-row">
    <div class="ap-label">
      <strong>{m.config_aparencia_desfoque()}</strong>
      <span>{m.config_aparencia_desfoque_desc()}</span>
    </div>
    {#if fundo !== 'desktop' || vidroDesktop}
      <SegmentedPicker value={desfoque} options={opcoesDesfoque} ariaLabel={m.config_aparencia_desfoque()}
                       onPick={(v) => { desfoque = v; setBackdropBlur(v); }} />
    {:else}
      <p class="hint">{m.config_aparencia_desfoque_hint()}</p>
    {/if}
  </div>

  <!-- Pele das chamadas de ferramenta. Interruptor, não migração: 'Clássico' é o padrão e nada
       muda pra quem não mexer. As duas leem os MESMOS dados — o diff da edição, o erro em texto e
       o realce do Read continuam iguais nas duas. -->
  <div class="ap-row">
    <div class="ap-label">
      <strong>{m.config_aparencia_chamadas()}</strong>
      <span>{m.config_aparencia_chamadas_desc()}</span>
    </div>
    <SegmentedPicker
      value={toolLook.look}
      options={[
        { v: 'classico', label: m.config_aparencia_classico(), aria: m.config_aparencia_classico_aria() },
        { v: 'chips', label: m.config_aparencia_chips(), aria: m.config_aparencia_chips_aria() },
      ]}
      ariaLabel={m.config_aparencia_chamadas()}
      onPick={(v) => { toolLook.look = v as ToolLook; }}
    />
  </div>

  <!-- Chave SEPARADA da pele das ferramentas, de propósito: são duas decisões independentes.
       Desligada, as chamadas de tarefa continuam aparecendo como linha de ferramenta normal. -->
  <div class="ap-row">
    <div class="ap-label">
      <strong>{m.config_aparencia_tarefas()}</strong>
      <span>{m.config_aparencia_tarefas_desc()}</span>
    </div>
    <SegmentedPicker
      value={taskRows.pref}
      options={[
        { v: 'off', label: m.config_aparencia_nao_mostrar(), aria: m.config_aparencia_nao_mostrar_aria() },
        { v: 'on', label: m.config_aparencia_capsulas(), aria: m.config_aparencia_capsulas_aria() },
      ]}
      ariaLabel={m.config_aparencia_tarefas()}
      onPick={(v) => { taskRows.pref = v as TaskRowsPref; }}
    />
  </div>

  <!-- Desligado por padrão: o recurso existe e está testado, mas fica fora do caminho até haver
       caso de uso. Com off o uPlot nem é baixado (import dinâmico). -->
  <div class="ap-row">
    <div class="ap-label">
      <strong>{m.config_aparencia_grafico()}</strong>
      <span>{m.config_aparencia_grafico_desc()}</span>
    </div>
    <SegmentedPicker
      value={tableChartPref.pref}
      options={[
        { v: 'off', label: m.config_aparencia_nao_mostrar(), aria: m.config_aparencia_sem_grafico_aria() },
        { v: 'on', label: m.config_aparencia_mostrar(), aria: m.config_aparencia_com_grafico_aria() },
      ]}
      ariaLabel={m.config_aparencia_grafico()}
      onPick={(v) => { tableChartPref.pref = v as TableChartPref; }}
    />
  </div>

  <div class="ap-row ap-row--stack">
    <div class="ap-head">
      <div class="ap-label">
        <strong>{m.config_aparencia_leitura()}</strong>
        <span>{m.config_aparencia_leitura_desc()}</span>
      </div>
      <SegmentedPicker value={leitura} options={opcoesLeitura} ariaLabel={m.config_aparencia_leitura()}
                       onPick={(v) => { leitura = v; setReadMode(v); }} />
    </div>
    {#if leitura !== 'glass'}
      <!-- Mesma lógica do slider do fundo: 100 tapa a foto atrás da conversa, 0 deixa ela passar
           inteira. "Sólida" no talo virava um bloco escuro — o ponto certo é olhando. -->
      <label class="ap-slider">
        <span>{leitura === 'solid' ? m.config_aparencia_solidez_folha() : m.config_aparencia_forca()}</span>
        <input type="range" min="0" max="100" step="1" value={solidez}
               oninput={(e) => { solidez = +(e.currentTarget as HTMLInputElement).value; setReadAlpha(solidez); }} />
        <em>{solidez}</em>
      </label>
    {/if}
    {#if leitura === 'text' || leitura === 'auto'}
      <!-- Contraste do texto: os tokens do app são propositalmente mais escuros que branco (conforto
           em sessão longa); sobre foto isso não vale, e aqui você escolhe quanto do branco volta. -->
      <label class="ap-slider">
        <span>{m.config_aparencia_contraste()}</span>
        <input type="range" min="0" max="100" step="1" value={contraste}
               oninput={(e) => { contraste = +(e.currentTarget as HTMLInputElement).value; setTextBoost(contraste); }} />
        <em>{contraste}</em>
      </label>
    {/if}
  </div>

  <!-- As tres medidas que decidem o conforto de leitura, cada uma como escala (100 = o de hoje).
       A LARGURA so aparece no desktop: no celular a coluna e a tela inteira e o slider nao teria o
       que mover. Medido nesta conversa: 43 caracteres por linha no celular contra 93 no desktop —
       a faixa confortavel de leitura e 45 a 75, e e por isso que o mesmo texto cansa mais no monitor. -->
  <div class="ap-row ap-row--stack">
    <div class="ap-label">
      <strong>{m.config_aparencia_texto_conversa()}</strong>
      <span>{m.config_aparencia_texto_conversa_desc()}</span>
    </div>
    {#each medidasTexto as m (m.v)}
      {#if m.v !== 'width' || isDesktop}
        <label class="ap-slider">
          <span>{m.label}</span>
          <input type="range" min="50" max="150" step="1" value={texto[m.v]}
                 aria-label={m.aria}
                 oninput={(e) => { const n = +(e.currentTarget as HTMLInputElement).value; texto[m.v] = n; setMedidaTexto(m.v, n); }} />
          <em>{texto[m.v]}</em>
        </label>
      {/if}
    {/each}
  </div>

  <!-- Fonte: nenhuma API web lê a fonte do terminal, então a escolha é aqui. 'Monoespaçada' usa
       `--font-mono`, que nomeia JetBrains e cai no ui-monospace de quem não tiver. O TAMANHO tem
       controle proprio logo acima (escala sobre o padrao da tela), que convive com o zoom do
       navegador em vez de brigar com ele. -->
  <div class="ap-row">
    <div class="ap-label">
      <strong>{m.config_aparencia_fonte()}</strong>
      <span>{m.config_aparencia_fonte_desc()}</span>
    </div>
    <SegmentedPicker value={fonte} options={opcoesFonte} ariaLabel={m.config_aparencia_fonte()}
                     onPick={(v) => { fonte = v; setFontPref(v); }} />
  </div>

  <div class="ap-row">
    <div class="ap-label">
      <strong>{m.config_aparencia_paineis()}</strong>
      <span>{m.config_aparencia_paineis_desc()}</span>
    </div>
    <SegmentedPicker value={paineis} options={opcoesPaineis} ariaLabel={m.config_aparencia_paineis()}
                     onPick={(v) => { paineis = v; setPanelStyle(v); }} />
  </div>

  <!-- Barra lateral: só existe no desktop (no celular a lista é a tela inteira), então a seção some
       abaixo de 820px em vez de oferecer um ajuste que não muda nada.
       Abrir e fechar a barra é o botão dela mesma — aqui ficam o modo da navegação recolhida e a
       altura. O modo reage na hora, sem reload (store $state, follow-up visual round 2). -->
  <div class="ap-row ap-row--desktop">
    <div class="ap-label">
      <strong>{m.config_aparencia_nav()}</strong>
      <span>{m.config_aparencia_nav_desc()}</span>
    </div>
    <SegmentedPicker value={navMode.mode} options={opcoesModo} ariaLabel={m.config_aparencia_nav_aria()}
                     onPick={(v) => (navMode.mode = v)} />
  </div>
  <div class="ap-row ap-row--desktop">
    <div class="ap-label">
      <strong>{m.config_aparencia_altura()}</strong>
      <span>{m.config_aparencia_altura_desc()}</span>
    </div>
    <SegmentedPicker value={sidebarPrefs.height} options={opcoesAltura} ariaLabel={m.config_aparencia_altura()}
                     onPick={(v) => (sidebarPrefs.height = v)} />
  </div>
</div>

<style>
  /* Container query, nao media query: quem aperta a linha e a largura do PAINEL, nao a da janela.
     No desktop o dock tem ~530px numa tela de 1440 — uma media query de 560px nunca dispararia ali,
     e era exatamente onde a descricao quebrava em palavras soltas ao lado do segmentado. O painel
     ainda e redimensionavel, entao o corte tem que seguir a largura de verdade. */
  .ap-sheet { container-type: inline-size; }

  /* Sticky com o fundo do painel por tras: sem o fundo, o texto dos sliders passaria POR CIMA da
     amostra ao rolar, e a amostra existe justamente pra ser um pedaco fiel da conversa. */
  .ap-amostra {
    position: sticky;
    top: 0;
    z-index: 2;
    padding-bottom: var(--space-2);
    background: var(--glass-panel, var(--bg-surface));
  }
  .ap-amostra--solta { position: static; background: none; padding-bottom: var(--space-3); }
  .ap-acoes { display: flex; gap: var(--space-2); }
  .ap-padrao {
    flex: 1;
    width: 100%;
    height: 34px;
    border: 1px solid var(--border-subtle);
    border-radius: var(--radius-md);
    background: var(--surface-raised);
    color: var(--text-secondary);
    font-size: var(--text-xs);
  }
  .ap-padrao:disabled { opacity: 0.45; cursor: default; }
  @media (hover: hover) { .ap-padrao:not(:disabled):hover { color: var(--text-primary); background: var(--bg-hover); } }
  .ap-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: var(--space-3);
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border-subtle);
  }
  /* O bloco do fundo tem seletor + slider + links: em coluna ele respira, em linha aperta tudo. */
  .ap-row--stack { flex-direction: column; align-items: stretch; }
  .ap-row:last-child { border-bottom: 0; }
  /* Ajuste que só tem efeito no desktop (a barra lateral não existe no celular). */
  @media (max-width: 819px) { .ap-row--desktop { display: none; } }
  .ap-label { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
  .ap-label strong { color: var(--text-primary); font-size: var(--text-sm); font-weight: 600; }
  .ap-label span { color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4; }
  .hint { margin: 0; color: var(--text-muted); font-size: var(--text-xs); line-height: 1.4; }
  /* Linha com título à esquerda e segmentado à direita, e o slider embaixo ocupando a largura. */
  .ap-head { display: flex; align-items: flex-start; justify-content: space-between; gap: var(--space-3); }
  /* O segmentado nao encolhe e o rotulo tem `min-width: 0`, entao ele cede TUDO: abaixo desta
     largura a descricao virava uma palavra por linha. Empilha antes de chegar la. Vale pras duas
     views — no celular a folha e a tela, no desktop e o painel. */
  @container (max-width: 560px) {
    .ap-head, .ap-row { flex-direction: column; align-items: stretch; }
  }
  .ap-slider { display: flex; align-items: center; gap: var(--space-2); margin-top: var(--space-2); }
  .ap-slider span { color: var(--text-muted); font-size: var(--text-xs); white-space: nowrap; }
  .ap-slider input { flex: 1; min-width: 120px; accent-color: var(--accent); }
  .ap-slider em { color: var(--text-muted); font-size: var(--text-xs); font-style: normal; min-width: 2ch; text-align: right; }
</style>
