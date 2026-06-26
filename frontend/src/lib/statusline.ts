// Parse the raw terminal statusline into typed fields, so the UI can place each piece where
// it belongs instead of dumping a wide <pre> that breaks on mobile.
//
// This is the HYBRID's statusline half: it reads the clear, labelled markers from the user's
// custom statusline (model, effort, cost, rate-limit windows, session time). The robust
// numeric metrics (context tokens, tokens-this-turn) come from the transcript `usage` and are
// reconciled on top of this (Slice 1B). Everything here is best-effort and defensive — a field
// that is not present just stays undefined; the ring/metrics then render an indeterminate state.

export interface StatusFields {
  model?: string;        // "Opus4.8·1M"
  effort?: string;       // "high" | "med" | "low"
  ctxUsed?: number;      // tokens in context
  ctxTotal?: number;     // model window
  ctxPct?: number;       // 0..100, clamped
  costUsd?: number;
  fiveHourPct?: number;
  fiveHourReset?: string;
  weeklyPct?: number;
  weeklyReset?: string;
  sessionTime?: string;  // ⏱ "2h37m"
  repo?: string;         // 📁 nome da pasta do projeto (git)
  branch?: string;       // branch atual (sem o '*' de dirty)
  dirty?: boolean;       // havia '*' (working tree suja)
  raw: string;
}

// "44k" -> 44000, "1M" -> 1_000_000, "1.5k" -> 1500
function toNumber(n: string, unit?: string): number {
  const base = parseFloat(n.replace(/,/g, ''));
  if (!isFinite(base)) return NaN;
  const u = (unit || '').toLowerCase();
  if (u === 'k') return base * 1e3;
  if (u === 'm') return base * 1e6;
  return base;
}

function clampPct(n: number): number {
  if (!isFinite(n)) return NaN;
  return Math.min(100, Math.max(0, n));
}

export function parseStatusLine(raw: string | null | undefined): StatusFields | null {
  if (!raw) return null;
  const out: StatusFields = { raw };

  // 🤖 Opus4.8·1M (high✦)   — model + effort (effort lives in the parenthetical)
  const model = raw.match(/🤖\s*([^(│]+?)\s*(?:\(([^)]*)\))?\s*(?:👤|│|$)/u);
  if (model) {
    if (model[1]) out.model = model[1].trim();
    if (model[2]) {
      // strip decorative glyphs (✦ ✧ etc.), keep the word
      const e = model[2].replace(/[^\p{L}\p{N}+-]/gu, '').trim();
      if (e) out.effort = e;
    }
  }

  // 💬 44k/18 40k/1M  — context. Take the last "<used>/<total>" pair as the window usage.
  const ctxSeg = raw.match(/💬([^│]*)/u);
  if (ctxSeg) {
    const pairs = [...ctxSeg[1].matchAll(/([\d.,]+)\s*([kKmM])?\s*\/\s*([\d.,]+)\s*([kKmM])?/g)];
    const last = pairs[pairs.length - 1];
    if (last) {
      const used = toNumber(last[1], last[2]);
      const total = toNumber(last[3], last[4]);
      if (isFinite(used)) out.ctxUsed = used;
      if (isFinite(total) && total > 0) {
        out.ctxTotal = total;
        out.ctxPct = clampPct((used / total) * 100);
      }
    }
  }

  const cost = raw.match(/💵\s*\$?\s*([\d.,]+)/u);
  if (cost) {
    const c = parseFloat(cost[1].replace(/,/g, ''));
    if (isFinite(c)) out.costUsd = c;
  }

  // ⚡5h:46% ↺34m
  const fiveH = raw.match(/⚡[^│]*?(\d+)\s*%\s*(?:↺\s*([^│⚡📅🕐]+))?/u);
  if (fiveH) {
    out.fiveHourPct = clampPct(parseInt(fiveH[1], 10));
    if (fiveH[2]) out.fiveHourReset = fiveH[2].trim();
  }

  // 📅7d:57% ↺sab 18h·2d1h
  const weekly = raw.match(/📅[^│]*?(\d+)\s*%\s*(?:↺\s*([^│🕐]+))?/u);
  if (weekly) {
    out.weeklyPct = clampPct(parseInt(weekly[1], 10));
    if (weekly[2]) out.weeklyReset = weekly[2].trim();
  }

  const sess = raw.match(/⏱\s*([0-9hms:]+)/u);
  if (sess) out.sessionTime = sess[1].trim();

  // 📁 frontend [main*]  — pasta do projeto + branch git (o '*' final = working tree suja)
  const repo = raw.match(/📁\s*([^[\]│]+?)\s*\[([^\]]+)\]/u);
  if (repo) {
    if (repo[1]) out.repo = repo[1].trim();
    const br = repo[2].trim();
    out.dirty = br.endsWith('*');
    out.branch = br.replace(/\*+$/, '').trim();
  }

  return out;
}
