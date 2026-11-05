import { memo, useCallback, useEffect, useMemo, useState } from 'react';
import { Linking, Platform, Pressable, View, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import type { UnistylesThemes } from 'react-native-unistyles';
import { ImageThumb, type MediaItem } from './ImageThumb';
import { AudioChip } from './AudioChip';
import { DocumentViewer, type DocumentItem } from '../features/attachments/DocumentViewer';
import { FileCard } from './FileCard';
import { toast } from '../ui/Toast';
import { EnrichedMarkdownText } from 'react-native-enriched-markdown';
import type { MarkdownStyle } from 'react-native-enriched-markdown';
import { useRouter } from 'expo-router';
import { codeCitation, fileUrlNative, fileAuthHeader, hideAttachmentPaths, lerTabelaMarkdown, linkCodeReferences, parseCodePaths, parseFileLink, parseFilePaths, parseMediaUrls, proposedPlan, planDisplayText, splitMermaidBlocks, buildQuoteRef, quoteAffixesInMarkdown, quoteTooLong } from '@hangar/core';
import type { TextContextMenuItem, TextSelectionMenuConfig } from 'react-native-enriched-markdown';
import { MermaidBlock } from './MermaidBlock';
import { contextRefsKey } from '../stores/contextRefs';
import { openCiteDialog } from '../stores/citeDialog';
import { useQuoteFlash } from '../stores/quoteFlash';
import * as m from '../paraglide/messages';
import { TableChart } from './TableChart';
import { ArquivoChip } from './ArquivoChip';
import { BubbleActions } from './BubbleActions';
import { getTableChartPref, setTableChartPref } from './tableChartPref';
import { superficie } from '../theme/superficie';
import { Icon, type IconName } from '../ui/Icon';
import { useAparencia } from '../stores/aparencia';
import { useServers } from '../stores/servers';

// Tema completo do unistyles (tokens + base) — UnistylesTheme não é exportado na raiz.
type TemaApp = UnistylesThemes[keyof UnistylesThemes];

// Entrelinha de fábrica de cada bloco da lib (normalizeMarkdownStyle): a escala da Aparência anda
// em cima dela, para 100% continuar igual ao que era.
const ios = Platform.OS === 'ios';
const LH = {
  p: ios ? 24 : 26, h1: ios ? 36 : 38, h2: ios ? 30 : 32, h3: ios ? 26 : 28, h4: ios ? 24 : 26,
  h5: ios ? 22 : 24, h6: ios ? 20 : 22, quote: ios ? 24 : 26, list: ios ? 22 : 26, code: ios ? 20 : 22,
};
const CODIGO_FABRICA = 14;

// Tema do markdown mapeado dos tokens do app (cores/links/code) — a lib recebe um
// MarkdownStyle plano; sem ele usa defaults pretos que somem no tema escuro.
// Tamanho, entrelinha e código vêm de Aparência › Texto da conversa (`theme.conversa`).
export function mkMarkdownStyle(t: TemaApp): MarkdownStyle {
  const { texto, linha, codigo } = t.conversa;
  const bloco = (size: number, lh: number) => ({ fontSize: size * texto, lineHeight: Math.round(lh * texto * linha) });
  return {
    paragraph: { color: t.tokens.text.primary, ...bloco(t.base.text.base, LH.p) },
    h1: { color: t.tokens.text.primary, ...bloco(t.base.text.xl, LH.h1), fontWeight: t.base.weight.bold },
    h2: { color: t.tokens.text.primary, ...bloco(t.base.text.lg, LH.h2), fontWeight: t.base.weight.bold },
    h3: { color: t.tokens.text.primary, ...bloco(t.base.text.base, LH.h3), fontWeight: t.base.weight.semibold },
    h4: { color: t.tokens.text.primary, ...bloco(t.base.text.base, LH.h4), fontWeight: t.base.weight.semibold },
    h5: { color: t.tokens.text.primary, ...bloco(t.base.text.sm, LH.h5), fontWeight: t.base.weight.semibold },
    h6: { color: t.tokens.text.secondary, ...bloco(t.base.text.sm, LH.h6), fontWeight: t.base.weight.semibold },
    link: { color: t.tokens.accent.base, underline: true },
    strong: { color: t.tokens.text.primary },
    em: { color: t.tokens.text.primary },
    // Código inline é pílula na cor de destaque, sem contorno: sem `borderColor` a lib desenha a
    // borda rosa de fábrica, que corta a frase em caixinhas.
    code: {
      fontFamily: t.base.fontMono,
      fontSize: codigo,
      color: t.tokens.accent.base,
      backgroundColor: t.tokens.accent.dim,
      borderColor: 'transparent',
    },
    codeBlock: {
      fontFamily: t.base.fontMono,
      fontSize: codigo,
      lineHeight: Math.round((LH.code * codigo) / CODIGO_FABRICA),
      color: t.tokens.text.primary,
      backgroundColor: superficie(t),
      borderColor: t.tokens.border.subtle,
      borderWidth: 1,
      borderRadius: t.base.radius.sm,
      padding: t.base.space[2],
      syntaxColors: {
        keyword: t.tokens.accent.base,
        string: t.tokens.status.success,
        number: t.tokens.status.warning,
        constant: t.tokens.status.warning,
        function: t.tokens.accent.base,
        type: t.tokens.accent.base,
        property: t.tokens.text.secondary,
        tag: t.tokens.accent.base,
        attribute: t.tokens.text.secondary,
        comment: t.tokens.text.muted,
      },
    },
    blockquote: { borderColor: t.tokens.border.default, backgroundColor: 'transparent', color: t.tokens.text.secondary, ...bloco(t.base.text.base, LH.quote) },
    list: { bulletColor: t.tokens.text.muted, color: t.tokens.text.primary, ...bloco(t.base.text.base, LH.list) },
    table: {
      headerBackgroundColor: superficie(t, 0.8),
      headerTextColor: t.tokens.text.primary,
      // Sem estas duas a biblioteca pinta as linhas de branco, e o texto claro do tema some nelas.
      rowEvenBackgroundColor: 'transparent',
      rowOddBackgroundColor: superficie(t, 0.3),
      borderColor: t.tokens.border.subtle,
      borderWidth: 1,
      color: t.tokens.text.primary,
    },
  };
}

// Markdown secundário (prévia em voo, trecho citado): o mesmo estilo com o parágrafo esmaecido.
export function mkMutedMarkdownStyle(t: TemaApp): MarkdownStyle {
  const base = mkMarkdownStyle(t);
  return { ...base, paragraph: { ...base.paragraph, color: t.tokens.text.secondary } };
}

export const AssistantBubble = memo(function AssistantBubble({
  text,
  sessionName,
  serverId,
  ts,
  eventId,
  canQuote = false,
}: {
  text: string;
  sessionName?: string;
  serverId?: string;
  ts?: number | null;
  /** `ChatEvent.id` da resposta: com ele, e `canQuote`, o menu da seleção oferece "Citar". */
  eventId?: string;
  /** A tela tem campo de mensagem para enviar a citação. */
  canQuote?: boolean;
}) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const server = useServers((s) => s.servers.find((x) => x.id === serverId) ?? null);
  const paths = useMemo(() => parseFilePaths(text), [text]);
  const proposed = useMemo(() => proposedPlan(text), [text]);
  // O caminho que vira prévia logo abaixo some do texto (fica no transcript, que é o que autoriza o arquivo).
  const drawsLocal = !!sessionName && !!server;
  const display = useMemo(() => {
    const d = planDisplayText(text);
    return drawsLocal ? hideAttachmentPaths(d, paths) : d;
  }, [text, drawsLocal, paths]);
  const md = useMemo(() => mkMarkdownStyle(theme), [theme]);
  // Anexos: mídia/pdf/html citados por caminho (precisam da sessão) + mídia remota por URL, que o
  // web desenha igual. Imagem e vídeo da mensagem formam UMA galeria no visor.
  const atts = useMemo(() => {
    const local = sessionName && server
      ? paths.map((r) => ({ ...r, uri: fileUrlNative(sessionName, r.path, server), headers: fileAuthHeader(server) as Record<string, string> | undefined }))
      : [];
    const remota = parseMediaUrls(text).map((r) => ({ ...r, uri: r.url!, headers: undefined }));
    return [...local, ...remota];
  }, [text, sessionName, server, paths]);
  const galeria = useMemo<MediaItem[]>(
    () => atts.flatMap((a) => (a.kind === 'image' || a.kind === 'video' ? [{ uri: a.uri, headers: a.headers, name: a.name, kind: a.kind }] : [])),
    [atts],
  );
  const [doc, setDoc] = useState<DocumentItem | null>(null);
  const abrirArquivo = useCallback(
    (path: string) => router.push(`/s/${serverId}/${sessionName}/files?path=${encodeURIComponent(path)}` as never),
    [router, serverId, sessionName],
  );
  const podeAbrirArquivo = !!sessionName && !!serverId;
  // Citação `caminho:linha` vira link no markdown (a lib só devolve toque em link); o toque abre a
  // aba Arquivos. URL de verdade sai pro sistema.
  // A biblioteca não intercepta bloco por linguagem: a resposta é partida nos ```mermaid e cada
  // diagrama vira imagem entre dois trechos de texto. Esta bolha já é a resposta pronta (a do
  // streaming é a PreviewBubble), então nenhum diagrama é pedido pela metade.
  const parts = useMemo(() => splitMermaidBlocks(display).map((p) => ('markdown' in p
    ? { markdown: p.markdown, shown: podeAbrirArquivo ? linkCodeReferences(p.markdown) : p.markdown }
    : p)), [display, podeAbrirArquivo]);
  // "Citar" no menu da seleção: abre o modal de resposta ao trecho; a bandeja só recebe ao confirmar.
  // A seleção é do texto desenhado (sem `**`); prefixo e sufixo saem da fonte, achada sem os marcadores.
  const quotable = !!eventId && canQuote;
  const cite = useCallback((selected: string, source: string, near?: number) => {
    if (!eventId) return;
    if (quoteTooLong(selected)) {
      toast.erro(m.cite_too_long_aria());
      return;
    }
    const ref = buildQuoteRef({ source: eventId, text: selected, ...quoteAffixesInMarkdown(source, selected, near) });
    if (ref) openCiteDialog(contextRefsKey(serverId, sessionName), ref);
  }, [eventId, serverId, sessionName]);
  const menus = useMemo(() => parts.map((p): TextContextMenuItem[] | undefined => (quotable && 'markdown' in p
    ? [{ text: m.cite_action(), icon: 'quote.opening', onPress: ({ text: sel, selection }) => cite(sel, p.markdown, selection?.start) }]
    : undefined)), [parts, quotable, cite]);
  // Os itens da biblioteca no menu da seleção ("Copy", "Copy as Markdown") no idioma da tela.
  const selectionMenu = useMemo<TextSelectionMenuConfig>(() => ({
    copy: { label: m.selection_copy() },
    copyAsMarkdown: { label: m.selection_copy_markdown() },
    copyImageUrl: {
      label: m.selection_copy_image_url(),
      pluralLabels: { other: m.selection_copy_image_urls({ count: '{count}' }) },
    },
  }), []);
  const flashing = useQuoteFlash((st) => !!eventId && st.eventId === eventId);
  const onLinkPress = useCallback(({ url }: { url: string }) => {
    const arquivo = parseFileLink(url);
    // ponytail: a aba Arquivos ainda não rola até a linha; o arquivo abre no topo.
    if (arquivo) {
      if (podeAbrirArquivo) abrirArquivo(arquivo.path);
      return;
    }
    // O link vem do texto do modelo: esquema de app (tel:, intent:, deep link) não abre por toque.
    if (!/^(https?|mailto):/i.test(url)) return;
    Linking.openURL(url).catch((e: unknown) => toast.erro(e instanceof Error ? e.message : String(e)));
  }, [abrirArquivo, podeAbrirArquivo]);
  // Arquivo de CÓDIGO citado na prosa (o parseFilePaths acima só pega mídia/pdf/html, que viram
  // miniatura). Só vira chip quando dá pra abrir na aba Arquivos. O `codeCitation` tira a mídia daqui:
  // com extensão aberta no absoluto o `parseCodePaths` casa `.png` e o arquivo saía nos dois.
  const codigos = useMemo(() => parseCodePaths(text).filter((p) => codeCitation(p)), [text]);
  const hasCodigos = codigos.length > 0 && !!sessionName && !!serverId;
  const tabelas = useMemo(() => lerTabelaMarkdown(display), [display]);
  const [pref, setPref] = useState(() => getTableChartPref());
  // Aparência › Gráfico nas tabelas: desligado, nem o botão aparece.
  const grafico = useAparencia((s) => s.graficoTabela);
  const [colIndices, setColIndices] = useState<number[]>(() => tabelas.map(() => 0));
  useEffect(() => {
    if (colIndices.length !== tabelas.length) setColIndices(tabelas.map(() => 0));
  }, [tabelas.length, colIndices.length]);

  return (
    <View style={[styles.wrap, flashing && { backgroundColor: theme.tokens.accent.dim, borderRadius: theme.base.radius.md }]}>
      {proposed ? <Text style={styles.planLabel}>{m.chat_plan_proposto()}</Text> : null}
      {parts.map((p, i) => ('mermaid' in p
        ? <MermaidBlock key={i} source={p.mermaid} serverId={serverId} md={md} />
        : <EnrichedMarkdownText key={i} markdown={p.shown} markdownStyle={md} flavor="github" onLinkPress={onLinkPress}
                                contextMenuItems={menus[i]} selectionMenuConfig={selectionMenu} />))}
      {grafico && tabelas.length > 0 ? (
        <View style={styles.tableBlock}>
          <Pressable
            onPress={() => {
              const next = pref === 'chart' ? 'table' : 'chart';
              setPref(next);
              setTableChartPref(next);
            }}
            style={[styles.chartBtn, { borderColor: theme.tokens.border.subtle, backgroundColor: superficie(theme) }]}
            accessibilityRole="button"
          >
            <Text style={[styles.chartBtnText, { color: theme.tokens.accent.base }]}>
              {pref === 'chart' ? m.tabela_botao_tabela() : m.tabela_botao_grafico()}
            </Text>
          </Pressable>
          {pref === 'chart'
            ? tabelas.map((t, idx) => (
                <TableChart
                  key={idx}
                  tabela={t}
                  coluna={colIndices[idx] ?? 0}
                  onColuna={(c) =>
                    setColIndices((prev) => {
                      const copy = [...prev];
                      copy[idx] = c;
                      return copy;
                    })
                  }
                />
              ))
            : null}
        </View>
      ) : null}
      {sessionName && paths.length > 0 && !server ? <Text style={{ color: theme.tokens.status.error }} accessibilityRole="alert">{m.chat_servidor_removido()}</Text> : null}
      {atts.length > 0 ? (
        <View style={styles.atts}>
          {atts.map((r) => {
            if (r.kind === 'image' || r.kind === 'video') {
              const i = galeria.findIndex((g) => g.uri === r.uri);
              return <ImageThumb key={r.path} items={galeria} index={i} style={styles.thumb} revealTs={ts ?? null} />;
            }
            if (r.kind === 'audio') return <AudioChip key={r.path} uri={r.uri} headers={r.headers} name={r.name} />;
            const kind = r.kind;
            if (kind === 'document') return <FileCard key={r.path} uri={r.uri} headers={r.headers} name={r.name} />;
            const icon: IconName = kind === 'pdf' ? 'FileText' : 'Globe';
            return (
              <Pressable
                key={r.path}
                onPress={() => setDoc({ uri: r.uri, headers: r.headers, name: r.name, kind })}
                style={[styles.chip, { backgroundColor: superficie(theme), borderColor: theme.tokens.border.subtle }]}
                accessibilityRole="button"
                accessibilityLabel={m.anexos_visualizar({ nome: r.name })}
              >
                <Icon name={icon} size={14} color={theme.tokens.text.secondary} />
                <Text style={[styles.chipName, { color: theme.tokens.text.primary }]} numberOfLines={1}>
                  {r.name}
                </Text>
                <Icon name="ChevronRight" size={14} color={theme.tokens.text.muted} />
              </Pressable>
            );
          })}
        </View>
      ) : null}
      <DocumentViewer doc={doc?.headers && !server ? null : doc} onClose={() => setDoc(null)} />
      {hasCodigos ? (
        <View style={styles.atts}>
          {codigos.map((p) => (
            <ArquivoChip
              key={p}
              caminho={p}
              onPress={() => abrirArquivo(p)}
            />
          ))}
        </View>
      ) : null}
      <View style={styles.meta}><BubbleActions text={text} ts={ts} serverId={serverId} /></View>
    </View>
  );
});

const styles = StyleSheet.create((theme) => ({
  planLabel: {
    color: theme.tokens.accent.base,
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
  },
  // Sem bolha, como no nativo: texto corrido na largura toda. Só tabela, código e anexo têm caixa,
  // e essa caixa vem da superficie() para seguir Transparência e Solidez.
  wrap: {
    alignSelf: 'stretch',
    gap: theme.base.space[2],
  },
  // Hora e ⋯ pertencem ao texto de cima: colados nele, longe da próxima linha da conversa.
  meta: { marginTop: -theme.base.space[1.5] },
  atts: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    gap: theme.base.space[2],
    marginTop: theme.base.space[1],
  },
  thumb: {
    width: 96,
    height: 96,
    borderRadius: theme.base.radius.md,
    backgroundColor: superficie(theme),
  },
  chip: {
    flexDirection: 'row',
    alignItems: 'center',
    gap: theme.base.space[1],
    paddingHorizontal: theme.base.space[2],
    paddingVertical: theme.base.space[1],
    borderRadius: theme.base.radius.md,
    borderWidth: 1,
    maxWidth: 220,
  },
  chipName: {
    fontSize: theme.base.text.xs,
    flexShrink: 1,
  },
  tableBlock: {
    gap: theme.base.space[2],
    marginTop: theme.base.space[1],
  },
  chartBtn: {
    alignSelf: 'flex-start',
    paddingHorizontal: theme.base.space[3],
    paddingVertical: theme.base.space[1],
    borderRadius: theme.base.radius.full,
    borderWidth: 1,
  },
  chartBtnText: {
    fontSize: theme.base.text.sm,
    fontWeight: theme.base.weight.semibold,
  },
}));
