# Upload de imagem (v1: enviar, eu vejo) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development. Steps use checkbox (`- [ ]`).

**Goal:** Permitir que o usuário, pelo app (celular, remoto), envie uma imagem pra sessão Claude Code — salva no `<cwd>/.claude-pocket-uploads/` e referenciada por path numa mensagem, pra o assistente ver via Read.

**Architecture:** Backend ganha `POST /api/sessions/{name}/upload` que recebe os **bytes crus** da imagem (sem multipart — `python-multipart` NÃO está instalado; usa `await request.body()` + header `Content-Type`), valida e salva. O app: botão 📎 no composer → escolhe foto → faz upload → pega o path → manda pelo `/input` existente `"<legenda>\n📎 imagem: <path>"`. O assistente Lê o path. Sem injeção no backend, sem render persistente (v1).

**Tech Stack:** FastAPI (Request raw body), pytest (backend TDD). Svelte 5 + Vite (frontend, verificado por `svelte-check`/`build`/device). `git -C /home/jefferson/pessoal/claude-pocket`, `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend`, nunca `cd`. Commits direto na `main`, conventional, inglês, SEM trailer `Co-Authored-By`.

---

### Task 1: backend `app/uploads.py` — save_upload (TDD)

**Files:**
- Create: `backend/app/uploads.py`
- Create: `backend/tests/test_uploads.py`

- [ ] **Step 1: Teste que falha**

Create `backend/tests/test_uploads.py`:
```python
from pathlib import Path

import pytest

from app.uploads import save_upload, UploadError

# 1x1 PNG valido (bytes minimos)
PNG = bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489"
    "0000000a49444154789c6360000002000154a24f5f0000000049454e44ae426082"
)


def test_save_upload_writes_into_cwd_subdir(tmp_path):
    cwd = str(tmp_path)
    path = save_upload(cwd, PNG, "image/png")
    p = Path(path)
    assert p.exists()
    assert p.read_bytes() == PNG
    assert p.parent == tmp_path / ".claude-pocket-uploads"
    assert p.suffix == ".png"


def test_save_upload_rejects_bad_content_type(tmp_path):
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), PNG, "application/pdf")
    assert e.value.status == 415


def test_save_upload_rejects_empty(tmp_path):
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), b"", "image/png")
    assert e.value.status == 400


def test_save_upload_rejects_too_large(tmp_path):
    big = b"x" * (10 * 1024 * 1024 + 1)
    with pytest.raises(UploadError) as e:
        save_upload(str(tmp_path), big, "image/png")
    assert e.value.status == 413


def test_save_upload_server_generated_name_not_client(tmp_path):
    # dois uploads -> nomes diferentes, gerados pelo servidor (nunca path do cliente)
    a = save_upload(str(tmp_path), PNG, "image/png")
    b = save_upload(str(tmp_path), PNG, "image/jpeg")
    assert a != b
    assert Path(a).suffix == ".png" and Path(b).suffix == ".jpg"
```

- [ ] **Step 2: Roda e vê falhar**

Run: `uv run pytest backend/tests/test_uploads.py -q` (do dir `backend`: `uv run pytest tests/test_uploads.py -q`)
Expected: FAIL com `ModuleNotFoundError: No module named 'app.uploads'`.

- [ ] **Step 3: Implementar**

Create `backend/app/uploads.py`:
```python
import os
import secrets
import time
from pathlib import Path

# content-type -> extensao. So imagens (o assistente le via Read).
ALLOWED: dict[str, str] = {
    "image/png": "png",
    "image/jpeg": "jpg",
    "image/webp": "webp",
    "image/gif": "gif",
}
MAX_BYTES = 10 * 1024 * 1024  # 10 MiB
UPLOAD_SUBDIR = ".claude-pocket-uploads"


class UploadError(Exception):
    """Erro de upload com status HTTP pra o endpoint mapear direto."""
    def __init__(self, status: int, detail: str):
        super().__init__(detail)
        self.status = status
        self.detail = detail


def save_upload(cwd: str, content: bytes, content_type: str | None) -> str:
    """Salva os bytes da imagem em <cwd>/.claude-pocket-uploads/ com nome gerado pelo
    servidor (nunca o filename do cliente -> sem path traversal). Devolve o path absoluto.
    Levanta UploadError(status, detail) em tipo invalido / vazio / grande demais."""
    ext = ALLOWED.get((content_type or "").split(";")[0].strip().lower())
    if ext is None:
        raise UploadError(415, "tipo de imagem nao suportado")
    if not content:
        raise UploadError(400, "arquivo vazio")
    if len(content) > MAX_BYTES:
        raise UploadError(413, "imagem maior que 10 MiB")

    base = Path(os.path.realpath(cwd)) / UPLOAD_SUBDIR
    base.mkdir(parents=True, exist_ok=True)
    fname = f"{int(time.time())}-{secrets.token_hex(3)}.{ext}"
    dest = base / fname
    # Containment defensivo: o destino real tem que ficar dentro de base.
    real_base = os.path.realpath(base)
    real_dest = os.path.realpath(dest)
    if not (real_dest == os.path.join(real_base, fname)):
        raise UploadError(400, "caminho invalido")
    Path(real_dest).write_bytes(content)
    return real_dest
```

- [ ] **Step 4: Roda e passa**

Run (do `backend`): `uv run pytest tests/test_uploads.py -q`
Expected: 5 passed.

- [ ] **Step 5: Commit**
```bash
git -C /home/jefferson/pessoal/claude-pocket add backend/app/uploads.py backend/tests/test_uploads.py
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(uploads): save_upload — validated image bytes into cwd subdir"
```

---

### Task 2: backend endpoint `POST /api/sessions/{name}/upload`

**Files:**
- Modify: `backend/app/api.py`

- [ ] **Step 1: Adicionar o endpoint**

Em `backend/app/api.py`:
- No import do fastapi, adicionar `Request`: trocar a 1a linha
  `from fastapi import FastAPI, Depends, HTTPException`
  por
  `from fastapi import FastAPI, Depends, HTTPException, Request`
- Adicionar o import: `from app.uploads import save_upload, UploadError`
- Adicionar o endpoint (perto dos outros de `/api/sessions/{name}/...`, ex depois do `interrupt`):
```python
@app.post("/api/sessions/{name}/upload", dependencies=[Depends(require_auth)])
async def upload(name: str, request: Request):
    # Resolve o cwd da sessao (registry.list() ja traz cwd via tmux #{pane_current_path}).
    info = next((s for s in registry.list() if s.name == name), None)
    if info is None:
        raise HTTPException(404, "sessao nao encontrada")
    if not info.cwd:
        raise HTTPException(409, "cwd da sessao indisponivel")
    # Rejeita cedo por Content-Length antes de ler o corpo todo na memoria.
    clen = request.headers.get("content-length")
    if clen and clen.isdigit() and int(clen) > 10 * 1024 * 1024:
        raise HTTPException(413, "imagem maior que 10 MiB")
    data = await request.body()
    try:
        path = save_upload(info.cwd, data, request.headers.get("content-type"))
    except UploadError as e:
        raise HTTPException(e.status, e.detail)
    return {"path": path}
```

- [ ] **Step 2: Verificar a suíte backend (sem regressão)**

Run (do `backend`): `uv run pytest -q`
Expected: tudo passa (104+ testes).

- [ ] **Step 3: Smoke do endpoint (manual, opcional)**

Com o backend rodando e uma sessão viva (`cc`), do `backend`:
```bash
TOK=$(grep CP_AUTH_TOKEN .env | cut -d= -f2)
curl -s -X POST "http://127.0.0.1:8765/api/sessions/cc/upload" \
  -H "Authorization: Bearer $TOK" -H "Content-Type: image/png" \
  --data-binary @tests/fixtures/pane_idle.txt -o /dev/null -w "%{http_code}\n"
```
Expected: `415` (pane_idle.txt não é imagem → valida o caminho de erro). (Um PNG real daria 200 + `{path}`.)

- [ ] **Step 4: Commit**
```bash
git -C /home/jefferson/pessoal/claude-pocket add backend/app/api.py
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(api): POST /sessions/{name}/upload — raw image body -> saved path"
```

---

### Task 3: frontend `lib/api.ts` — uploadImage

**Files:**
- Modify: `frontend/src/lib/api.ts`

- [ ] **Step 1: Adicionar `uploadImage`**

Em `frontend/src/lib/api.ts`, adicionar (depois de `sendInput`, reusando `authHeaders`/`getToken`/`clearCredentials` que já estão no arquivo):
```ts
/**
 * Envia os bytes crus de uma imagem pra sessao (sem multipart). O backend salva e devolve o
 * path; o app depois manda a legenda + path pelo /input. 401 -> self-heal (igual apiFetch).
 */
export async function uploadImage(name: string, file: File): Promise<{ path: string }> {
  const base = getBaseUrl();
  const res = await fetch(`${base}/api/sessions/${encodeURIComponent(name)}/upload`, {
    method: 'POST',
    headers: {
      ...authHeaders(),
      'Content-Type': file.type || 'application/octet-stream',
    },
    body: file,
  });
  if (res.status === 401 && getToken()) {
    clearCredentials();
    if (typeof window !== 'undefined') window.location.reload();
    throw new Error('401: sessão expirada — faça login novamente');
  }
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`${res.status}: ${text}`);
  }
  return res.json() as Promise<{ path: string }>;
}
```

- [ ] **Step 2: Verificar**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check`
Expected: 0 errors / 0 warnings.

- [ ] **Step 3: Commit**
```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/lib/api.ts
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(api): uploadImage — POST raw image bytes to the session"
```

---

### Task 4: frontend Composer — botão 📎 + preview + upload no envio

**Files:**
- Modify: `frontend/src/components/Composer.svelte`

- [ ] **Step 1: Script — estado + handlers**

Em `frontend/src/components/Composer.svelte`:
- Import: adicionar `uploadImage` ao import de `../lib/api` (que já traz `getCommands, setModelEffort, type ModelEffortBody`):
```ts
  import { getCommands, setModelEffort, uploadImage, type ModelEffortBody } from '../lib/api';
```
- Estado (perto dos outros `$state`):
```ts
  let attachment = $state<File | null>(null);
  let attachmentUrl = $state<string | null>(null);
  let fileInput: HTMLInputElement | undefined = $state();
  let uploading = $state(false);
  let attachError = $state('');
```
- `canSend` passa a aceitar anexo (substituir a linha `const canSend = $derived(inputText.trim().length > 0);`):
```ts
  const canSend = $derived((inputText.trim().length > 0 || attachment !== null) && !uploading);
```
- Handlers (perto de `submit`):
```ts
  function onPickFile(e: Event) {
    const f = (e.target as HTMLInputElement).files?.[0];
    if (!f) return;
    if (attachmentUrl) URL.revokeObjectURL(attachmentUrl);
    attachment = f;
    attachmentUrl = URL.createObjectURL(f);
    attachError = '';
  }

  function removeAttachment() {
    if (attachmentUrl) URL.revokeObjectURL(attachmentUrl);
    attachment = null;
    attachmentUrl = null;
    attachError = '';
    if (fileInput) fileInput.value = '';
  }
```
- Trocar a função `submit()` atual por uma versão async que sobe o anexo antes:
```ts
  async function submit() {
    if (!canSend) return;
    const caption = inputText.trim();
    if (attachment) {
      uploading = true;
      attachError = '';
      try {
        const { path } = await uploadImage(sessionName, attachment);
        const msg = (caption ? caption + '\n' : '') + '📎 imagem: ' + path;
        inputText = '';
        if (textareaEl) textareaEl.style.height = 'auto';
        removeAttachment();
        onSend(msg);
      } catch (err) {
        attachError = err instanceof Error ? err.message : 'Falha no upload';
      } finally {
        uploading = false;
      }
      return;
    }
    const msg = caption;
    inputText = '';
    if (textareaEl) textareaEl.style.height = 'auto';
    onSend(msg);
  }
```
(NOTA: a `submit()` antiga era síncrona e fazia `if (!canSend) return; const msg = inputText.trim(); inputText=''; ...; onSend(msg);` — substitua o corpo inteiro pela versão acima. O `handleKeydown` que chama `submit()` no Enter continua funcionando, agora dispara a versão async.)

- [ ] **Step 2: Markup — input escondido + botão 📎 + chip de preview**

- Adicionar o input escondido logo dentro do `<footer class="composer">` (antes do `<div class="composer-card">`):
```svelte
  <input
    type="file"
    accept="image/*"
    bind:this={fileInput}
    onchange={onPickFile}
    class="file-input"
    aria-hidden="true"
    tabindex="-1"
  />
```
- Dentro do `composer-card`, ANTES da textarea (logo após o `composer-top` do custo / antes do `<SlashSuggest>`), o chip de preview:
```svelte
    {#if attachment}
      <div class="attach-chip">
        {#if attachmentUrl}<img class="attach-thumb" src={attachmentUrl} alt="anexo" />{/if}
        <span class="attach-name">{attachment.name}</span>
        {#if uploading}<span class="attach-status">enviando…</span>{/if}
        {#if attachError}<span class="attach-error">{attachError}</span>{/if}
        <button class="attach-remove" onclick={removeAttachment} aria-label="Remover anexo">×</button>
      </div>
    {/if}
```
- No `control-left`, adicionar o botão 📎 ANTES do `slash-btn`:
```svelte
        <button class="attach-btn" onclick={() => fileInput?.click()} aria-label="Anexar imagem">
          <span class="attach-glyph" aria-hidden="true">📎</span>
        </button>
```

- [ ] **Step 3: CSS**

Adicionar ao `<style>` do Composer:
```css
  .file-input { display: none; }

  .attach-chip {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-2);
    background: var(--bg-hover);
    border-radius: var(--radius-md);
  }
  .attach-thumb {
    width: 36px;
    height: 36px;
    border-radius: var(--radius-sm);
    object-fit: cover;
    flex-shrink: 0;
  }
  .attach-name {
    font-size: var(--text-xs);
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    flex: 1;
    min-width: 0;
  }
  .attach-status { font-size: var(--text-xs); color: var(--text-muted); }
  .attach-error { font-size: var(--text-xs); color: var(--error); }
  .attach-remove {
    width: 28px; height: 28px; min-height: 0; flex-shrink: 0;
    color: var(--text-secondary); font-size: var(--text-lg); line-height: 1;
  }

  .attach-btn {
    width: 44px; height: 44px; flex-shrink: 0;
    border-radius: var(--radius-md);
    background: transparent;
    color: var(--text-secondary);
  }
  .attach-btn:active { background: var(--bg-hover); }
  .attach-glyph { font-size: var(--text-lg); line-height: 1; }
```

- [ ] **Step 4: Verificar**

Run: `npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run check && npm --prefix /home/jefferson/pessoal/claude-pocket/frontend run build`
Expected: 0 errors / 0 warnings.

- [ ] **Step 5: Commit**
```bash
git -C /home/jefferson/pessoal/claude-pocket add frontend/src/components/Composer.svelte
git -C /home/jefferson/pessoal/claude-pocket commit -m "feat(composer): attach image — 📎 picker, preview chip, upload-on-send"
```

---

### Task 5: .gitignore

**Files:**
- Modify: `.gitignore`

- [ ] **Step 1: Ignorar a pasta de uploads**

Adicionar ao `/home/jefferson/pessoal/claude-pocket/.gitignore` uma linha:
```
.claude-pocket-uploads/
```

- [ ] **Step 2: Commit**
```bash
git -C /home/jefferson/pessoal/claude-pocket add .gitignore
git -C /home/jefferson/pessoal/claude-pocket commit -m "chore: gitignore .claude-pocket-uploads/"
```

---

### Task 6: Verificação no aparelho

- [ ] Reiniciar o backend pra carregar o endpoint novo: matar o pid atual e `cd backend && uv run python -m app.main` (lê `.env`, mesmo token). Frontend pega por HMR.
- [ ] No app, na sessão **claude-pocket** (esta): tocar 📎 → escolher/tirar foto → chip de preview aparece.
- [ ] (Opcional) escrever uma legenda. Enviar.
- [ ] O assistente recebe `📎 imagem: <path>`, faz Read no path e **descreve o que vê** — em especial: **mandar o print do bug do teclado/top-bar iOS** pra finalmente diagnosticar.
- [ ] Conferir o arquivo em `<cwd>/.claude-pocket-uploads/`.
- [ ] Atualizar handoff.

---

## Cobertura (self-review)
- Endpoint upload (raw body, validação, cwd subdir, containment) → Task 1 (save_upload) + Task 2 (HTTP glue). Sem `python-multipart` (não instalado) — raw body.
- Frontend upload → Task 3 (uploadImage) + Task 4 (📎 + preview + upload-on-send).
- Legenda + path via `/input` existente → Task 4 (`onSend("<legenda>\n📎 imagem: <path>")`).
- Assistente vê → Read no path (Task 6 verificação).
- .gitignore → Task 5.
- Consistência: `save_upload(cwd, content, content_type) -> str` (Task 1) é o que o endpoint chama (Task 2); `uploadImage(name, file) -> {path}` (Task 3) é o que o Composer chama (Task 4). Marcador `📎 imagem: <path>` consistente Composer↔assistente.
- Fora do v1 (não nas tasks): render persistente, múltiplas imagens, áudio, bubble otimista de imagem (o transcript mostra o marcador-texto).
