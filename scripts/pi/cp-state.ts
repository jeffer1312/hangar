// scripts/pi/cp-state.ts
// Publica o estado da sessao Pi no MESMO marcador que o hook do Claude escreve
// (<config>/.claude-pocket-state/<session_id>.json), entao o HookState do backend le os dois sem
// saber a diferenca. Escrita atomica (tmp + rename) pelo mesmo motivo do state_hook.py: o watcher
// pode ler no meio da escrita e um JSON pela metade viraria marcador ignorado.
//
// `agent_settled` e nao `agent_end`: o `agent_end` dispara antes de o Pi poder auto-retry ou
// auto-compactar e seguir rodando, entao marcar idle nele deixaria a sessao "ociosa" enquanto ela
// ainda trabalha.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const base = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), ".claude");
const dir = path.join(base, ".claude-pocket-state");
const paneDir = path.join(base, ".claude-pocket-pi");

// O arquivo da sessao vem do ctx do handler, NAO do ambiente. Medido na Task 0 (fato 6): o processo
// do pi nao tem NENHUMA var PI_* no /proc/<pid>/environ e `process.env.PI_SESSION_FILE` e undefined
// dentro da extensao. Quem sabe e `ctx.sessionManager.getSessionFile()`
// (ReadonlySessionManager, dist/core/session-manager.d.ts:140).
function sessionFile(ctx: any): string | null {
  return ctx?.sessionManager?.getSessionFile?.() ?? null;   // --no-session: nada pra rastrear
}

// A CHAVE do marcador de estado e o stem do arquivo de sessao, nao o session-id: o backend procura
// por `Path(jsonl).stem` (sse.py:305, registry.py:604). No Claude os dois coincidem (<uuid>.jsonl);
// no Pi o arquivo e <ts>_<uuid>.jsonl, entao gravar por session-id cria um marcador que ninguem le
// — e o sintoma seria "sessao Pi sempre ociosa", sem erro em lugar nenhum.
function writeAtomic(target: string, data: unknown): void {
  try {
    fs.mkdirSync(path.dirname(target), { recursive: true });
    const tmp = `${target}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(data));
    fs.renameSync(tmp, target);   // atomico: o watcher pode ler no meio da escrita
  } catch {
    // Marcador e otimizacao, nao correcao: falhar aqui nunca pode derrubar o turno do usuario.
  }
}

function publishState(state: "working" | "idle", ctx: any): void {
  const file = sessionFile(ctx);
  if (!file) return;
  writeAtomic(path.join(dir, `${path.basename(file, ".jsonl")}.json`),
              { state, ts: Date.now() / 1000 });
}

// Bilhete pane -> arquivo de sessao. E a UNICA forma de o backend ligar um pane Pi ao transcript
// dele: o pi reescreve o proprio argv (Task 0, fato 7), entao o `--session-id` some do
// /proc/<pid>/cmdline e nao ha o que casar. `TMUX_PANE` (ex `%123`) o pi HERDA — medido.
function publishPane(ctx: any): void {
  const pane = process.env.TMUX_PANE;
  const file = sessionFile(ctx);
  if (!pane || !file) return;      // fora do tmux nao ha o que ligar
  writeAtomic(path.join(paneDir, `${pane.replace("%", "")}.json`),
              { file, id: ctx?.sessionManager?.getSessionId?.() ?? null, ts: Date.now() / 1000 });
}

export default function (pi: ExtensionAPI) {
  // Handlers recebem (event, ctx) — types.d.ts:845. Sem o ctx nao ha arquivo de sessao.
  pi.on("session_start", async (_e: any, ctx: any) => { publishPane(ctx); });
  pi.on("agent_start", async (_e: any, ctx: any) => { publishPane(ctx); publishState("working", ctx); });
  pi.on("agent_settled", async (_e: any, ctx: any) => { publishState("idle", ctx); });
}

// publishPane roda tambem no agent_start de proposito: /tree, /fork e troca de sessao mudam o
// arquivo com a sessao ja rodando, e o session_start sozinho deixaria o bilhete apontando pro
// arquivo velho.
