import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import assert from "node:assert/strict";
import * as fs from "node:fs";

export default function (pi: ExtensionAPI) {
  pi.on("session_start", async (_event, ctx) => {
    const result: Record<string, unknown> = { run: process.env.OMP_DRIVER_RUN, ok: false };
    try {
      const prompt = ctx.getSystemPrompt().join("\n");
      result.global = prompt.includes("GLOBAL_CLAUDE_SENTINEL");
      result.project = prompt.includes("PROJECT_CLAUDE_SENTINEL");
      result.agents = prompt.includes("FORBIDDEN_AGENTS_SENTINEL");
      result.rule = prompt.includes("Verificar existência ou configuração não equivale a carregar o conteúdo");
      assert.equal(result.global, true, "Contexto global deve estar carregado");
      result.ruleCount = prompt.split("Verificar existência ou configuração não equivale a carregar o conteúdo").length - 1;
      assert.equal(result.project, process.env.EXPECT_PROJECT === "1", "Contexto do projeto deve refletir o arquivo existente");
      assert.equal(result.agents, false, "AGENTS.md não deve entrar como instrução");
      assert.equal(result.rule, true, "Regra de leitura explícita deve estar carregada");
      assert.equal(result.ruleCount, 1, "A política deve estar carregada uma única vez");
      result.ok = true;
    } catch (error) {
      result.error = String(error);
    }
    fs.writeFileSync(process.env.OMP_DRIVER_RESULT!, JSON.stringify(result));
    ctx.shutdown();
  });
}
