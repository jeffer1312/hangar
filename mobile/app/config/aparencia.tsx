import { useRef, useState } from 'react';
import { Platform, ScrollView, Text, View } from 'react-native';
import { StyleSheet } from 'react-native-unistyles';
import { useHeaderHeight } from 'expo-router/react-navigation';
import * as ImagePicker from 'expo-image-picker';
import * as DocumentPicker from 'expo-document-picker';
import { Image } from 'expo-image';
import { MenuView } from '@react-native-menu/menu';
import type { ReactNode } from 'react';
import type { BackgroundEffect, PensamentoTools } from '@hangar/core';
import { Pagina } from '../../src/features/config/Pagina';
import { PageHeader, Pill } from '../../src/features/config/PageHeader';
import { SectionCard } from '../../src/features/config/SectionCard';
import { SectionTabs } from '../../src/features/config/SectionTabs';
import { SettingsRow } from '../../src/features/config/SettingsRow';
import { Segmented, type Option } from '../../src/features/config/Segmented';
import { ThemeTiles } from '../../src/features/config/ThemeTiles';
import { BackgroundTiles } from '../../src/features/config/BackgroundTiles';
import { EffectTiles } from '../../src/features/config/EffectTiles';
import { ColorDots } from '../../src/features/config/ColorDots';
import { Slider } from '../../src/features/config/Slider';
import { Tiles } from '../../src/features/config/Tiles';
import { ReadingArt, ToolsArt } from '../../src/features/config/AppearanceArt';
import { useSettingsColors } from '../../src/features/config/colors';
import { toast } from '../../src/ui/Toast';
import {
  CONVERSA_COMPACTA,
  leituraEmVigor,
  useAparencia,
  type DestaquePergunta,
  type Ferramentas,
  type Fundo,
  type HomeScreen,
  type Leitura,
  type Organizar,
  type Tema,
} from '../../src/stores/aparencia';
import * as m from '../../src/paraglide/messages';
import { opacidadeDe, transparenciaDe } from '../../src/theme/superficie';
import { ACENTO_PALETA, SEM_TINTA, TINTAS, type Paleta } from '../../src/theme/paleta';

// Destaques do desktop Rust (theme.rs ACCENTS / ACCENTS_LIGHT). O primeiro é o de fábrica: escolhê-lo
// grava "sem acento" e o tema volta à cor própria de cada modo.
const ACCENTS_DARK = ['#7c87e8', '#9b7cf0', '#f08a4b', '#e9b93f', '#3fbf6f', '#3cc4d6', '#e070b0'];
const ACCENTS_LIGHT = ['#5b6ad0', '#7a55d6', '#c8622a', '#a87a12', '#238a4f', '#1a8a9a', '#b8457f'];

type SectionKey = 'theme' | 'color' | 'background' | 'reading' | 'text' | 'conversation';
// Altura do trilho de abas preso no topo: a seção rolada para no pé dele, não embaixo.
const TABS_HEIGHT = 56;

export default function Aparencia() {
  const c = useSettingsColors();
  const header = useHeaderHeight();
  const tema = useAparencia((s) => s.tema);
  const fundo = useAparencia((s) => s.fundo);
  const imagemUri = useAparencia((s) => s.imagemUri);
  const panelAlpha = useAparencia((s) => s.panelAlpha);
  const surfaceAlpha = useAparencia((s) => s.surfaceAlpha);
  const acento = useAparencia((s) => s.acento);
  const pensamento = useAparencia((s) => s.pensamentoTools);
  const efeito = useAparencia((s) => s.efeito);
  const processando = useAparencia((s) => s.efeitoProcessando);
  const paleta = useAparencia((s) => s.paleta);
  const tinta = useAparencia((s) => s.tinta);
  const forcaTinta = useAparencia((s) => s.forcaTinta);
  const leitura = useAparencia((s) => s.leitura);
  const solidezFolha = useAparencia((s) => s.solidezFolha);
  const contraste = useAparencia((s) => s.contraste);
  const conversa = useAparencia((s) => s.conversa);
  const ferramentas = useAparencia((s) => s.ferramentas);
  const tarefas = useAparencia((s) => s.tarefas);
  const graficoTabela = useAparencia((s) => s.graficoTabela);
  const destaquePergunta = useAparencia((s) => s.destaquePergunta);
  const organizar = useAparencia((s) => s.organizar);
  const homeScreen = useAparencia((s) => s.homeScreen);
  const ap = useAparencia.getState;

  const scroll = useRef<ScrollView>(null);
  const offsets = useRef<Partial<Record<SectionKey, number>>>({});
  const [current, setCurrent] = useState<SectionKey | null>(null);
  const at = (k: SectionKey) => (e: { nativeEvent: { layout: { y: number } } }) => {
    offsets.current[k] = e.nativeEvent.layout.y;
  };
  // No iOS o conteúdo rola por baixo do cabeçalho transparente; no Android a rolagem começa abaixo dele.
  const jump = (k: SectionKey) => {
    setCurrent(k);
    const y = offsets.current[k];
    if (y === undefined) return;
    scroll.current?.scrollTo({ y: y - TABS_HEIGHT - (Platform.OS === 'ios' ? header : 0), animated: true });
  };

  const TEMAS: ReadonlyArray<Option<Tema>> = [
    { v: 'system', label: m.native_settings_theme_auto(), aria: m.config_idioma_sistema() },
    { v: 'light', label: m.native_settings_theme_light() },
    { v: 'dark', label: m.native_settings_theme_dark() },
  ];
  const FUNDOS: ReadonlyArray<Option<Fundo>> = [
    { v: 'flat', label: m.config_fundo_liso(), aria: m.config_fundo_chapado() },
    { v: 'texture', label: m.config_fundo_textura(), aria: m.config_fundo_grao() },
    { v: 'aurora', label: m.config_fundo_luz(), aria: m.config_fundo_aurora() },
    { v: 'image', label: m.config_fundo_imagem(), aria: m.config_fundo_usar_imagem() },
  ];
  // Ordem e rótulos do CHOICES de effects.rs.
  const EFEITOS: ReadonlyArray<Option<BackgroundEffect>> = [
    { v: 'none', label: m.native_settings_effect_none() },
    { v: 'dither', label: m.native_settings_effect_dither() },
    { v: 'ascii', label: m.native_settings_effect_ascii() },
    { v: 'halftone', label: m.native_settings_effect_halftone() },
    { v: 'scanlines', label: m.native_settings_effect_scanlines() },
  ];
  const PENSAMENTO: ReadonlyArray<Option<PensamentoTools>> = [
    { v: 'nada', label: m.config_aparencia_pensamento_nada(), aria: m.config_aparencia_pensamento_nada_aria() },
    { v: 'busca', label: m.config_aparencia_pensamento_busca(), aria: m.config_aparencia_pensamento_busca_aria() },
    { v: 'tudo', label: m.config_aparencia_pensamento_tudo(), aria: m.config_aparencia_pensamento_tudo_aria() },
  ];
  const PALETAS: ReadonlyArray<Option<Paleta>> = [
    { v: 'neutral', label: m.native_settings_palette_neutral() },
    { v: 'classic', label: m.native_settings_palette_classic() },
  ];
  const LEITURAS: ReadonlyArray<Option<Leitura>> = [
    { v: 'auto', label: m.native_settings_reading_auto() },
    { v: 'none', label: m.native_settings_reading_none() },
    { v: 'text', label: m.native_settings_reading_text() },
    { v: 'sheet', label: m.native_settings_reading_sheet() },
  ];
  const FERRAMENTAS: ReadonlyArray<Option<Ferramentas>> = [
    { v: 'classic', label: m.native_settings_tool_calls_classic() },
    { v: 'chips', label: m.native_settings_tool_calls_chips() },
    { v: 'tree', label: m.native_settings_tool_calls_tree() },
  ];
  const TAREFAS: ReadonlyArray<Option<'off' | 'on'>> = [
    { v: 'off', label: m.native_settings_task_list_hide() },
    { v: 'on', label: m.native_settings_task_list_progress() },
  ];
  const GRAFICO: ReadonlyArray<Option<'off' | 'on'>> = [
    { v: 'off', label: m.native_settings_table_chart_hide() },
    { v: 'on', label: m.native_settings_table_chart_show() },
  ];
  const DESTAQUES: ReadonlyArray<Option<DestaquePergunta>> = [
    { v: 'accent', label: m.native_settings_ask_highlight_accent() },
    { v: 'amber', label: m.native_settings_ask_highlight_amber() },
  ];
  const HOME_SCREENS: ReadonlyArray<Option<HomeScreen>> = [
    { v: 'sessions', label: m.home_open_app_sessions() },
    { v: 'newConversation', label: m.home_open_app_conversation() },
  ];
  const ORGANIZAR: ReadonlyArray<Option<Organizar>> = [
    { v: 'sessions', label: m.aparencia_organizar_sessoes_opcao() },
    { v: 'conversations', label: m.aparencia_organizar_conversas_opcao() },
  ];
  const modo = c.dark ? 'dark' : 'light';
  const accents = c.dark ? ACCENTS_DARK : ACCENTS_LIGHT;
  const dots = accents.map((color, i) => ({
    key: color,
    // A primeira amostra é o destaque da paleta (accent_swatches do Rust), não o escolhido agora.
    color: i === 0 ? ACENTO_PALETA[paleta][modo] : color,
    label: i === 0 ? m.config_aparencia_voltar_padrao() : m.native_settings_accent_n({ n: i + 1 }),
    selected: i === 0 ? acento === null : acento?.toLowerCase() === color,
    onPress: () => useAparencia.getState().setAcento(i === 0 ? null : color),
  }));
  const tintDots = TINTAS[modo].map((color, i) => ({
    key: `tint-${i}`,
    color: i === 0 ? SEM_TINTA[paleta][modo] : color,
    label: i === 0 ? m.native_settings_tint_none() : m.native_settings_tint_n({ n: i }),
    selected: tinta === i,
    empty: i === 0,
    onPress: () => ap().setTinta(i),
  }));
  const emVigor = leituraEmVigor({ leitura, fundo, imagemUri });
  const compacto = ferramentas === 'tree'
    && (Object.keys(CONVERSA_COMPACTA) as Array<keyof typeof conversa>).every((k) => conversa[k] === CONVERSA_COMPACTA[k]);
  const pct = (v: number) => <Text style={[styles.value, { color: c.muted }]}>{`${Math.round(v * 100)}%`}</Text>;

  // Três desfechos diferentes, três avisos diferentes: sem a permissão a galeria nem abre e o
  // resultado é indistinguível de um cancelamento — o toque parecia não fazer nada.
  const escolherImagem = async () => {
    try {
      const permissao = await ImagePicker.requestMediaLibraryPermissionsAsync();
      if (!permissao.granted) {
        toast.erro(m.aparencia_fundo_sem_permissao());
        return;
      }
      const r = await ImagePicker.launchImageLibraryAsync({ mediaTypes: ['images'], quality: 0.8 });
      if (r.canceled) return;
      if (!r.assets[0]) {
        console.warn('Aparência: picker voltou sem cancelar e sem imagem');
        toast.erro(m.aparencia_fundo_erro_galeria());
        return;
      }
      // A falha da CÓPIA é avisada pelo próprio store, com a mensagem dela.
      await useAparencia.getState().setImagemUri(r.assets[0].uri);
    } catch {
      toast.erro(m.aparencia_fundo_erro_galeria());
    }
  };

  // A imagem nem sempre está na galeria (baixada, recebida por app): o seletor de arquivos alcança o resto.
  const escolherArquivo = async () => {
    try {
      const r = await DocumentPicker.getDocumentAsync({ type: 'image/*', copyToCacheDirectory: true });
      if (r.canceled) return;
      if (!r.assets[0]) {
        toast.erro(m.aparencia_fundo_erro_galeria());
        return;
      }
      await useAparencia.getState().setImagemUri(r.assets[0].uri);
    } catch {
      toast.erro(m.aparencia_fundo_erro_galeria());
    }
  };

  // De onde vem a imagem: no celular nem toda foto mora na galeria.
  const origemImagem = (child: ReactNode) => (
    <MenuView
      actions={[
        { id: 'photos', title: m.composer_fotos(), image: Platform.select({ ios: 'photo' }) },
        { id: 'files', title: m.arq_aba(), image: Platform.select({ ios: 'folder' }) },
      ]}
      onPressAction={({ nativeEvent }) => void (nativeEvent.event === 'files' ? escolherArquivo() : escolherImagem())}
    >
      {child}
    </MenuView>
  );

  return (
    <Pagina scrollRef={scroll} stickyHeaderIndices={[1]}>
      <PageHeader
        title={m.config_modal_aparencia()}
        subtitle={m.config_aparencia_lead()}
        actions={[{ label: m.native_settings_reset(), icon: 'RotateCcw', onPress: () => ap().redefinir() }]}
      />
      <SectionTabs
        tabs={[
          { key: 'theme', label: m.config_tema_curto() },
          { key: 'color', label: m.native_settings_color() },
          { key: 'background', label: m.native_settings_background_group() },
          { key: 'reading', label: m.native_settings_reading_group() },
          { key: 'text', label: m.native_settings_text_group() },
          { key: 'conversation', label: m.native_settings_conversation_group() },
        ]}
        current={current}
        onPick={jump}
      />

      <SectionCard
        icon="Sparkles"
        title={m.native_settings_style()}
        subtitle={compacto ? m.native_settings_style_applied() : m.config_aparencia_estilo_dica()}
        extra={<Pill icon={compacto ? 'Check' : undefined} label={m.native_settings_style_compact()} onPress={() => ap().aplicarCompacto()} />}
      />

      <SectionCard icon="List" title={m.home_open_app()}>
        <View style={styles.tiles}>
          <Segmented options={HOME_SCREENS} value={homeScreen} onChange={(value) => ap().setHomeScreen(value)} label={m.home_open_app()} />
        </View>
      </SectionCard>

      <SectionCard icon="Monitor" title={m.config_tema_curto()} subtitle={m.config_aparencia_tema_desc()} onLayout={at('theme')}>
        <View style={styles.tiles}>
          <ThemeTiles options={TEMAS} value={tema} onChange={(v) => useAparencia.getState().setTema(v)} label={m.config_tema_curto()} paleta={paleta} />
        </View>
      </SectionCard>

      <SectionCard icon="Palette" title={m.native_settings_color()} onLayout={at('color')}>
        <SettingsRow title={m.native_settings_palette()}>
          <Segmented options={PALETAS} value={paleta} onChange={(v) => ap().setPaleta(v)} label={m.native_settings_palette()} />
        </SettingsRow>
        <SettingsRow title={m.native_settings_accent()} description={m.config_aparencia_destaque_desc()}>
          <ColorDots dots={dots} label={m.native_settings_accent()} />
        </SettingsRow>
        <SettingsRow title={m.native_settings_tint()} description={m.native_settings_tint_desc()}>
          <ColorDots dots={tintDots} label={m.native_settings_tint()} />
        </SettingsRow>
        {/* Sem tinta a força não muda nada: a linha fica visível e desligada. */}
        <SettingsRow
          title={m.native_settings_tint_strength()}
          description={tinta === 0 ? m.native_settings_tint_strength_off() : undefined}
          right={pct(forcaTinta)}
        >
          <Slider
            valor={forcaTinta}
            min={0.05}
            max={1}
            onChange={(v) => ap().setForcaTinta(v)}
            label={m.native_settings_tint_strength()}
            disabled={tinta === 0}
          />
        </SettingsRow>
      </SectionCard>

      <SectionCard icon="Image" title={m.native_settings_background_group()} onLayout={at('background')}>
        <View style={styles.tiles}>
          <BackgroundTiles
            options={FUNDOS}
            value={fundo}
            onChange={(v) => useAparencia.getState().setFundo(v)}
            label={m.config_fundo_curto()}
            // Sem foto guardada, "Imagem" começa pela escolha; o fundo só muda quando ela der certo.
            imageMenu={imagemUri ? undefined : origemImagem}
          />
        </View>
        {fundo === 'image' && imagemUri ? (
          <SettingsRow
            title={m.native_settings_background_effect()}
            description={processando ? m.aparencia_efeito_aplicando() : undefined}
          >
            <EffectTiles
              options={EFEITOS}
              value={efeito}
              onChange={(v) => useAparencia.getState().setEfeito(v)}
              label={m.native_settings_background_effect()}
              imageUri={imagemUri}
            />
          </SettingsRow>
        ) : null}
        <SettingsRow title={m.native_settings_image()} description={imagemUri ? undefined : m.native_settings_image_none()}>
          <View style={styles.imageLine}>
            {imagemUri ? <Image source={{ uri: imagemUri }} style={styles.thumb} contentFit="cover" /> : null}
            {origemImagem(<Pill icon="ImageUp" label={m.native_settings_image_pick()} onPress={() => {}} />)}
            {imagemUri ? (
              <Pill icon="Trash2" label={m.native_settings_image_remove()} onPress={() => void useAparencia.getState().setImagemUri(null)} />
            ) : null}
          </View>
        </SettingsRow>
        <SettingsRow title={m.config_fundo_transparencia()} description={m.config_fundo_transparencia_detalhe()}>
          {/* O valor guardado é a OPACIDADE do vidro; o slider mostra o que o rótulo diz (direita = mais transparente). */}
          <Slider
            valor={transparenciaDe(panelAlpha)}
            min={0}
            max={1}
            onChange={(v) => useAparencia.getState().setPanelAlpha(opacidadeDe(v))}
            label={m.config_fundo_transparencia()}
          />
        </SettingsRow>
        <SettingsRow title={m.config_fundo_solidez()} description={m.config_fundo_solidez_detalhe()}>
          <Slider
            valor={surfaceAlpha}
            min={0}
            max={1}
            onChange={(v) => useAparencia.getState().setSurfaceAlpha(v)}
            label={m.config_fundo_solidez()}
          />
        </SettingsRow>
      </SectionCard>

      <SectionCard
        icon="BookOpen"
        title={m.native_settings_reading_group()}
        subtitle={m.native_settings_reading_desc()}
        onLayout={at('reading')}
      >
        <View style={styles.tiles}>
          <Tiles
            options={LEITURAS}
            value={leitura}
            onChange={(v) => ap().setLeitura(v)}
            label={m.native_settings_reading()}
            art={(v) => <ReadingArt leitura={v} />}
          />
        </View>
        <SettingsRow
          title={m.native_settings_sheet_solidity()}
          description={leitura === 'sheet' ? undefined : m.native_settings_sheet_only()}
          right={pct(solidezFolha)}
        >
          <Slider
            valor={solidezFolha}
            min={0}
            max={1}
            onChange={(v) => ap().setSolidezFolha(v)}
            label={m.native_settings_sheet_solidity()}
            disabled={leitura !== 'sheet'}
          />
        </SettingsRow>
        <SettingsRow
          title={m.native_settings_contrast()}
          description={emVigor === 'text' ? undefined : m.native_settings_contrast_only()}
          right={pct(contraste)}
        >
          <Slider
            valor={contraste}
            min={0}
            max={1}
            onChange={(v) => ap().setContraste(v)}
            label={m.native_settings_contrast()}
            disabled={emVigor !== 'text'}
          />
        </SettingsRow>
      </SectionCard>

      <SectionCard icon="Type" title={m.native_settings_text_group()} onLayout={at('text')}>
        <SettingsRow title={m.native_settings_text_size()} right={pct(conversa.texto)}>
          <Slider valor={conversa.texto} min={0.5} max={1.5} onChange={(v) => ap().setConversa({ texto: v })} label={m.native_settings_text_size()} />
        </SettingsRow>
        <SettingsRow title={m.native_settings_code_size()} right={<Text style={[styles.value, { color: c.muted }]}>{`${conversa.codigo} px`}</Text>}>
          <Slider
            valor={conversa.codigo}
            min={8}
            max={24}
            // Meio pixel por passo, como o code_size do Rust.
            onChange={(v) => ap().setConversa({ codigo: Math.round(v * 2) / 2 })}
            label={m.native_settings_code_size()}
          />
        </SettingsRow>
        <SettingsRow title={m.native_settings_line_height()} right={pct(conversa.linha)}>
          <Slider valor={conversa.linha} min={0.5} max={1.5} onChange={(v) => ap().setConversa({ linha: v })} label={m.native_settings_line_height()} />
        </SettingsRow>
        {/* Acima de 100% a coluna só cresce até a largura da tela, que o celular já ocupa toda. */}
        <SettingsRow title={m.native_settings_column()} right={pct(conversa.coluna)}>
          <Slider valor={conversa.coluna} min={0.5} max={1} onChange={(v) => ap().setConversa({ coluna: v })} label={m.native_settings_column()} />
        </SettingsRow>
      </SectionCard>

      <SectionCard icon="MessageSquare" title={m.native_settings_conversation_group()} onLayout={at('conversation')}>
        <View style={styles.tiles}>
          <Text style={[styles.sublabel, { color: c.muted }]}>{m.native_settings_tool_calls()}</Text>
          <Tiles
            options={FERRAMENTAS}
            value={ferramentas}
            onChange={(v) => ap().setFerramentas(v)}
            label={m.native_settings_tool_calls()}
            art={(v) => <ToolsArt look={v} />}
          />
        </View>
        {/* Na Árvore o raciocínio já entra no grupo com todas as chamadas: a escolha fica sem efeito. */}
        <SettingsRow
          title={m.config_aparencia_pensamento_tools()}
          description={ferramentas === 'tree' ? m.native_settings_thinking_tree() : m.config_aparencia_pensamento_tools_desc()}
        >
          <Segmented
            options={PENSAMENTO}
            value={pensamento}
            onChange={(v) => useAparencia.getState().setPensamentoTools(v)}
            label={m.config_aparencia_pensamento_tools()}
            disabled={ferramentas === 'tree'}
          />
        </SettingsRow>
        <SettingsRow title={m.native_settings_task_list()}>
          <Segmented
            options={TAREFAS}
            value={tarefas ? 'on' : 'off'}
            onChange={(v) => ap().setTarefas(v === 'on')}
            label={m.native_settings_task_list()}
          />
        </SettingsRow>
        <SettingsRow title={m.native_settings_table_chart()}>
          <Segmented
            options={GRAFICO}
            value={graficoTabela ? 'on' : 'off'}
            onChange={(v) => ap().setGraficoTabela(v === 'on')}
            label={m.native_settings_table_chart()}
          />
        </SettingsRow>
        <SettingsRow title={m.native_settings_ask_highlight()} description={m.native_settings_ask_highlight_desc()}>
          <Segmented
            options={DESTAQUES}
            value={destaquePergunta}
            onChange={(v) => ap().setDestaquePergunta(v)}
            label={m.native_settings_ask_highlight()}
          />
        </SettingsRow>
        <SettingsRow title={m.aparencia_organizar_sessoes()} description={m.aparencia_organizar_dica()}>
          <Segmented
            options={ORGANIZAR}
            value={organizar}
            onChange={(v) => ap().setOrganizar(v)}
            label={m.aparencia_organizar_sessoes()}
          />
        </SettingsRow>
      </SectionCard>
    </Pagina>
  );
}

const styles = StyleSheet.create({
  tiles: { paddingHorizontal: 16, paddingBottom: 16 },
  imageLine: { flexDirection: 'row', flexWrap: 'wrap', alignItems: 'center', gap: 8 },
  thumb: { width: 48, height: 32, borderRadius: 4 },
  value: { fontSize: 13.5, fontVariant: ['tabular-nums'] },
  sublabel: { fontSize: 13.5, fontWeight: '500', paddingTop: 4, paddingBottom: 8 },
});
