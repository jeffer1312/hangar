#!/usr/bin/env node
// OmniRoute Status Line para Claude Code
// Mostra billing/rate limits das contas Claude via OmniRoute

const path = require('path');
const os = require('os');
const fs = require('fs');

const stdinTimeout = setTimeout(() => process.exit(0), 3000);
let input = '';
process.stdin.setEncoding('utf8');
process.stdin.on('data', chunk => input += chunk);
process.stdin.on('end', () => {
  clearTimeout(stdinTimeout);
  try {
    const data = JSON.parse(input);

    // Expoe o custo REAL da sessao (Claude Code passa cost.total_cost_usd aqui) num arquivo temp que
    // o cost-tracker do ecc le como fonte autoritativa -> ele mostra ~$30 real em vez da estimativa
    // por soma de tokens (que infla em sessao longa). Contrato: {ts:<unix s>, cost_usd} valido <=300s.
    try {
      const sid = data.session_id;
      const costUsd = data.cost?.total_cost_usd;
      if (sid && typeof costUsd === 'number') {
        fs.writeFileSync(
          path.join(os.tmpdir(), `harness-cost-${sid}.json`),
          JSON.stringify({ ts: Math.floor(Date.now() / 1000), cost_usd: costUsd })
        );
      }
    } catch {}

    // Modelo compacto: "Opus 4.8 (1M context)" -> "Opus4.8·1M"
    const model = (data.model?.display_name || 'Claude')
      .replace(/\s*\(1M context\)/i, '·1M')
      .replace(/Opus (\d)/, 'Opus$1');
    // Workspace atual (pasta sendo trabalhada)
    const dir = path.basename(data.workspace?.current_dir || process.cwd());
    // Diretório de config do Claude (CLAUDE_CONFIG_DIR ou ~/.claude)
    const claudeDir = process.env.CLAUDE_CONFIG_DIR || path.join(os.homedir(), '.claude');
    const claudeDirShort = claudeDir.replace(os.homedir(), '~').replace(/^~\//, '');
    // Email da conta Claude (oauthAccount.emailAddress em ~/.claude.json)
    let account = '';
    try {
      // Conta do perfil ativo: usa CLAUDE_CONFIG_DIR se setado (cc=.claude-clean,
      // claude=.claude-work), senão o ~/.claude.json da raiz (perfil default).
      const cfgPath = process.env.CLAUDE_CONFIG_DIR
        ? path.join(process.env.CLAUDE_CONFIG_DIR, '.claude.json')
        : path.join(os.homedir(), '.claude.json');
      const cfg = JSON.parse(fs.readFileSync(cfgPath, 'utf8'));
      const email = cfg.oauthAccount?.emailAddress;
      if (email) account = ' \x1b[97m👤 ' + email.split('@')[0] + '\x1b[0m';
    } catch {}
    const remaining = data.context_window?.remaining_percentage;
    const totalIn = data.context_window?.total_input_tokens ?? null;
    const totalOut = data.context_window?.total_output_tokens ?? null;
    const ctxSize = data.context_window?.context_window_size ?? null;
    // Branch git (verde se limpo, amarelo se dirty)
    let gitBranch = '';
    const gitDir = data.workspace?.project_dir || data.workspace?.current_dir || '';
    if (gitDir && fs.existsSync(path.join(gitDir, '.git'))) {
      try {
        const { execFileSync } = require('child_process');
        const branch = execFileSync('git', ['-C', gitDir, '--no-optional-locks', 'rev-parse', '--abbrev-ref', 'HEAD'],
          { encoding: 'utf8', timeout: 2000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
        if (branch) {
          const dirty = execFileSync('git', ['-C', gitDir, '--no-optional-locks', 'status', '--porcelain'],
            { encoding: 'utf8', timeout: 2000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
          gitBranch = dirty
            ? ' \x1b[33m[' + branch + '*]\x1b[0m'
            : ' \x1b[32m[' + branch + ']\x1b[0m';
        }
      } catch {}
    }

    const fiveHourPct = data.rate_limits?.five_hour?.used_percentage ?? null;
    const fiveHourResets = data.rate_limits?.five_hour?.resets_at ?? null;
    const sevenDayPct = data.rate_limits?.seven_day?.used_percentage ?? null;
    const sevenDayResets = data.rate_limits?.seven_day?.resets_at ?? null;

    const fmtTok = n => {
      if (n >= 1000000) return (n / 1000000).toFixed(1).replace(/\.0$/, '') + 'M';
      if (n >= 1000) return Math.round(n / 1000) + 'k';
      return String(n);
    };

    // Tokens
    let tokens = '';
    if (totalIn != null && totalOut != null) {
      let ctxUsage = '';
      if (ctxSize != null && remaining != null) {
        const used = Math.round((1 - remaining / 100) * ctxSize);
        ctxUsage = ' ' + fmtTok(used) + '/' + fmtTok(ctxSize);
      }
      tokens = ' \x1b[97m💬 ' + fmtTok(totalIn) + '/' + fmtTok(totalOut) + '\x1b[0m\x1b[36m' + ctxUsage + '\x1b[0m';
    }

    // Rate limits nativos
    let rateLimit = '';
    let sevenDay = '';
    const fh = fiveHourPct;
    const sd = sevenDayPct;
    const fhReset = fiveHourResets;
    const sdReset = sevenDayResets;

    if (fh != null) {
      let resetStr = '';
      if (fhReset) {
        const diff = Math.max(0, fhReset - Math.floor(Date.now() / 1000));
        const m = Math.floor(diff / 60);
        if (m >= 60) { resetStr = ' ↺' + Math.floor(m/60) + 'h' + (m%60 > 0 ? m%60 + 'm' : ''); }
        else if (m > 0) { resetStr = ' ↺' + m + 'm'; }
      }
      const c = fh < 50 ? '\x1b[32m' : fh < 75 ? '\x1b[33m' : '\x1b[91m';
      rateLimit = ' ' + c + '⚡5h:' + Math.round(fh) + '%' + resetStr + '\x1b[0m';
    }
    if (sd != null) {
      let resetStr = '';
      if (sdReset) {
        // Hora absoluta de quando a janela de 7d vence: ↺sex 14h·2d3h
        const d = new Date(sdReset * 1000);
        const dias = ['dom','seg','ter','qua','qui','sex','sab'];
        const diff = Math.max(0, sdReset - Math.floor(Date.now() / 1000));
        const dd = Math.floor(diff / 86400);
        const hh = Math.floor((diff % 86400) / 3600);
        const left = (dd > 0 ? dd + 'd' : '') + hh + 'h';
        resetStr = ' ↺' + dias[d.getDay()] + ' ' + d.getHours() + 'h·' + left;
      }
      const c = sd < 50 ? '\x1b[32m' : sd < 75 ? '\x1b[33m' : '\x1b[91m';
      sevenDay = ' ' + c + '📅7d:' + Math.round(sd) + '%' + resetStr + '\x1b[0m';
    }

    // Nome da sessão tmux (= endereço da sessão no cp-send / claude-pocket)
    let tmuxSess = '';
    if (process.env.TMUX && process.env.TMUX_PANE) {
      try {
        const { execFileSync } = require('child_process');
        const s = execFileSync('tmux', ['display-message', '-p', '-t', process.env.TMUX_PANE, '#S'],
          { encoding: 'utf8', timeout: 1000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
        if (s) {
          tmuxSess = ' \x1b[95m📟 ' + s + '\x1b[0m';
          // Pareamento (claude-pocket): sidecar <config>/.claude-pocket-pair/<sessao>.json -> chip 🤝 peer.
          try {
            const pair = JSON.parse(fs.readFileSync(
              path.join(claudeDir, '.claude-pocket-pair', s + '.json'), 'utf8'));
            if (pair.peer) tmuxSess += ' \x1b[93m🤝 ' + pair.peer + '\x1b[0m';
          } catch {}
        }
      } catch {}
    }

    // kubectl current-context (vermelho piscando se prod)
    let kctx = '';
    try {
      const { execFileSync } = require('child_process');
      const ctxName = execFileSync('kubectl', ['config', 'current-context'],
        { encoding: 'utf8', timeout: 1000, stdio: ['ignore', 'pipe', 'ignore'] }).trim();
      if (ctxName) {
        const isProd = /prod/i.test(ctxName);
        const color = isProd ? '\x1b[5;1;97;41m' : '\x1b[36m';
        const prefix = isProd ? '⚠ ' : '⎈ ';
        kctx = ' ' + color + prefix + ctxName + '\x1b[0m';
      }
    } catch {}

    // Effort + thinking como sufixo do modelo: "Opus4.8·1M (high✦)"
    let effortSuffix = '';
    const effortLvl = data.effort?.level;
    const thinkingOn = data.thinking?.enabled;
    if (effortLvl) {
      effortSuffix = ' (' + effortLvl + (thinkingOn ? '✦' : '') + ')';
    } else if (thinkingOn) {
      effortSuffix = ' (thinking)';
    }

    // Custo da sessão (built-in do Claude Code; só aparece quando preenchido)
    let cost = '';
    const usd = data.cost?.total_cost_usd;
    if (usd != null) {
      cost = ' \x1b[32m💵 $' + usd.toFixed(2) + '\x1b[0m';
    }

    // Hora local HH:MM + tempo de sessão (relógio de parede)
    const now = new Date();
    const hhmm = String(now.getHours()).padStart(2, '0') + ':' + String(now.getMinutes()).padStart(2, '0');
    let sessStr = '';
    const durMs = data.cost?.total_duration_ms;
    if (durMs != null) {
      const m = Math.floor(durMs / 60000);
      sessStr = ' ⏱ ' + (m >= 60 ? Math.floor(m / 60) + 'h' + (m % 60 ? (m % 60) + 'm' : '') : m + 'm');
    }
    const clock = ' \x1b[1;97m🕐 ' + hhmm + sessStr + '\x1b[0m';

    // Segmentos lógicos (trim remove o espaço inicial que cada um trazia)
    const segs = [
      '\x1b[1;35m🤖 ' + model + effortSuffix + '\x1b[0m',
      '\x1b[97m📁 ' + dir + '\x1b[0m' + gitBranch,
      tmuxSess, kctx, tokens, cost, rateLimit, sevenDay, clock
    ].map(s => s.trim()).filter(Boolean);

    const sep = ' │ ';
    // Largura visível: ignora códigos ANSI ao medir
    const visLen = s => s.replace(/\x1b\[[0-9;]*m/g, '').length;
    // COLUMNS setado pelo Claude Code (v2.1.153+); 0 = sem wrap (fallback antigo)
    const cols = parseInt(process.env.COLUMNS, 10) || 0;

    const lines = [];
    let cur = '', curLen = 0;
    for (const seg of segs) {
      const segLen = visLen(seg);
      if (cur === '') { cur = seg; curLen = segLen; continue; }
      const add = visLen(sep) + segLen;
      if (cols > 0 && curLen + add > cols) {
        lines.push(cur);        // não cabe → quebra linha
        cur = seg; curLen = segLen;
      } else {
        cur += sep + seg; curLen += add;
      }
    }
    if (cur) lines.push(cur);

    process.stdout.write(lines.join('\n'));
  } catch {}
});
