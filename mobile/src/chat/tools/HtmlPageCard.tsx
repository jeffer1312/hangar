import { memo, useCallback, useState } from 'react';
import { Linking, Pressable, Text, View, useWindowDimensions, type LayoutChangeEvent } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { WebView, type WebViewMessageEvent } from 'react-native-webview';
import { useRouter } from 'expo-router';
import { frameHeight, pageExpandable, parseHostMessage, reservedHeight, type HtmlPageRef } from '@hangar/core';
import { useAparencia } from '../../stores/aparencia';
import { hasImageBackground } from '../../theme/aplicarMaterial';
import { toast } from '../../ui/Toast';
import { Icon } from '../../ui/Icon';
import * as m from '../../paraglide/messages';
import { onlyInitial, rememberReported, usePageDocument, usePageWebView } from './pageDocument';
import { pageScreenHref } from './pageRoute';

// Página publicada pelo html_render no lugar da chamada, como o cartão do nativo (page_card.rs):
// sem moldura nem título (sobre a foto de fundo, num fundo liso); carregando ocupa a altura reservada,
// erro e expirada viram uma linha de texto apagado. A mensagem da página é do agente, não confiável:
// só altura finita e link http(s) passam.
export const HtmlPageCard = memo(function HtmlPageCard(props: { page: HtmlPageRef; sessionName: string; serverId?: string }) {
  return props.page.url ? <SiteCard title={props.page.title} url={props.page.url} /> : <LivePageCard {...props} />;
});

// Site de verdade só vive no app desktop; aqui o cartão só abre o endereço.
function SiteCard({ title, url }: { title: string; url: string }) {
  const open = () => { Linking.openURL(url).catch((err: unknown) => toast.erro(err instanceof Error ? err.message : String(err))); };
  return (
    <View style={styles.site}>
      <Text style={styles.siteTitle} selectable>{title}</Text>
      <Text style={styles.note} selectable numberOfLines={2}>{url}</Text>
      <Pressable onPress={open} style={styles.retry} accessibilityRole="link" hitSlop={8}>
        <Text style={styles.retryText}>{m.page_open_browser()}</Text>
      </Pressable>
    </View>
  );
}

function LivePageCard({ page, sessionName, serverId }: { page: HtmlPageRef; sessionName: string; serverId?: string }) {
  const { theme } = useUnistyles();
  const router = useRouter();
  const { width: screen } = useWindowDimensions();
  const [width, setWidth] = useState(0);
  const { key, state, doc, reported, setReported, load, params } = usePageDocument(page, sessionName, serverId);
  const { web, applyTheme, touched, openLink } = usePageWebView(params, page.ownTheme);
  // Sobre a foto de fundo a página transparente perde a letra na imagem: ganha um fundo liso, como no nativo.
  const backed = useAparencia(hasImageBackground);

  const onMessage = useCallback((e: WebViewMessageEvent) => {
    const msg = parseHostMessage(e.nativeEvent.data);
    if (msg?.kind === 'height') {
      const next = Math.round(msg.height);
      if (rememberReported(key, next)) setReported((prev) => (prev === next ? prev : next));
    } else if (msg?.kind === 'link') {
      openLink(msg.url);
    }
  }, [key, setReported, openLink]);

  const onLayout = useCallback((e: LayoutChangeEvent) => setWidth(e.nativeEvent.layout.width), []);
  const w = width || screen - 2 * theme.base.space[4];
  const height = frameHeight(page, w, reported);
  // Altura fixa pedida pelo agente e conteúdo maior que ela: a página rola por dentro.
  const inside = page.height != null && (reported ?? reservedHeight(page, w)) > height;
  // Conversa arquivada não tem servidor da sessão: sem tela cheia.
  const expandable = serverId != null && pageExpandable(page, state);

  if (state === 'expired') return <Text style={styles.note} selectable>{m.page_expired()}</Text>;
  if (state === 'error') {
    return (
      <View style={styles.errorRow} accessibilityRole="alert">
        <Text style={[styles.note, styles.errorText]} selectable>{m.page_error({ title: page.title })}</Text>
        <Pressable onPress={() => void load()} style={styles.retry} accessibilityRole="button" hitSlop={8}>
          <Text style={styles.retryText}>{m.page_retry()}</Text>
        </Pressable>
      </View>
    );
  }
  return (
    <View style={backed ? styles.backdrop : undefined}>
      <View style={[styles.page, { height }]} onLayout={onLayout} accessibilityLabel={page.title}
        onTouchStart={touched}>
        {state === 'loading' ? (
          <View style={styles.loading} accessibilityRole="progressbar">
            <Text style={styles.note}>{m.page_loading({ title: page.title })}</Text>
          </View>
        ) : (
          <WebView
            ref={web}
            source={{ html: doc }}
            style={styles.web}
            // Com '*' toda navegação passa pelo filtro abaixo; com uma lista menor, o que ficasse de fora
            // a própria lib abriria no Linking sem gesto, em qualquer esquema.
            originWhitelist={['*']}
            onShouldStartLoadWithRequest={onlyInitial}
            // Sem janela nova: o window.open vira navegação no próprio quadro e o filtro barra.
            setSupportMultipleWindows={false}
            onMessage={onMessage}
            onLoadEnd={applyTheme}
            scrollEnabled={inside}
            nestedScrollEnabled={inside}
            bounces={false}
            overScrollMode="never"
            showsVerticalScrollIndicator={false}
            showsHorizontalScrollIndicator={false}
            automaticallyAdjustContentInsets={false}
          />
        )}
        {expandable && serverId ? (
          <Pressable style={styles.expand} hitSlop={8} accessibilityRole="button" accessibilityLabel={m.page_fullscreen()}
            onPress={() => router.push(pageScreenHref(serverId, sessionName, page) as never)}>
            <Icon name="Maximize2" size={14} color={theme.tokens.text.secondary} />
          </Pressable>
        ) : null}
      </View>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  page: { width: '100%', borderRadius: theme.base.radius.sm, borderCurve: 'continuous', overflow: 'hidden', backgroundColor: 'transparent' },
  web: { flex: 1, backgroundColor: 'transparent' },
  backdrop: { padding: theme.base.space[3], borderRadius: theme.base.radius.md, borderCurve: 'continuous', backgroundColor: theme.tokens.bg.base },
  loading: { flex: 1, justifyContent: 'center' },
  note: { paddingVertical: theme.base.space[3], fontSize: theme.base.text.xs, color: theme.tokens.text.muted },
  errorRow: { flexDirection: 'row', alignItems: 'center', gap: theme.base.space[2] },
  errorText: { flexShrink: 1 },
  retry: { minHeight: 44, justifyContent: 'center', paddingHorizontal: theme.base.space[1] },
  site: { alignItems: 'flex-start', paddingTop: theme.base.space[2] },
  siteTitle: { fontSize: theme.base.text.sm, fontWeight: theme.base.weight.semibold, color: theme.tokens.text.primary },
  retryText: { fontSize: theme.base.text.xs, fontWeight: theme.base.weight.semibold, color: theme.tokens.accent.base },
  // Como o T3 no celular: botão redondo no canto, sempre à vista.
  expand: {
    position: 'absolute', top: 6, right: 6, width: 32, height: 32, borderRadius: theme.base.radius.full, alignItems: 'center',
    justifyContent: 'center', backgroundColor: theme.tokens.bg.surface, borderWidth: 1, borderColor: theme.tokens.border.subtle,
  },
}));
