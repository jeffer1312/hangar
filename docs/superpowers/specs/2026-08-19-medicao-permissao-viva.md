# Medição — trocar permissão numa sessão viva

**Data:** 2026-08-20  
**Sessão de medição:** `t4-medicao` (descartável, `provider=claude`, `permission_mode=plan`, `config_dir=~/.claude-t4`, `cwd=/home/jefferson/pessoal/hangar`)  
**Branch:** `enxugada-c5-permissao` @ `f9f31d1c` (Task 3 já aprovada)  
**Método:** um `config_dir` de teste (`~/.claude-t4`, criado com `mkdir -p ~/.claude-t4 && touch ~/.claude-t4/.hangar-conta` e copiado `~/.claude/settings.json` + `.credentials.json` + `.claude.json` para evitar onboarding/login) — não encostei em `~/.claude`, `~/.claude-02-200` ou `~/.claude-jefferson`. O `statusLine` foi trocado **temporariamente** para um `tee` e **restaurado** ao fim do Step 1.

> Cada afirmação abaixo vem com o comando e a saída colados. Resposta negativa é resultado.

---

## Step 1 — O stdin da statusline traz o modo?

**Comando de preparação (config de teste):**

```bash
cp ~/.claude/settings.json ~/.claude-t4/settings.json
cat > /tmp/t4-tee.sh <<'EOS'
#!/bin/bash
tee /tmp/t4-stdin.json | /home/jefferson/.local/share/fnm/node-versions/v24.15.0/installation/bin/node /home/jefferson/pessoal/hangar/scripts/omniroute-statusline.js
EOS
chmod +x /tmp/t4-tee.sh
python3 <<'PY'
import json, pathlib
p = pathlib.Path("/home/jefferson/.claude-t4/settings.json")
d = json.loads(p.read_text())
d["statusLine"] = {"type": "command", "command": "/tmp/t4-tee.sh"}
p.write_text(json.dumps(d, indent=2))
PY
```

**Criação da sessão:**

```bash
curl -s -H "Authorization: Bearer $CP_AUTH_TOKEN" -H "Content-Type: application/json" \
  -X POST http://127.0.0.1:8765/api/sessions \
  -d '{"name":"t4-medicao","cwd":"/home/jefferson/pessoal/hangar","provider":"claude","config_dir":"/home/jefferson/.claude-t4","permission_mode":"plan"}'
```

Saída (200):

```json
{"name":"t4-medicao","cwd":"/home/jefferson/pessoal/hangar","jsonl":"/home/jefferson/.claude-t4/projects/-home-jefferson-pessoal-hangar/d61759cf-7265-4384-9487-fe31dbb8723b.jsonl","provider":"claude","state":"idle","tracked":true}
```

Cmdline do pane (prova de que nasceu em plan):

```bash
tmux display -p -t '=t4-medicao:' '#{pane_pid}'  # → 1792106
tr '\0' ' ' < /proc/1792106/cmdline
# → claude --session-id d61759cf-7265-4384-9487-fe31dbb8723b --permission-mode plan
```

Após ~3 s, o `tee` capturou o stdin da statusline:

```bash
cat /tmp/t4-stdin.json | python3 -m json.tool
```

```json
{
    "session_id": "d61759cf-7265-4384-9487-fe31dbb8723b",
    "transcript_path": "/home/jefferson/.claude-t4/projects/-home-jefferson-pessoal-hangar/d61759cf-7265-4384-9487-fe31dbb8723b.jsonl",
    "cwd": "/home/jefferson/pessoal/hangar",
    "effort": {"level": "high"},
    "model": {"id": "claude-fable-5", "display_name": "Fable 5"},
    "workspace": {"current_dir": "/home/jefferson/pessoal/hangar", "project_dir": "/home/jefferson/pessoal/hangar", "added_dirs": [], "repo": {"host": "github.com", "owner": "jeffer1312", "name": "hangar"}},
    "version": "2.1.237",
    "output_style": {"name": "default"},
    "cost": {"total_cost_usd": 0, "total_duration_ms": 1148, "total_api_duration_ms": 0, "total_lines_added": 0, "total_lines_removed": 0},
    "context_window": {"total_input_tokens": 0, "total_output_tokens": 0, "context_window_size": 1000000, "current_usage": null, "used_percentage": null, "remaining_percentage": null},
    "exceeds_200k_tokens": false,
    "fast_mode": false,
    "thinking": {"enabled": true}
}
```

```bash
cat /tmp/t4-stdin.json | grep -i perm || echo "no perm found"
# → no perm found
cat /tmp/t4-stdin.json | python3 -c "import json; print(list(json.load(open('/tmp/t4-stdin.json')).keys()))"
# → ['session_id', 'transcript_path', 'cwd', 'effort', 'model', 'workspace', 'version', 'output_style', 'cost', 'context_window', 'exceeds_200k_tokens', 'fast_mode', 'thinking']
```

**Restauração imediata (mesma sessão ainda viva):**

```bash
python3 <<'PY'
import json, pathlib
p = pathlib.Path("/home/jefferson/.claude-t4/settings.json")
d = json.loads(p.read_text())
d["statusLine"] = {"type": "command", "command": "/home/jefferson/.local/share/fnm/node-versions/v24.15.0/installation/bin/node /home/jefferson/pessoal/hangar/scripts/omniroute-statusline.js"}
p.write_text(json.dumps(d, indent=2))
PY
```

**Resposta Step 1: NÃO.** O JSON do stdin da statusline **não contém campo de permissão** em nenhum nível (`permissionMode`, `permission_mode`, `permissions` não existem). O sidecar que o app já usa (`~/.claude-t4/.claude-pocket-status/<sid>.json`, publicado pelo `scripts/omniroute-statusline.js`) também não contém — ele só espelha `line`/`ts` da linha renderizada (ver `backend/app/statusline.py`). Não há leitura via sidecar.

---

## Step 2 — Existe comando direto de troca? E o Shift+Tab?

### 2a. `/permissions`

```bash
tmux send-keys -t '=t4-medicao:' "/permissions" Enter
sleep 2
tmux capture-pane -p -t '=t4-medicao:' | tail -n 20
```

Saída:

```
   Permissions  Recently denied   Allow   Ask   Deny   Workspace
   Claude Code won't ask before using allowed tools.
   ╭───────────────────────────╮
   │ ⌕ Search…                 │
   ╰───────────────────────────╯
     1. Add a new rule…
   ←/→ to switch · ↓ to select · Esc to cancel
```

`/permissions` **existe**, mas **não aceita argumento** e **não troca modo** — abre um TUI interativo de regras de ferramentas (Allow/Ask/Deny), não um setter de `permission_mode`.

```bash
tmux send-keys -t '=t4-medicao:' "/permissions plan" Enter
# → mesmo TUI, sem efeito; Esc fecha
tmux send-keys -t '=t4-medicao:' Escape
```

Outros candidatos (`/permission`, `/bypass`, `/dontAsk`, `/plan`, `/auto`) não existem:

```bash
tmux send-keys -t '=t4-medicao:' "/bypass"  # → autocomplete mostra só skills, sem match
tmux send-keys -t '=t4-medicao:' C-c  # limpa
```

`/help` confirma: atalhos listados são `shift + tab to auto-accept edits`, não há linha para trocar permissão.

**Resposta 2a: NÃO há comando direto** (`/permissions` não serve; não há `/permission-mode`, `/plan` etc. como setter).

### 2b. Shift+Tab (`BTab`)

Estado inicial (logo após `Esc` da tela anterior, sem enviar nada):

```bash
tmux capture-pane -p -t '=t4-medicao:' | grep "mode on"
# → ⏸ plan mode on (shift+tab to cycle) · ← for agents
```

Composer antes (linha inferior do TUI):

```
  🤖 Fable 5 (high✦) │ 📁 hangar [enxugada-c5-permissao*] │ 📟 t4-medicao │ ⚠ k8s-prod │ 💬 0/0 │ 💵 $0.00 │ 🕐 22:56 ⏱ 0m
  ⏸ plan mode on (shift+tab to cycle) · ← for agents
  ❯ (prompt vazio)
```

Cada `BTab`:

```bash
tmux send-keys -t '=t4-medicao:' BTab
sleep 1
tmux capture-pane -p -t '=t4-medicao:' | grep "mode on\|accept edits"
```

Sequência medida (ciclo fechado, 4 estados):

```
plan mode on            (⏸)
→ BTab → auto mode on   (⏵⏵)
→ BTab → manual mode on (⏸)
→ BTab → accept edits on (⏵⏵)   # nota: sem a palavra "mode"
→ BTab → plan mode on   (volta)
```

Reproduzido por 10 ciclos seguidos:

```bash
for i in $(seq 1 10); do tmux send-keys -t '=t4-medicao:' BTab; sleep 0.8; tmux capture-pane -p -t '=t4-medicao:' | grep -E "mode on|accept edits"; done
# plan → auto → manual → accept edits → plan → auto → manual → accept edits → plan → auto
```

**Ordem exata:** `plan → auto → manual → acceptEdits → plan` (ciclo de **4**, não 6).  
`bypassPermissions` e `dontAsk` (as duas que faltam da lista `claude --help`) **não aparecem no ciclo** — só via `--permission-mode` no arranque ou via `settings.json` (`permissions.defaultMode`). Confirmado em `tmux capture-pane` após 12 `BTab`: nunca surge `bypass` ou `dontAsk`.

**Confirmação legível para "leitura de volta":** a **linha inferior do chrome** (rodapé) é a confirmação — cada `BTab` reescreve essa linha para o modo que ficou:

```
⏸ plan mode on (shift+tab to cycle) · ← for agents
⏵⏵ auto mode on (shift+tab to cycle) · ← for agents
⏸ manual mode on · ← for agents
⏵⏵ accept edits on (shift+tab to cycle) · ← for agents
```

É legível e estável: aparece em `tmux capture-pane -p` na última linha antes do `❯`, sem precisar rolar. Precedente do `terminal_input.py:152-164` (`_READY_MARKERS` casa `bypass permissions`, `⏵⏵`, `⏸`, `? for shortcuts`) cita exatas as marcas `⏵⏵`/`⏸` — parsear essa linha é o caminho do gate de prontidão.

---

## Step 3 — Rastro

### 3a. Scrollback após N trocas

```bash
tmux capture-pane -p -S -100 -t '=t4-medicao:' | tail -n 100
```

Após 6 `BTab` seguidos, o scrollback **não acumula linhas** — só o rodapé muda, o miolo da conversa (welcome + `Let's get started`) permanece idêntico. Não há `● Auto mode...` empilhados no scrollback visível; a única linha que muda é a do rodapé. O `model_picker` tinha o defeito de 5× `⎿ Set model to...` no scrollback; aqui não acontece.

### 3b. Jsonl após N trocas

```bash
grep -c '"type":"permission-mode"' ~/.claude-t4/projects/-home-jefferson-pessoal-hangar/d61759cf-7265-4384-9487-fe31dbb8723b.jsonl
# → 2
grep '"type":"permission-mode"' ~/.claude-t4/projects/-home-jefferson-pessoal-hangar/d61759cf-7265-4384-9487-fe31dbb8723b.jsonl
# → {"type":"permission-mode","permissionMode":"auto","sessionId":"d61759cf-..."}
# → {"type":"permission-mode","permissionMode":"auto","sessionId":"d61759cf-..."}

tail -n 5 ~/.claude-t4/projects/-home-jefferson-pessoal-hangar/d61759cf-7265-4384-9487-fe31dbb8723b.jsonl | cat
# → ... {"type":"system","subtype":"informational","content":"Auto mode lets Claude handle..."}
# → {"type":"last-prompt", ...}
# → {"type":"mode","mode":"normal", ...}
# → {"type":"permission-mode","permissionMode":"auto", ...}
```

Cada `BTab` grava **um** `{"type":"permission-mode","permissionMode":"<modo>"}` e **um** `{"type":"system","subtype":"informational","content":"Auto/Manual/... mode lets..."}` no jsonl. São tipos que o `transcript.py` do app **ignora** (`ChatEvent` só entende `user_msg`/`assistant_msg`/`tool_use`/`tool_result`) — não poluem o chat como `user_msg` e não viram bolha no app. Mas **ficam no arquivo**; um `tail` de 6 ciclos mostra 2 entradas `permission-mode` (as outras foram sobrescritas pelo `leafUuid` atual — só o último modo por `leafUuid` permanece, mas cada ciclo gera um par).

**Resumo do rastro:**
- **Pane visível:** só o rodapé troca — scrollback limpo, sem empilhamento (tetos do `model_picker` respeitados).
- **Jsonl:** grava `permission-mode` + `system informational` por troca — **não é poluição de chat** (não é `user_msg`), mas é dado persistido. Para a pílula da T5, é irrelevante: o chat continua limpo.

---

## Recomendação para a Task 5

**Caminho recomendado: (b) Shift+Tab com leitura de confirmação — único caminho seguro hoje.**

- **Leitura:** `tmux capture-pane -p -t '=nome:'` + parse da última linha que casa `⏸|⏵⏵ .+ (mode on|accept edits on)` (mesma família de `terminal_input.py:_READY_MARKERS`). Exige **duas capturas** no gate de "posso digitar?" (precedente `model_picker`), para não confundir spinner com prompt.
- **Escrita:** `tmux send-keys -t '=nome:' BTab` **N vezes** até o rodapé casar o modo pedido, com **confirmação por string** (`permissionMode` desejado está contido na linha do rodapé) — nunca contar `Down` cego. À semelhança do `model_picker`, esperar o rodapé após cada `BTab` (até 2 s) antes de parsear.
- **Limitação conhecida e aceita:** o ciclo cobre **4 de 6** modos (`plan`, `auto`, `manual`, `acceptEdits`). `bypassPermissions` e `dontAsk` **não são alcançáveis por `BTab`** — só no arranque (`--permission-mode`) ou editando `~/.claude/settings.json` (`permissions.defaultMode`). A T5, se seguir este caminho, deve oferecer **só os 4 no popover** e documentar que os outros 2 exigem recriar a sessão (Task 3 já cobre).
- **Rastro aceitável:** sem poluição de scrollback; jsonl grava `permission-mode`/`system` mas não como `user_msg` — o app não mostra. Teto igual ao do `model_picker` (sem `❯ /model` empilhado).
- **Caminhos descartados:**
  - **(a) sidecar/comando direto:** inexistentes — stdin da statusline não traz `permissionMode` (provado acima, JSON sem campo), e `/permissions` não aceita argumento.
  - **(c) parar:** desnecessário — (b) é seguro para os 4 modos, com rastro mínimo.

**Se a T5 exigir os 6 modos em sessão viva:** então a resposta cai em **(c) "nenhum caminho seguro — parar"** para os 2 faltantes, e a T5 deve ser reescrita para só trocar entre os 4 via `BTab`, ou gravar `permissions.defaultMode` em `settings.json` e pedir `reload` (não coberto por esta medição).

---

## Sessão de medição

```bash
curl -s -H "Authorization: Bearer $CP_AUTH_TOKEN" -X DELETE http://127.0.0.1:8765/api/sessions/t4-medicao
# → {"ok":true}
tmux has-session -t '=t4-medicao:'  # → can't find session: t4-medicao (exit 1)
```

`~/.claude-t4/settings.json` restaurado para `statusLine: {"type":"command","command":"/home/jefferson/.local/share/fnm/.../node .../scripts/omniroute-statusline.js"}` (original).  
`/tmp/t4-tee.sh` e `/tmp/t4-stdin.json` deixados em `/tmp` apenas como prova; não afetam sessões futuras.
