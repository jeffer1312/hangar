import type { On } from "claude-code";

// Mod de apoio do uiOtherMods.test.ts, no molde do mod `pm`: desenha o painel e a linha da faixa sem
// chamar `next`, então só um hook acima dele vê o desenho.
export function registerOutro(on: On): void {
  on("ui.render", { component: "Pane", requestId: "outro-painel" }, ($, e) => {
    const { Box, Button, Text } = $.ui.resolve(e);
    return (
      <Box>
        <Text>corpo do outro</Text>
        <Button key="ir" label="Ir" onPress={() => undefined} />
      </Box>
    );
  });
  on("ui.render", { component: "AbovePrompt" }, ($, e) => {
    const { Text } = $.ui.resolve(e);
    return <Text>faixa do outro</Text>;
  });
}
