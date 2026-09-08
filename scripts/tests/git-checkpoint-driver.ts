import type { ExtensionAPI, ExtensionContext, ExtensionCommandContext, SessionManager } from "@earendil-works/pi-coding-agent";
import type { AgentContext } from "../pi/lib/agent-context";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import * as fs from "node:fs";
import * as path from "node:path";
import { homedir } from "node:os";
import { setTimeout as delay } from "node:timers/promises";

interface StoredCheckpoint {
  version?: number;
  ref: string;
  shadowDir: string;
  workTree: string;
  prompt: string;
  createdAt: string;
}
type Handler = (event: Record<string, unknown>, ctx: ExtensionContext) => unknown;

export default async function (pi: ExtensionAPI) {
  const startupSdk = process.env.CHECKPOINT_SCENARIO === "capture_deadline"
    ? await import("@earendil-works/pi-coding-agent") : undefined;
  // O driver externo precisa sobreviver à régua de 30 s do dispatcher que ele mede.
  startupSdk?.testSetExtensionHandlerTimeoutMs(40_000);
  pi.on("session_start", async (_event, runtimeCtx) => {
    const result: Record<string, unknown> = { run: process.env.OMP_DRIVER_RUN, scenario: process.env.CHECKPOINT_SCENARIO, ok: false };
    try {
      assert.ok(process.env.OMP_DRIVER_RESULT && process.env.OMP_DRIVER_RUN, "driver exige HOME isolada pelo helper");
      const root = fs.realpathSync(homedir());
      assert.ok(fs.realpathSync(path.dirname(process.env.OMP_DRIVER_RESULT!)).startsWith(root + path.sep));
      const cleanEnv = Object.fromEntries(Object.entries(process.env).filter(([name]) => !name.startsWith("GIT_")));
      const git = (args: string[], cwd = root) => execFileSync("/usr/bin/git", args, { cwd, env: cleanEnv, encoding: "utf8" }).trim();
      const write = (file: string, text: string) => { fs.mkdirSync(path.dirname(file), { recursive: true }); fs.writeFileSync(file, text); };
      const checksum = (file: string) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
      git(["init", "-q", "-b", "fixture"]);
      git(["config", "user.name", "Fixture"]);
      git(["config", "user.email", "fixture@example.invalid"]);
      write(path.join(root, ".gitignore"), ".omp/\n.pi/\n.claude/\n.local/\n.cache/\n.config/\nprovas/\n.fixture-sessions/\n.fixture-control/\n.gitconfig\n.gitignore-global\nignored.txt\nother-project/\n");
      const tracked = path.join(root, "tracked.txt");
      write(tracked, "commit inicial\n");
      git(["add", ".gitignore", "tracked.txt"]);
      if (process.env.CHECKPOINT_SCENARIO === "directory_replaced") {
        write(path.join(root, "directory/old.txt"), "diretório original\n");
        git(["add", "directory/old.txt"]);
      }
      git(["-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false", "commit", "-qm", "fixture"]);
      write(tracked, "alteração staged anterior\n");
      git(["add", "tracked.txt"]);
      write(tracked, "antes\n");
      write(path.join(root, "ignored.txt"), "ignorado\n");
      write(path.join(root, "local-ignored.txt"), "exclusão local\n");
      write(path.join(root, "global-ignored.txt"), "exclusão global\n");
      write(path.join(root, ".git/info/exclude"), "local-ignored.txt\n");
      write(path.join(root, ".gitignore-global"), "global-ignored.txt\n");
      write(path.join(root, ".gitconfig"), `[core]\n  excludesFile = ${path.join(root, ".gitignore-global")}\n  hooksPath = ${path.join(root, ".fixture-control/hooks")}\n[commit]\n  gpgSign = true\n`);
      const hookSentinel = path.join(root, ".fixture-control/hook-ran");
      for (const name of ["pre-commit", "post-checkout", "post-commit"]) {
        const file = path.join(root, ".fixture-control/hooks", name);
        write(file, `#!/bin/sh\nprintf executado > ${JSON.stringify(hookSentinel)}\n`);
        fs.chmodSync(file, 0o700);
      }
      git(["config", "core.hooksPath", path.join(root, ".fixture-control/hooks")]);
      const realIndex = path.join(root, ".git/index");
      const indexBefore = checksum(realIndex);
      const headBefore = git(["rev-parse", "HEAD"]);
      const branchBefore = git(["symbolic-ref", "HEAD"]);
      process.env.GIT_DIR = path.join(root, ".git");
      process.env.GIT_WORK_TREE = root;
      process.env.GIT_INDEX_FILE = realIndex;
      process.env.GIT_OBJECT_DIRECTORY = path.join(root, ".git/objects");

      // O contexto expõe o SessionManager real do SDK; só seleção e navegação são dirigidas pelo fixture.
      const Manager = runtimeCtx.sessionManager.constructor as typeof SessionManager;
      const sessionDir = path.join(root, ".fixture-sessions");
      let manager = Manager.create(root, sessionDir);
      let activeCwd = root;
      let busy = false;
      let failNavigation = false;
      let onSelect: (() => Promise<void> | void) | undefined;
      let choices = [0, 2];
      let notifications: { text: string; level: string }[] = [];
      let editor = "";
      let handlers = new Map<string, Handler[]>();
      let restoreCommandName = "hangar-rewind";
      let commands = new Map<string, { handler: (args: string, ctx: ExtensionCommandContext) => Promise<void> }>();
      const emit = async (name: string, event: Record<string, unknown> = {}, ctx = context()) => {
        for (const handler of handlers.get(name) ?? []) {
          const response = await handler({ type: name, ...event }, ctx);
          if (response && typeof response === "object" && "cancel" in response && response.cancel) return response;
        }
      };
      function context(): ExtensionCommandContext {
        const value = {
          ...runtimeCtx, cwd: activeCwd, sessionManager: manager, hasUI: true, isIdle: () => !busy,
          ui: {
            ...runtimeCtx.ui,
            select: async (_title: string, options: string[]) => {
              const callback = onSelect; onSelect = undefined;
              if (callback) await callback();
              const choice = choices.shift();
              return choice === undefined ? undefined : options[choice];
            },
            notify: (text: string, level = "info") => notifications.push({ text, level }),
            setEditorText: (text: string) => { editor = text; },
          },
          navigateTree: async (id: string) => {
            if (failNavigation) throw new Error("falha sintética da conversa");
            const oldLeafId = manager.getLeafId();
            await emit("session_before_tree", { preparation: { targetId: id, oldLeafId, commonAncestorId: id, entriesToSummarize: [], userWantsSummary: false }, signal: new AbortController().signal });
            manager.branch(id);
            await emit("session_tree", { newLeafId: manager.getLeafId(), oldLeafId });
            return { cancelled: false };
          },
        };
        return value as ExtensionCommandContext;
      }
      // Import tardio mantém falhas de carregamento na conclusão estruturada do driver.
      const module = await import("../pi/git-checkpoint.ts");
      function install(override?: AgentContext) {
        handlers = new Map(); commands = new Map();
        restoreCommandName = override?.harness === "pi" ? "rewind" : "hangar-rewind";
        const api = new Proxy(pi, {
          get(target, key) {
            if (key === "on") return (name: string, handler: Handler) => {
              const listeners = handlers.get(name) ?? []; listeners.push(handler); handlers.set(name, listeners);
            };
            if (key === "appendEntry") return (name: string, data: unknown) => manager.appendCustomEntry(name, data);
            if (key === "registerCommand") return (name: string, definition: { handler: (args: string, ctx: ExtensionCommandContext) => Promise<void> }) => {
              commands.set(name, definition);
              if (override?.harness !== "pi") Reflect.apply(target.registerCommand, target, [name, definition]);
            };
            return Reflect.get(target, key);
          },
        });
        if (override) module.createCheckpointExtension(api, override);
        else module.default(api);
      }
      install();
      await emit("session_start");
      const entries = () => manager.getBranch().filter(entry => entry.type === "custom" && entry.customType === "git-checkpoint");
      const capture = async (prompt = "Modificar o arquivo sintético") => {
        await emit("before_agent_start", { prompt, systemPrompt: ["fixture"] });
        const entry = entries().at(-1);
        assert.ok(entry && entry.type === "custom", "captura deve estar persistida antes da primeira alteração");
        assert.ok(entry.data && typeof entry.data === "object");
        const data = entry.data as StoredCheckpoint;
        assert.equal(typeof data.ref, "string");
        assert.equal(typeof data.shadowDir, "string", "registro precisa identificar a origem dos objetos");
        assert.equal(data.workTree, root);
        return { entry, data };
      };
      const userMessage = (text: string) => manager.appendMessage({ role: "user", content: text, timestamp: Date.now() });
      const rewind = async (mode = 2) => {
        choices = [0, mode]; notifications = [];
        const command = commands.get(restoreCommandName);
        assert.ok(command, `Comando de restauração ausente: ${restoreCommandName}`);
        await command.handler("", context());
        result.lastNotifications = notifications;
      };
      const assertRealGitIntact = () => {
        assert.equal(checksum(realIndex), indexBefore);
        assert.equal(git(["rev-parse", "HEAD"]), headBefore);
        assert.equal(git(["symbolic-ref", "HEAD"]), branchBefore);
        assert.equal(fs.existsSync(hookSentinel), false);
      };
      const scenario = process.env.CHECKPOINT_SCENARIO;
      if (scenario === "capture_restore") {
        const rawFile = path.join(root, "arquivo [x].txt");
        write(rawFile, "a\r\nb\r\n");
        write(path.join(root, ".gitattributes"), "* text eol=lf\n");
        const captured = await capture();
        assert.ok(pi.getCommands().some(command => command.name === "hangar-rewind"));
        assert.equal(fs.existsSync(path.join(root, ".pi/agent/checkpoints")), false);
        // Árvore igual à da última foto reaproveita a revisão: cada pedido ganha registro, não commit.
        const repeated = await capture("de novo, sem mudar nada");
        assert.equal(repeated.data.ref, captured.data.ref);
        assert.equal(repeated.data.shadowDir, captured.data.shadowDir);
        assert.equal(fs.readdirSync(captured.data.shadowDir).filter(name => name.startsWith("index")).length, 0, "índice por captura precisa ser apagado");
        const listed = git(["--git-dir", captured.data.shadowDir, "ls-tree", "-r", "--name-only", captured.data.ref]);
        for (const ignored of ["ignored.txt", "local-ignored.txt", "global-ignored.txt"]) assert.ok(!listed.split("\n").includes(ignored));
        userMessage("alterar");
        write(tracked, "depois\n");
        const created = path.join(root, "created-after.txt");
        write(created, "manter\n");
        write(rawFile, "normalizado depois\n");
        await rewind();
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        assert.equal(fs.readFileSync(created, "utf8"), "manter\n");
        assert.equal(fs.readFileSync(rawFile, "utf8"), "a\r\nb\r\n");
        fs.unlinkSync(tracked);
        await rewind();
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        assertRealGitIntact();
        result.files = { restored: fs.readFileSync(tracked, "utf8"), createdAfter: fs.readFileSync(created, "utf8"), realIndexBefore: indexBefore, realIndexAfter: checksum(realIndex), realHeadBefore: headBefore, realHeadAfter: git(["rev-parse", "HEAD"]) };
      } else if (scenario === "resume_fork") {
        const captured = await capture();
        userMessage("retomar");
        await manager.ensureOnDisk(); await manager.flush();
        const originalFile = manager.getSessionFile()!;
        manager = await Manager.open(originalFile, sessionDir);
        install(); await emit("session_start");
        // Retomar a sessão reusa a pasta de checkpoints dela em vez de abrir outra com a árvore inteira.
        const resumed = await capture("após retomada");
        assert.equal(resumed.data.shadowDir, captured.data.shadowDir);
        write(tracked, "após retomada\n"); await rewind();
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        manager.createBranchedSession(captured.entry.id);
        await manager.ensureOnDisk(); await manager.flush();
        await emit("session_branch", { previousSessionFile: originalFile });
        const sourceIndex = path.join(captured.data.shadowDir, "index");
        write(sourceIndex, "índice da origem não deve ser usado\n");
        const sourceBefore = checksum(sourceIndex);
        write(tracked, "após fork\n"); await rewind();
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        assert.equal(checksum(sourceIndex), sourceBefore);
        assertRealGitIntact();
      } else if (scenario === "before_branch") {
        const captured = await capture();
        write(tracked, "antes da ramificação\n");
        choices = [0];
        await emit("session_before_branch", { entryId: captured.entry.id });
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        assertRealGitIntact();
        fs.rmSync(captured.data.shadowDir, { recursive: true });
        write(tracked, "preservar após falha\n");
        choices = [0];
        const response = await emit("session_before_branch", { entryId: captured.entry.id });
        assert.deepEqual(response, { cancel: true });
        assert.equal(fs.readFileSync(tracked, "utf8"), "preservar após falha\n");
        assertRealGitIntact();
      } else if (scenario === "branch_scope") {
        const first = await capture("primeiro pedido"); userMessage("primeiro");
        write(tracked, "intermediário\n"); await capture("pedido descartado"); userMessage("segundo");
        manager.branch(first.entry.id);
        await emit("session_tree", { newLeafId: first.entry.id, oldLeafId: null });
        write(tracked, "depois da troca de ramo\n"); await rewind();
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        assertRealGitIntact();
      } else if (scenario === "late_capture" || scenario === "native_timeout" || scenario === "capture_deadline") {
        const control = path.join(root, ".fixture-control");
        const pause = path.join(control, "pause"), started = path.join(control, "started"), release = path.join(control, "release");
        write(path.join(control, "bin/git"), `#!/usr/bin/env python3\nimport os, pathlib, sys, time\npause=pathlib.Path(${JSON.stringify(pause)})\nif pause.exists():\n pathlib.Path(${JSON.stringify(started)}).touch()\n end=time.monotonic()+45\n while not pathlib.Path(${JSON.stringify(release)}).exists():\n  if time.monotonic()>end: sys.exit(92)\n  time.sleep(0.01)\nos.execv('/usr/bin/git',['git',*sys.argv[1:]])\n`);
        fs.chmodSync(path.join(control, "bin/git"), 0o700);
        process.env.PATH = path.join(control, "bin") + path.delimiter + process.env.PATH;
        write(pause, "aguardar");
        const waitForGit = async () => {
          for (let tries = 0; !fs.existsSync(started) && tries < 200; tries++) await delay(10);
          assert.ok(fs.existsSync(started), "Git real precisa estar em curso");
        };
        if (scenario === "late_capture") {
          const pending = emit("before_agent_start", { prompt: "captura atrasada", systemPrompt: [] });
          await waitForGit();
          const previous = manager.getSessionFile();
          manager = Manager.create(root, sessionDir);
          await emit("session_switch", { reason: "new", previousSessionFile: previous });
          write(release, "liberar");
          await pending;
          assert.equal(entries().length, 0, "captura antiga não pertence à sessão nova");
          await capture("pedido da sessão nova");
        } else {
          // Dispatcher e relógio são do SDK real; só o orçamento da prova é abreviado.
          const sdk = startupSdk ?? await import("@earendil-works/pi-coding-agent");
          const completion = Promise.withResolvers<void>();
          let finished = false;
          const nativeRuntime = sdk.createExtensionRuntime();
          Object.assign(nativeRuntime, { appendEntry: (name: string, data: unknown) => manager.appendCustomEntry(name, data) });
          const extension = await sdk.loadExtensionFromFactory((nativeApi: ExtensionAPI) => {
            module.default(new Proxy(nativeApi, {
              get(target, key) {
                if (key === "on") return (name: string, handler: Handler) => {
                  const wrapped = name === "before_agent_start"
                    ? async (event: Record<string, unknown>, ctx: ExtensionContext) => {
                      try { return await handler(event, ctx); }
                      finally { finished = true; completion.resolve(); }
                    } : handler;
                  Reflect.apply(target.on, target, [name, wrapped]);
                };
                return Reflect.get(target, key);
              },
            }));
          }, root, pi.events, nativeRuntime, "checkpoint-timeout-fixture");
          const dispatcher = new sdk.ExtensionRunner([extension], nativeRuntime, root, manager, runtimeCtx.modelRegistry);
          await dispatcher.emit({ type: "session_start" });
          sdk.testSetExtensionHandlerTimeoutMs(scenario === "native_timeout" ? 100 : sdk.EXTENSION_HANDLER_TIMEOUT_MS);
          try {
            const began = performance.now();
            const gate = dispatcher.emitBeforeAgentStart("pedido com Git bloqueado", undefined, ["fixture"]);
            await waitForGit();
            await gate;
            if (scenario === "capture_deadline") {
              assert.equal(finished, true, "captura deve encerrar antes do prazo nativo");
              assert.ok(performance.now() - began < sdk.EXTENSION_HANDLER_TIMEOUT_MS);
              assert.equal(entries().length, 0);
              result.captureDeadlineMs = performance.now() - began;
            } else {
              assert.equal(finished, false, "dispatcher deve ter liberado o pedido com o Git ainda bloqueado");
              write(tracked, "alterado depois de o dispatcher liberar o pedido\n");
              await dispatcher.emit({ type: "agent_start" });
              write(release, "liberar");
              await completion.promise;
              assert.equal(entries().length, 0, "não pode persistir captura posterior ao início do turno");
              result.nativeDispatcherReleasedBeforeCapture = true;
            }
          } finally {
            write(release, "liberar");
            sdk.testSetExtensionHandlerTimeoutMs(sdk.EXTENSION_HANDLER_TIMEOUT_MS);
          }
        }
        assertRealGitIntact();
      } else if (scenario === "subagent") {
        await capture();
        const count = entries().length;
        const parent = manager.getSessionFile()!;
        const child = Manager.create(root, parent.replace(/\.jsonl$/, ""));
        const childCtx = { ...context(), sessionManager: child };
        await emit("session_start", {}, childCtx);
        await emit("before_agent_start", { prompt: "subagente", systemPrompt: [] }, childCtx);
        await emit("turn_end", {}, childCtx);
        assert.equal(entries().length, count);
        write(tracked, "alteração do pai\n"); await rewind();
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        assertRealGitIntact();
      } else if (scenario === "invalid_reference") {
        const captured = await capture();
        write(tracked, "não alterar\n");
        manager.resetLeaf(); manager.appendCustomEntry("git-checkpoint", { ...captured.data, ref: "f".repeat(40) });
        await emit("session_tree", { newLeafId: manager.getLeafId(), oldLeafId: null });
        await rewind(); assert.equal(fs.readFileSync(tracked, "utf8"), "não alterar\n");
        manager.resetLeaf(); manager.appendCustomEntry("git-checkpoint", { ...captured.data, workTree: path.join(root, "other-project") });
        await emit("session_tree", { newLeafId: manager.getLeafId(), oldLeafId: null });
        await rewind(); assert.equal(fs.readFileSync(tracked, "utf8"), "não alterar\n");
        manager.resetLeaf(); manager.appendCustomEntry("git-checkpoint", { ...captured.data, shadowDir: path.join(root, ".git") });
        await emit("session_tree", { newLeafId: manager.getLeafId(), oldLeafId: null });
        await rewind(); assert.equal(fs.readFileSync(tracked, "utf8"), "não alterar\n");
        assertRealGitIntact();
      } else if (scenario === "selection_and_partial") {
        const prompt = "Um pedido suficientemente longo para não caber no rótulo, mas que deve voltar completo ao editor.";
        const captured = await capture(prompt); userMessage(prompt);
        write(tracked, "depois\n");
        onSelect = () => { busy = true; };
        await rewind(); assert.equal(fs.readFileSync(tracked, "utf8"), "depois\n");
        busy = false;
        const originalManager = manager;
        onSelect = async () => {
          manager = Manager.create(root, sessionDir);
          await emit("session_switch", { reason: "new", previousSessionFile: originalManager.getSessionFile() });
        };
        await rewind(); assert.equal(fs.readFileSync(tracked, "utf8"), "depois\n");
        manager = originalManager;
        await emit("session_switch", { reason: "resume", previousSessionFile: undefined });
        failNavigation = true;
        await rewind(0);
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        assert.ok(notifications.some(note => note.level === "warning" || note.level === "error"));
        assert.ok(!notifications.some(note => note.level === "info"), "falha parcial não é sucesso completo");
        failNavigation = false;
        write(tracked, "conversa apenas\n");
        await rewind(1);
        assert.equal(fs.readFileSync(tracked, "utf8"), "conversa apenas\n");
        assert.equal(manager.getLeafId(), captured.entry.id);
        assert.equal(editor, prompt);
        assertRealGitIntact();
      } else if (scenario === "legacy_pi") {
        const captured = await capture();
        const piDir = path.join(root, ".pi/agent");
        const legacyDir = path.join(piDir, "checkpoints", module.sessionSlug(manager.getSessionFile()));
        git(["init", "--bare", "--template=", legacyDir]);
        git(["--git-dir", legacyDir, "-c", "core.hooksPath=/dev/null", "fetch", "--no-tags", captured.data.shadowDir, captured.data.ref]);
        git(["--git-dir", legacyDir, "config", "user.name", "pi-code-checkpoint"]);
        manager.resetLeaf();
        const userId = userMessage("pedido legado completo");
        manager.appendCustomEntry("git-checkpoint", { entryId: userId, ref: captured.data.ref, prompt: "resumo legado", createdAt: captured.data.createdAt });
        install({ harness: "pi", agentDir: piDir, claudeDir: path.join(root, ".claude") });
        await emit("session_start");
        write(tracked, "depois do registro legado\n");
        await rewind();
        assert.equal(fs.readFileSync(tracked, "utf8"), "antes\n");
        assertRealGitIntact();
      } else if (scenario === "directory_replaced") {
        await capture("antes da troca de tipo");
        fs.unlinkSync(path.join(root, "directory/old.txt"));
        fs.rmdirSync(path.join(root, "directory"));
        write(path.join(root, "directory"), "agora é arquivo\n");
        await capture("depois da troca de tipo");
        write(path.join(root, "directory"), "alterado depois\n");
        await rewind();
        assert.equal(fs.readFileSync(path.join(root, "directory"), "utf8"), "agora é arquivo\n");
        assertRealGitIntact();
      } else throw new Error(`Cenário desconhecido: ${scenario}`);
      await manager.flush();
      result.ok = true;
    } catch (error) {
      result.error = error instanceof Error ? error.stack : String(error);
    }
    fs.writeFileSync(process.env.OMP_DRIVER_RESULT!, JSON.stringify(result));
    runtimeCtx.shutdown();
  });
}
