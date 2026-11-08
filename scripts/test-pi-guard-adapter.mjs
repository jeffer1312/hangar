// Contrato do filtro público do adaptador: só a guarda nativa substitui a entrada antiga.
import assert from "node:assert/strict";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";

const home = fs.mkdtempSync(path.join(os.tmpdir(), "hangar-guard-adapter-"));
process.env.HOME = home;
process.env.USERPROFILE = home;
fs.mkdirSync(path.join(home, ".claude"));
fs.mkdirSync(path.join(home, ".pi/agent"), { recursive: true });
try {
  const { default: extension } = await import("./pi/claude-hooks-adapter.ts");
  let status;
  extension({ on() {}, registerCommand(_name, command) { status = command.handler; } });
  async function count(command, config = {}) {
    fs.writeFileSync(path.join(home, ".claude/settings.json"), JSON.stringify({ hooks: { PreToolUse: [{ hooks: [{ command }] }] } }));
    fs.writeFileSync(path.join(home, ".pi/agent/claude-hooks-adapter.json"), JSON.stringify(config));
    let text;
    await status([], { ui: { notify(value) { text = value; } } });
    return JSON.parse(text.split("mapped hooks: ")[1]).PreToolUse;
  }
  for (const command of [
    '"/opt/Hangar App/bin/hangar-server" guard-tmux',
    '"C:\\Hangar App\\bin\\hangar-server.exe" guard-tmux',
    '"/opt/current/scripts/hangar-server-command" guard-tmux',
    '/opt/bin/hangar-server guard-tmux',
  ]) assert.equal(await count(command), 1, command);
  for (const command of [
    '"/opt/bin/hangar-server" engine',
    '"/opt/bin/hangar-server" guard-tmux && touch /tmp/forbidden',
    '"/opt/$(touch /tmp/forbidden)/hangar-server" guard-tmux',
    '"/opt/`touch /tmp/forbidden`/hangar-server" guard-tmux',
    'python /opt/backend/hooks/guard_tmux.py',
    '"/opt/not-hangar-server" guard-tmux',
  ]) assert.equal(await count(command), 0, command);
  assert.equal(await count('"/opt/bin/hangar-server" guard-tmux', { allowPatterns: ["/my-hooks/"] }), 0);
  assert.equal(await count('"/opt/bin/hangar-server" guard-tmux', { skipPatterns: ["guard-tmux"] }), 0);
  assert.equal(await count('python /my-hooks/own.py', { allowPatterns: ["/my-hooks/"] }), 1);
  console.log("Adaptador Pi: guarda Rust aceita; comandos alheios, expansão e políticas pessoais preservados.");
} finally { fs.rmSync(home, { recursive: true, force: true }); }
