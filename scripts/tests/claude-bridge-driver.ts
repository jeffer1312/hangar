import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import type * as BridgeModule from "../pi/claude-bridge";
import type * as ContextModule from "../pi/lib/agent-context";
import type { FrontmatterParser } from "../pi/lib/frontmatter";
import type { SyncResult } from "../pi/claude-bridge";

type BeforeHandler = (event: { systemPrompt: string | string[] }, ctx: { cwd: string }) => Promise<{ systemPrompt: string | string[] } | undefined>;
import assert from "node:assert/strict";
import * as fs from "node:fs";
import { homedir } from "node:os";
import * as path from "node:path";

const home = homedir();
const agentDir = path.join(home, ".omp/agent");
const claudeDir = path.join(home, ".claude");
const scenario = process.env.BRIDGE_SCENARIO;
const source = path.join(claudeDir, "agents");
const agents = path.join(agentDir, "agents");
const personal = path.join(agents, "personal.md");
const native = "---\nname: personal-agent\ndescription: agente nativo\n---\nConteúdo pessoal intacto.\n";
const write = (file: string, content: string) => {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, content);
};
const agent = (name: string, extra = "") => `---\nname: ${JSON.stringify(name)}\ndescription: 'Revisão: fixture sintética'\ntools: [Read, Glob, Task, mcp__fixture__lookup]\nmodel: opus\n${extra}---\nRevise somente o fixture.\n`;

export default async function (pi: ExtensionAPI) {
  const hooks = new Map<string, BeforeHandler>();
  let bridge: { sync(): SyncResult };
  let module: typeof BridgeModule;
  let parse: FrontmatterParser;
  let contextModule: typeof ContextModule;
  let startupError: unknown;
  try {
    write(path.join(source, "reviewer.md"), agent("fixture-reviewer"));
    write(personal, native);
    write(path.join(source, "personal.md"), agent("personal-agent"));
    write(path.join(source, "escape.md"), agent("../../escape"));
    write(path.join(claudeDir, "commands/fixture.md"), "Comando nativo, não espelhar.");
    const extraAgents = path.join(home, "extra", "agents");
    write(path.join(extraAgents, "dup.md"), agent("fixture-reviewer"));
    write(path.join(agentDir, "claude-bridge.json"), JSON.stringify({ enabled: scenario !== "disabled", skillPlugins: [], cacheCommands: [],
      agents: scenario === "discovery" ? { extraSources: [extraAgents] } : undefined }));
    if (scenario === "legacy") {
      // Instalação da ponte antiga: manifesto v1 só com nomes de prompts e a pasta de agents inteira dela.
      write(path.join(agentDir, "claude-bridge-manifest.json"), JSON.stringify({ prompts: ["fixture-cmd.md"] }));
      write(path.join(agentDir, "prompts", "fixture-cmd.md"), "---\ndescription: legado\n---\nCorpo legado.\n");
      write(path.join(agents, "claude-bridge", "user", "fixture-reviewer.md"), "---\nname: fixture-reviewer\ndescription: legado\n---\nCorpo legado.\n");
    }
    const memoryDir = path.join(claudeDir, "projects", home.replace(/[^A-Za-z0-9]/g, "-"), "memory");
    write(path.join(memoryDir, "MEMORY.md"), "Índice sintético de memória.");
    // Import tardio: a prova precisa registrar falhas de carregamento no resultado estruturado.
    module = await import("../pi/claude-bridge.ts");
    const api = new Proxy(pi, {
      get(target, key) {
        if (key === "on") return (name: string, handler: unknown) => {
          // O evento tem o mesmo contrato nos dois harnesses, exceto pelo array do prompt.
          if (name === "before_agent_start") hooks.set(name, handler as BeforeHandler);
          Reflect.apply(target.on, target, [name, handler]);
        };
        return Reflect.get(target, key);
      },
    });
    bridge = await module.default(api);
  } catch (error) {
    startupError = error;
  }

  pi.on("session_start", async (_event, ctx) => {
    const result: Record<string, unknown> = { run: process.env.OMP_DRIVER_RUN, ok: false, scenario };
    try {
      if (startupError) throw startupError;
      contextModule = await import("../pi/lib/agent-context.ts");
      const parserModule = await import("../pi/lib/frontmatter.ts");
      parse = await parserModule.loadFrontmatterParser(contextModule.getAgentContext());
      const generated = () => fs.readdirSync(agents).map(file => path.join(agents, file)).find(file => parse(fs.readFileSync(file, "utf8")).frontmatter.name === "fixture-reviewer");
      if (scenario === "discovery") {
        const task = pi.getAllTools().find(tool => tool.name === "task");
        assert.ok(task?.description.includes("fixture-reviewer"), "task deve descobrir o agent importado");
        assert.ok(!task.description.includes("../../escape"));
        assert.deepEqual(parse(fs.readFileSync(generated()!, "utf8")).frontmatter.tools, ["read", "glob", "task", "mcp__fixture__lookup"]);
        assert.equal(fs.readFileSync(personal, "utf8"), native);
        assert.equal(fs.existsSync(path.join(home, ".pi")), false);
        assert.equal(fs.existsSync(path.join(agentDir, "prompts")), false);
        assert.equal(fs.existsSync(path.join(agentDir, "skills-bridge")), false);
        assert.equal(fs.existsSync(path.join(home, "escape.md")), false);
        assert.equal(fs.readdirSync(agents).filter(file => parse(fs.readFileSync(path.join(agents, file), "utf8")).frontmatter.name === "personal-agent").length, 1);
        // Mesmo nome em duas fontes: no omp o caminho é plano, então o segundo é pulado — e reportado.
        assert.ok(bridge.sync().agents.skipped.some(entry => entry.includes("extra/dup.md") && entry.includes("já usado")));
        assert.equal(fs.readdirSync(agents).filter(file => parse(fs.readFileSync(path.join(agents, file), "utf8")).frontmatter.name === "fixture-reviewer").length, 1);
      } else if (scenario === "conversion") {
        const converted = module.convertAgent("\ufeff" + agent("fixture-reviewer", "effort: high\n").replaceAll("\n", "\r\n"), {}, "omp", parse);
        assert.ok(converted);
        const fm = parse(converted.content).frontmatter;
        assert.deepEqual(fm.tools, ["read", "glob", "task", "mcp__fixture__lookup"]);
        assert.equal(fm.model, undefined);
        assert.equal(fm.thinking, "high");
        assert.equal(fm.description, "Revisão: fixture sintética");
        const alias = module.convertAgent(agent("alias").replace("model: opus", "model: fable"), {}, "omp", parse);
        assert.ok(alias);
        assert.equal(parse(alias.content).frontmatter.model, undefined);
        assert.equal(module.convertAgent(agent("MaIn"), {}, "omp", parse), null);
        assert.equal(module.convertAgent(agent("sub"), {}, "omp", parse), null);
        for (const harness of ["omp", "pi"] as const) {
          const restricted = module.convertAgent(agent("restricted", "disallowedTools: [Task]\n"), {}, harness, parse);
          assert.ok(restricted);
          assert.deepEqual(parse(restricted.content).frontmatter.tools, harness === "omp"
            ? ["read", "glob", "mcp__fixture__lookup"] : "read, find, fixture_lookup");
          const inherited = agent("inherited", "disallowedTools: [Write]\n").replace("tools: [Read, Glob, Task, mcp__fixture__lookup]\n", "");
          assert.equal(module.convertAgent(inherited, {}, harness, parse), null);
          assert.equal(module.convertAgent(agent("denied", "disallowedTools: [Read, Glob, Task, mcp__fixture__lookup]\n"), {}, harness, parse), null);
          assert.equal(module.convertAgent(agent("pattern", "disallowedTools: ['mcp__*']\n"), {}, harness, parse), null);
        }
        const mapped = module.convertAgent(agent("mapped"), { agents: { modelMap: { opus: "fixture/model" } } }, "omp", parse);
        assert.ok(mapped);
        assert.equal(parse(mapped.content).frontmatter.model, "fixture/model");
        assert.equal(module.convertAgent(agent("denied").replace("[Read, Glob, Task, mcp__fixture__lookup]", "[UnknownTool]"), {}, "omp", parse), null);
        assert.equal(module.convertAgent(agent("empty").replace("[Read, Glob, Task, mcp__fixture__lookup]", "[]"), {}, "omp", parse), null);
        assert.equal(module.convertAgent("---\nname: [broken\n---\nbody", {}, "omp", parse), null);
        assert.equal(module.convertAgent("---\n- invalid\n---\nbody", {}, "omp", parse), null);
        assert.equal(module.convertAgent(agent("../../escape"), {}, "omp", parse), null);
        const piAgent = module.convertAgent(agent("pi-agent"), {}, "pi", parse);
        assert.ok(piAgent);
        assert.equal(parse(piAgent.content).frontmatter.tools, "read, find, subagent, fixture_lookup");
        const command = module.convertCommand("---\ndescription: 'Revisão: prompt'\n---\nUse $ARGUMENTS e $1.", "fixture.md", {}, parse);
        assert.ok(command);
        assert.equal(parse(command.content).body.trim(), "Use $ARGUMENTS e $1.");
      } else if (scenario === "ownership") {
        const original = fs.readFileSync(generated()!, "utf8");
        write(path.join(source, "gone.md"), agent("gone-agent"));
        bridge.sync();
        const gone = path.join(agents, fs.readdirSync(agents).find(file => parse(fs.readFileSync(path.join(agents, file), "utf8")).frontmatter.name === "gone-agent")!);
        // Fonte ilegível pula só ela e suspende remoções: a cópia de uma fonte que sumiu fica até
        // todas voltarem a ser lidas — a ponte não sabe qual cópia a fonte quebrada geraria.
        write(path.join(source, "reviewer.md"), "---\nname: [broken\n---\nbody");
        fs.unlinkSync(path.join(source, "gone.md"));
        const partial = bridge.sync();
        assert.ok(partial.agents.skipped.some(entry => entry.includes("reviewer.md") && entry.includes("frontmatter inválida")));
        assert.equal(fs.readFileSync(generated()!, "utf8"), original);
        assert.equal(fs.existsSync(gone), true);
        write(path.join(source, "reviewer.md"), agent("fixture-reviewer"));
        bridge.sync();
        assert.equal(fs.existsSync(gone), false);
        const target = generated()!;
        assert.ok(target);
        const changed = fs.readFileSync(target, "utf8") + "Edição manual.\n";
        write(target, changed);
        write(path.join(source, "reviewer.md"), agent("fixture-reviewer") + "Fonte atualizada.\n");
        const conflict = bridge.sync();
        assert.ok(conflict.agents.skipped.length > 0, "conflito precisa ser reportado");
        assert.equal(fs.readFileSync(target, "utf8"), changed);
        write(path.join(source, "removed.md"), agent("removed-agent"));
        bridge.sync();
        const removed = fs.readdirSync(agents).find(file => parse(fs.readFileSync(path.join(agents, file), "utf8")).frontmatter.name === "removed-agent")!;
        fs.unlinkSync(path.join(source, "removed.md"));
        fs.unlinkSync(path.join(source, "reviewer.md"));
        bridge.sync();
        assert.equal(fs.existsSync(path.join(agents, removed)), false);
        assert.equal(fs.readFileSync(target, "utf8"), changed);
        assert.equal(fs.readFileSync(personal, "utf8"), native);
      } else if (scenario === "memory" || scenario === "disabled") {
        const before = hooks.get("before_agent_start")!;
        const original = ["bloco-a", "bloco-b"];
        const first = await before({ systemPrompt: original }, { cwd: home });
        if (scenario === "disabled") {
          assert.equal(first, undefined);
          assert.equal(generated(), undefined);
        } else {
          const memoryBlock = `\n\n# Memórias do projeto (Claude, índice — corpo em ${path.join(claudeDir, "projects", home.replace(/[^A-Za-z0-9]/g, "-"), "memory")}/)\nÍndice sintético de memória.`;
          assert.ok(first);
          assert.deepEqual(first.systemPrompt, ["bloco-a", "bloco-b", memoryBlock]);
          assert.deepEqual(original, ["bloco-a", "bloco-b"]);
          const second = await before({ systemPrompt: first.systemPrompt }, { cwd: home });
          assert.equal(second, undefined);
          assert.equal(await before({ systemPrompt: ["prefixo" + memoryBlock] }, { cwd: home }), undefined);
          const text = await before({ systemPrompt: "prompt-original" }, { cwd: home });
          assert.ok(text);
          assert.equal(text.systemPrompt, "prompt-original" + memoryBlock);
          // Configuração ilegível não derruba o turno: o hook desiste da memória e segue.
          write(path.join(agentDir, "claude-bridge.json"), "{quebrado");
          assert.equal(await before({ systemPrompt: "prompt-original" }, { cwd: home }), undefined);
        }
      } else if (scenario === "legacy") {
        const manifest = JSON.parse(fs.readFileSync(path.join(agentDir, "claude-bridge-manifest.json"), "utf8"));
        assert.equal(manifest.version, 2);
        const target = generated()!;
        assert.ok(target, "agent legado precisa ter sido regravado no layout novo");
        assert.ok(manifest.files[path.relative(agentDir, target)]);
        assert.equal(fs.existsSync(path.join(agents, "claude-bridge", "user", "fixture-reviewer.md")), false);
        assert.equal(fs.existsSync(path.join(agentDir, "prompts", "fixture-cmd.md")), false);
        assert.equal(Object.keys(manifest.files).some(key => key.startsWith("agents/claude-bridge/") || key.startsWith("prompts/")), false);
        assert.equal(fs.readFileSync(personal, "utf8"), native);
      } else if (scenario === "context") {
        assert.deepEqual(contextModule.getAgentContext(), { harness: "omp", agentDir, claudeDir });
        process.env.PI_CODING_AGENT_DIR = "~/custom/../agent";
        process.env.CLAUDE_CONFIG_DIR = "~/claude-custom";
        assert.deepEqual(contextModule.getAgentContext(), { harness: "omp", agentDir: path.join(home, "agent"), claudeDir: path.join(home, "claude-custom") });
      } else {
        throw new Error(`Cenário desconhecido: ${scenario}`);
      }
      result.ok = true;
    } catch (error) {
      result.error = error instanceof Error ? error.stack : String(error);
    }
    fs.writeFileSync(process.env.OMP_DRIVER_RESULT!, JSON.stringify(result));
    ctx.shutdown();
  });
}
