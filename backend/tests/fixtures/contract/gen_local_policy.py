"""Golden das políticas puras que o ator Rust roda no lugar do Python (prepare_prompt, format_status,
skill_catalog). A saída vem das funções Python reais; o teste Rust compara byte a byte.

Uso, de backend/: uv run python tests/fixtures/contract/gen_local_policy.py
"""
import base64
import json
import os
import sys
import tempfile
import time
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))

TZ = "BRT3"          # UTC-3 sem horário de verão: prova que a hora é a local, não UTC
NOW = 1_800_000_000.0
PNG = base64.b64encode(bytes.fromhex(
    "89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c4890000000d49444154789c6360000002000001"
    "e221bc330000000049454e44ae426082")).decode()
MIB = 1024 * 1024


def _sub(value, root):
    if isinstance(value, str):
        return value.replace("{ROOT}", root)
    if isinstance(value, list):
        return [_sub(item, root) for item in value]
    if isinstance(value, dict):
        return {key: _sub(item, root) for key, item in value.items()}
    return value


def _materialize(files, root):
    for rel, spec in (files or {}).items():
        path = Path(root) / rel
        if spec.get("dir"):
            path.mkdir(parents=True, exist_ok=True)
            continue
        path.parent.mkdir(parents=True, exist_ok=True)
        if "b64" in spec:
            path.write_bytes(base64.b64decode(spec["b64"]))
        elif "size" in spec:
            path.write_bytes(b"\0" * spec["size"])
        else:
            # `times` repete o texto: o preenchimento de 600 KiB não entra no golden por extenso.
            path.write_bytes((spec["text"] * spec.get("times", 1)).encode("utf-8"))


def _claude_status(payload, meta, windows, now):
    from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter, _hora_local
    data = SimpleNamespace(model=payload.get("model"), effort=payload.get("effort"), usage=payload.get("usage"),
        context_window=payload.get("context_window"), cost=payload.get("cost"), meta=meta,
        janelas=[SimpleNamespace(**window) for window in windows])
    rate = payload.get("rate_limit_info") or {}
    with patch("time.time", return_value=now):
        line = ClaudeHeadlessAdapter.status_line(None, data)
    return {"status_line": line, "limit_reset": _hora_local(rate.get("resetsAt")) if rate.get("status") == "rejected" else None}


def _unknown_step(payload, meta, counts, now):
    """O `_unknown` do `runtime_policy` antes da migração, sem o estado global."""
    from app import log_paths
    from app.adapters.claude_headless.adapter import _MAX_DESCONHECIDOS_B, _TETO_DESCONHECIDOS
    kind = payload.get("kind")
    if not isinstance(kind, str) or len(kind) > 512 or not isinstance(payload.get("event"), dict):
        raise ValueError("evento privado inválido")
    key = (meta.get("key"), meta.get("generation"), kind)
    if counts.get(key, 0) >= _TETO_DESCONHECIDOS:
        return {"recorded": False, "reason": "type_limit"}
    directory = log_paths.base() / "privado"
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    path = directory / ("claude-headless-desconhecidos.jsonl" if meta["provider"] == "claude" else "codex-headless-desconhecidos.jsonl")
    if path.exists() and path.stat().st_size > _MAX_DESCONHECIDOS_B:
        return {"recorded": False, "reason": "file_limit"}
    line = json.dumps({"ts": now, "sessao": meta.get("name"), "tipo": kind, "evento": payload["event"]}, ensure_ascii=False)
    with path.open("a", encoding="utf-8") as stream:
        stream.write(line + "\n")
    counts[key] = counts.get(key, 0) + 1
    return {"recorded": True}


def _unknown_reference(case, root):
    counts, results = {}, []
    for payload in case["steps"]:
        try:
            results.append(_unknown_step(payload, case["meta"], counts, case.get("now", NOW)))
        except Exception:
            results.append({"error": True})
    files = {}
    for rel in case.get("read", []):
        path = Path(root) / rel
        files[rel] = path.read_text(encoding="utf-8") if path.exists() else None
    return {"results": results, "files": files}


def _parked_reference(case, root):
    """O `state` que o `_state_stream` do Python emite para a sessão Claude parada: `dead` sem sidecar, senão
    `idle` com modo, linha de status e o problema da última vida (que o sidecar guarda)."""
    from app.adapters.claude_headless import sessions as hl_sessions
    from app.adapters.claude_headless.adapter import ClaudeHeadlessAdapter, _linha_parada
    from app.config import settings
    name = case["name"]
    if case.get("sidecar") is not None:
        hl_sessions._write(name, case["sidecar"])
    with patch.object(settings, "projects_dir", Path(root) / "home" / ".claude" / "projects"):
        if not hl_sessions.exists(name):
            return {"state": "dead"}
        adapter = ClaudeHeadlessAdapter()
        meta = hl_sessions.load(name) or {}
        prob = adapter.problema_de(name)
        return {"state": "idle", "claude_permission_mode": meta.get("permission_mode"),
                "claude_previous_non_plan": meta.get("previous_non_plan"),
                "status_line": _linha_parada(meta, adapter.transcript_path_de(meta)),
                "problema": prob[0] if prob else None, "problema_detalhe": prob[1] if prob else None}


def reference(case, root):
    """O que o `runtime_policy.run` devolvia antes da migração, chamando as funções que ficaram."""
    kind, payload, meta = case["kind"], case["payload"], case["meta"]
    now = case.get("now", NOW)
    if kind == "parked_state":
        return _parked_reference(case, root)
    if kind == "unknown_private":
        return _unknown_reference(case, root)
    try:
        if kind == "last_usage":
            from app.adapters.claude_headless.adapter import _uso_da_ultima_chamada
            path = Path(meta["jsonl"])
            try:
                with path.open("rb"):
                    pass
            except FileNotFoundError:
                return {"usage": None}
            return {"usage": _uso_da_ultima_chamada(str(path))}
        if kind == "reload_stamp":
            from app.adapters.claude_headless.adapter import _marca_config
            recorded = (meta.get("cano") or {}).get("config_marca")
            return {"reason": "config" if recorded and _marca_config(meta.get("config_dir")) != recorded else None}
        if kind == "prepare_prompt":
            text = payload.get("text")
            if not isinstance(text, str):
                raise ValueError("entrada sem texto")
            if meta["provider"] == "claude":
                from app import uds_messaging
                from app.adapters.claude_headless.adapter import _blocos_do_prompt
                content, notices = _blocos_do_prompt(text)
                sender, _ = uds_messaging.separar_prefixo(text)
                return {"content": content, "notices": notices, "native_candidate": sender is not None}
            return {"input": [{"type": "text", "text": text}],
                    "skill_name": text.lstrip().split()[0][1:] if text.lstrip().startswith("/") else None}
        if kind == "skill_catalog":
            from app.adapters.codex.chat_controls import skills_do_catalogo
            skills = skills_do_catalogo(payload["catalog"])
            return {"skill": next((skill for skill in skills if skill["name"] == payload.get("name")), None)}
        if kind == "format_status":
            if meta["provider"] == "codex":
                from app.adapters.codex.adapter import format_status_line
                return {"status_line": format_status_line(payload.get("model") or payload.get("default_model"),
                    payload.get("effort") or payload.get("default_effort"), payload.get("token_usage"),
                    payload.get("rate_limits"), now=now)}
            return _claude_status(payload, meta, (case.get("quota") or {}).get("windows", []), now)
    except Exception:
        return {"error": True}
    raise ValueError(kind)


def _resolve_stamp(case, root):
    """`config_marca` pode pedir a marca de outro conjunto de arquivos (`stamp_of`) ou a do disco (`{STAMP}`);
    o golden guarda o hash literal, que não depende da raiz temporária."""
    from app.adapters.claude_headless.adapter import _marca_config
    cano = case["meta"].get("cano") or {}
    wanted = cano.get("config_marca")
    if isinstance(wanted, dict):
        other = Path(root) / "stamp-of"
        _materialize(wanted["stamp_of"], other)
        cano["config_marca"] = _marca_config(str(other / "cfg"))
    elif wanted == "{HOME_STAMP}":
        cano["config_marca"] = _marca_config(str(Path(root) / "home" / ".claude"))
    elif wanted == "{STAMP}":
        cano["config_marca"] = _marca_config(str(Path(root) / "cfg"))


def run_case(case):
    with tempfile.TemporaryDirectory() as root:
        _materialize(case.get("files"), root)
        case = json.loads(json.dumps(case))
        if case["kind"] == "reload_stamp":
            _resolve_stamp(case, root)
        live = _sub(case, root)
        saved = {key: os.environ.get(key) for key in ("CLAUDE_CODE_EFFORT_LEVEL", "HOME")}
        os.environ.pop("CLAUDE_CODE_EFFORT_LEVEL", None)
        os.environ["HOME"] = str(Path(root) / "home")
        os.environ.update(live.get("env") or {})
        try:
            expected = reference(live, root)
        finally:
            for key, value in saved.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value
    # O texto do Codex volta como veio, com a raiz temporária dentro: o golden guarda o marcador.
    return {**case, "expected": json.loads(json.dumps(expected, ensure_ascii=False).replace(json.dumps(root)[1:-1], "{ROOT}"))}


CLAUDE = {"provider": "claude"}


def prepare_cases():
    out = []

    def add(name, text, files=None, provider="claude", meta=None):
        out.append({"name": name, "kind": "prepare_prompt", "payload": {"text": text}, "files": files,
                    "meta": {**(meta or {}), "provider": provider}})

    png = {"b64": PNG}
    add("plain", "só texto, sem imagem")
    add("one_image", "veja 📎 imagem: {ROOT}/a.png", {"a.png": png})
    add("two_images", "x 📎 imagem: {ROOT}/a.jpg 📎 imagem: {ROOT}/b.webp", {"a.jpg": png, "b.webp": png})
    add("uppercase_suffix", "📎 imagem: {ROOT}/A.PNG", {"A.PNG": png})
    add("missing_file", "📎 imagem: {ROOT}/nope.png")
    add("one_ok_one_missing", "📎 imagem: {ROOT}/a.gif 📎 imagem: {ROOT}/nope.jpeg", {"a.gif": png})
    add("directory_named_like_image", "📎 imagem: {ROOT}/dir.png", {"dir.png": {"dir": True}})
    add("over_cap_7mb", "📎 imagem: {ROOT}/big.png", {"big.png": {"size": 7 * MIB}})
    add("over_cap_by_one_byte", "📎 imagem: {ROOT}/big.png", {"big.png": {"size": 5 * MIB + 1}})
    add("trailing_punctuation", "olha (📎 imagem: {ROOT}/a.png).", {"a.png": png})
    add("path_with_space", "📎 imagem: {ROOT}/with space.png", {"with space.png": png})
    add("first_word_fallback", "📎 imagem: {ROOT}/a.png e mais texto colado", {"a.png": png})
    add("unsupported_suffix", "📎 imagem: {ROOT}/a.txt", {"a.txt": png})
    add("no_suffix", "📎 imagem: {ROOT}/semsufixo", {"semsufixo": png})
    add("multiline", "legenda\n📎 imagem: {ROOT}/a.png\noutra linha 📎 imagem: {ROOT}/b.png", {"a.png": png, "b.png": png})
    add("newline_after_marker", "📎 imagem:\n {ROOT}/a.png", {"a.png": png})
    add("marker_without_path", "📎 imagem:")
    add("marker_blanks_only", "📎 imagem:   \n")
    add("marker_no_space", "📎imagem:{ROOT}/a.png", {"a.png": png})
    add("emoji_without_label", "📎 {ROOT}/a.png", {"a.png": png})
    add("prefix_de", "[de: sessao-x] oi")
    add("prefix_grupo", "[grupo: g1]\nmsg")
    add("prefix_painel", "[painel: Meu painel] aviso")
    add("prefix_blank_name", "[de:  ] x")
    add("prefix_not_at_start", "texto [de: x] oi")
    add("prefix_leading_space", " [de: x] oi")
    add("prefix_unknown_tag", "[para: x] oi")
    add("prefix_with_image", "[de: x] 📎 imagem: {ROOT}/a.png", {"a.png": png})
    add("unicode_text", "ação — 日本語 📎 imagem: {ROOT}/ü.png", {"ü.png": png})
    out.append({"name": "text_not_a_string", "kind": "prepare_prompt", "payload": {"text": 5}, "meta": CLAUDE})
    out.append({"name": "text_missing", "kind": "prepare_prompt", "payload": {}, "meta": CLAUDE})
    add("codex_plain", "olá", provider="codex")
    add("codex_slash", "/minha-skill arg1 arg2", provider="codex")
    add("codex_slash_leading_blank", "  \n /outra\tx", provider="codex")
    add("codex_slash_only", "/", provider="codex")
    add("codex_slash_with_image_marker", "/s 📎 imagem: {ROOT}/a.png", {"a.png": png}, provider="codex")
    add("codex_prefix_is_not_native", "[de: x] oi", provider="codex")
    return out


def status_cases():
    out = []
    usage = {"input_tokens": 1500, "cache_creation_input_tokens": 1000, "cache_read_input_tokens": 0, "output_tokens": 500}

    def add(name, payload, meta=None, quota=None, env=None, files=None, now=NOW):
        case = {"name": name, "kind": "format_status", "payload": payload, "meta": {"provider": "claude", **(meta or {})},
                "now": now}
        if quota is not None:
            case["quota"] = {"windows": quota}
        if env:
            case["env"] = env
        if files:
            case["files"] = files
        out.append(case)

    for model in ("claude-opus-5", "claude-opus-4-7", "claude-opus-5[1m]", "claude-opus-5[1M]", "claude-sonnet-4-5-20250929",
                  "claude-haiku-5", "claude-fable-5-1", "claude-fable-5-1[1m]", "opus", "sonnet[1m]", "claude-sonnet",
                  "gpt-5.5", "  claude-opus-5  ", "claude-opus-4-1-20250805", "[1m]", "claude-"):
        add(f"model_{model.strip() or 'blank'}", {"model": model, "effort": "high"})
    add("model_engine_slash_with_account", {"model": "deepseek/deepseek-v4", "effort": "low"}, meta={"engine_account": "ds"})
    add("model_engine_slash_without_account", {"model": "deepseek/deepseek-v4", "effort": "low"})
    add("model_engine_family_after_slash", {"model": "anthropic/claude-sonnet-4-6"}, meta={"engine_account": "x"},
        env={"CLAUDE_CODE_EFFORT_LEVEL": "max"})
    add("model_empty", {"model": ""})
    add("model_null_everything_null", {})
    add("effort_from_env", {"model": "claude-opus-5"}, env={"CLAUDE_CODE_EFFORT_LEVEL": "xhigh"})
    add("effort_session_beats_env", {"model": "claude-opus-5", "effort": "low"}, env={"CLAUDE_CODE_EFFORT_LEVEL": "xhigh"})
    cfg = {"cfg/settings.json": {"text": json.dumps({"effortLevel": "medium"})}}
    add("effort_from_account_settings", {"model": "claude-opus-5"}, meta={"config_dir": "{ROOT}/cfg"}, files=cfg)
    add("effort_env_beats_settings", {"model": "claude-opus-5"}, meta={"config_dir": "{ROOT}/cfg"}, files=cfg,
        env={"CLAUDE_CODE_EFFORT_LEVEL": "high"})
    add("effort_settings_not_json", {"model": "claude-opus-5"}, meta={"config_dir": "{ROOT}/cfg"},
        files={"cfg/settings.json": {"text": "{nao"}})
    add("effort_settings_not_object", {"model": "claude-opus-5"}, meta={"config_dir": "{ROOT}/cfg"},
        files={"cfg/settings.json": {"text": "[1]"}})
    add("effort_settings_empty_level", {"model": "claude-opus-5"}, meta={"config_dir": "{ROOT}/cfg"},
        files={"cfg/settings.json": {"text": json.dumps({"effortLevel": ""})}})
    add("effort_settings_level_not_string", {"model": "claude-opus-5"}, meta={"config_dir": "{ROOT}/cfg"},
        files={"cfg/settings.json": {"text": json.dumps({"effortLevel": 3})}})
    add("effort_settings_missing", {"model": "claude-opus-5"}, meta={"config_dir": "{ROOT}/vazio"})
    add("effort_from_home_default", {"model": "claude-opus-5"},
        files={"home/.claude/settings.json": {"text": json.dumps({"effortLevel": "low"})}})

    ctx = {"model": "claude-opus-5", "effort": "high", "context_window": 200000}
    add("usage_basic", {**ctx, "usage": usage})
    add("usage_tie_rounds_to_even", {**ctx, "usage": {"input_tokens": 2500}})
    add("usage_tie_rounds_to_even_odd", {**ctx, "usage": {"input_tokens": 1500, "output_tokens": 3500}})
    add("usage_millions", {**ctx, "usage": {"input_tokens": 1_234_567, "output_tokens": 2_500_000}, "context_window": 2_500_000})
    add("usage_below_1000", {**ctx, "usage": {"input_tokens": 999, "output_tokens": 1}, "context_window": 999})
    add("usage_float_values", {**ctx, "usage": {"input_tokens": 1499.5, "output_tokens": 0.5}})
    add("usage_zero", {**ctx, "usage": {"input_tokens": 0}})
    add("usage_null_values", {**ctx, "usage": {"input_tokens": None, "cache_read_input_tokens": 12000}})
    add("usage_without_window", {"model": "claude-opus-5", "usage": usage})
    add("usage_window_zero", {"model": "claude-opus-5", "usage": usage, "context_window": 0})
    add("usage_null", {**ctx, "usage": None})
    add("usage_cache_only", {**ctx, "usage": {"cache_read_input_tokens": 150_000, "output_tokens": 800}})

    for cost in (0, 0.0, 0.125, 0.375, 0.005, 1.005, 2.675, 12.3456, 3, 1234.5, 0.994999):
        add(f"cost_{cost!r}", {"model": "claude-opus-5", "cost": cost})
    add("cost_null", {"model": "claude-opus-5", "cost": None})
    add("cost_only", {"cost": 0.5})

    windows = [{"rotulo": "5h", "pct": 42, "reset_ts": NOW + 90, "por_modelo": False},
               {"rotulo": "7d", "pct": 7.5, "reset_ts": NOW + 2 * 86400 + 3600, "por_modelo": False}]
    add("windows_both", {"model": "claude-opus-5"}, quota=windows)
    add("windows_five_hours_only", {"model": "claude-opus-5"}, quota=windows[:1])
    add("windows_without_model", {}, quota=windows)
    add("windows_empty", {"model": "claude-opus-5"}, quota=[])
    add("windows_unknown_label_skipped", {"model": "claude-opus-5"},
        quota=[{"rotulo": "1h", "pct": 10, "reset_ts": None}, windows[0]])
    for index, (pct, reset) in enumerate(((0.5, None), (1.5, NOW), (2.5, NOW - 100), (99.5, NOW + 59), (100, NOW + 3600),
                                          (41.5, NOW + 3600 * 5 + 60), (12, NOW + 86400), (12, NOW + 86400 * 9 + 3 * 3600 + 59),
                                          (12, NOW + 60), (12, NOW + 3599.9), (33, 0), (33, NOW + 0.5))):
        add(f"window_pct_reset_{index}", {"model": "claude-opus-5"}, quota=[{"rotulo": "5h", "pct": pct, "reset_ts": reset}])
    add("window_seven_days_hours_only", {}, quota=[{"rotulo": "7d", "pct": 5, "reset_ts": NOW + 3 * 86400}])
    add("windows_with_usage_cost_and_model", {"model": "claude-sonnet-4-5", "effort": "medium", "usage": usage,
        "context_window": 1000000, "cost": 4.2}, quota=windows)

    rate = lambda status, resets: {"rate_limit_info": {"status": status, "resetsAt": resets}}
    add("limit_rejected", {"model": "claude-opus-5", **rate("rejected", NOW + 5 * 3600)})
    add("limit_rejected_float", {"model": "claude-opus-5", **rate("rejected", NOW + 12345.678)})
    add("limit_rejected_local_midnight", {**rate("rejected", 1_800_000_000 - 1_800_000_000 % 86400 + 3 * 3600 - 1)})
    add("limit_allowed_ignores_reset", {**rate("allowed", NOW)})
    add("limit_rejected_without_reset", {"rate_limit_info": {"status": "rejected"}})
    add("limit_rejected_reset_text", {**rate("rejected", "amanhã")})
    add("limit_null_info", {"model": "claude-opus-5", "rate_limit_info": None})
    add("limit_info_not_an_object", {"model": "claude-opus-5", "rate_limit_info": "boom"})
    add("limit_rejected_negative_epoch", {**rate("rejected", -3600)})
    return out


def codex_cases():
    out = []

    def add(name, payload, now=NOW):
        out.append({"name": name, "kind": "format_status", "payload": payload, "meta": {"provider": "codex"}, "now": now})

    usage = {"last": {"inputTokens": 14389, "outputTokens": 1200}, "modelContextWindow": 258400}
    primary = {"windowDurationMins": 300, "usedPercent": 41.6, "resetsAt": NOW + 3 * 3600 + 120}
    secondary = {"windowDurationMins": 10080, "usedPercent": 12, "resetsAt": NOW + 5 * 86400}
    add("full", {"model": "GPT-5.5", "effort": "high", "token_usage": usage, "rate_limits": {"primary": primary, "secondary": secondary}})
    add("only_model", {"model": "gpt-5.5"})
    add("model_default_fallback", {"model": None, "default_model": "gpt-5.5", "effort": None, "default_effort": "medium"})
    add("model_blank_falls_back", {"model": "", "default_model": "gpt-5.5", "effort": "", "default_effort": "low"})
    add("model_beats_default", {"model": "a", "default_model": "b", "effort": "x", "default_effort": "y"})
    add("effort_without_model", {"effort": "high"})
    add("effort_default_without_model", {"default_effort": "high"})
    add("everything_missing", {})
    add("usage_needs_window", {"token_usage": {"last": {"inputTokens": 1}}})
    add("usage_needs_input", {"token_usage": {"last": {"outputTokens": 1}, "modelContextWindow": 1000}})
    add("usage_zero_input", {"token_usage": {"last": {"inputTokens": 0}, "modelContextWindow": 1000}})
    add("usage_without_output", {"token_usage": {"last": {"inputTokens": 2500}, "modelContextWindow": 1_000_000}})
    add("usage_empty_object", {"token_usage": {}})
    add("windows_five_hours_edges", {"rate_limits": {"primary": {"windowDurationMins": 270, "usedPercent": 1}, "secondary": {"windowDurationMins": 330, "usedPercent": 2}}})
    add("windows_outside_ranges", {"rate_limits": {"primary": {"windowDurationMins": 269, "usedPercent": 1}, "secondary": {"windowDurationMins": 331, "usedPercent": 2}}})
    add("windows_seven_days_edges", {"rate_limits": {"primary": {"windowDurationMins": 10020, "usedPercent": 1}, "secondary": {"windowDurationMins": 10140, "usedPercent": 2}}})
    add("windows_seven_days_outside", {"rate_limits": {"primary": {"windowDurationMins": 10019, "usedPercent": 1}, "secondary": {"windowDurationMins": 10141, "usedPercent": 2}}})
    add("window_missing_percent", {"rate_limits": {"primary": {"windowDurationMins": 300}}})
    add("window_missing_minutes", {"rate_limits": {"primary": {"usedPercent": 5}}})
    add("window_no_reset", {"rate_limits": {"primary": {"windowDurationMins": 300, "usedPercent": 50.5}}})
    add("window_banker_percent", {"rate_limits": {"primary": {"windowDurationMins": 300, "usedPercent": 0.5}, "secondary": {"windowDurationMins": 10080, "usedPercent": 2.5}}})
    add("window_reset_in_past", {"rate_limits": {"primary": {"windowDurationMins": 300, "usedPercent": 5, "resetsAt": NOW - 50}}})
    add("window_reset_days_only", {"rate_limits": {"secondary": {"windowDurationMins": 10080, "usedPercent": 5, "resetsAt": NOW + 2 * 86400}}})
    add("window_reset_hours_only", {"rate_limits": {"secondary": {"windowDurationMins": 10080, "usedPercent": 5, "resetsAt": NOW + 7200}}})
    add("window_reset_minutes", {"rate_limits": {"primary": {"windowDurationMins": 300, "usedPercent": 5, "resetsAt": NOW + 1799}}})
    add("rate_limits_empty", {"rate_limits": {}})
    add("rate_limits_null_window", {"rate_limits": {"primary": None, "secondary": secondary}})
    return out


def catalog_cases():
    out = []

    def skill(name, path, enabled=True, **extra):
        return {"name": name, "path": path, "enabled": enabled, **extra}

    def add(name, catalog, wanted):
        out.append({"name": name, "kind": "skill_catalog", "payload": {"catalog": catalog, "name": wanted},
                    "meta": {"provider": "codex"}})

    group = {"data": [{"skills": [skill("beta", "/s/beta", description="B"), skill("alpha", "/s/alpha")]}]}
    add("found", group, "alpha")
    add("not_found", group, "gamma")
    add("name_null", group, None)
    twins = {"data": [{"skills": [skill("dup", "/a/dup"), skill("solo", "/s/solo")]}, {"skills": [skill("dup", "/b/dup")]}]}
    add("homonyms_first", twins, "dup:" + __import__("hashlib").sha256(b"/a/dup").hexdigest()[:8])
    add("homonyms_second", twins, "dup:" + __import__("hashlib").sha256(b"/b/dup").hexdigest()[:8])
    add("homonyms_bare_name_not_found", twins, "dup")
    add("disabled_and_incomplete_skipped", {"data": [{"skills": [skill("off", "/s/off", False), skill("", "/s/x"),
        skill("nopath", ""), {"name": "nokey", "path": "/s/nokey"}, skill("on", "/s/on")]}]}, "on")
    add("disabled_not_found", {"data": [{"skills": [skill("off", "/s/off", False)]}]}, "off")
    add("same_path_listed_twice_last_wins", {"data": [{"skills": [skill("old", "/s/p", description="1")]},
        {"skills": [skill("new", "/s/p", description="2")]}]}, "new")
    add("empty_data", {"data": []}, "x")
    add("no_data_key", {}, "x")
    add("group_without_skills", {"data": [{}]}, "x")
    add("unicode_name_sorted_by_code_point", {"data": [{"skills": [skill("zé", "/s/1"), skill("Zebra", "/s/2"),
        skill("ze", "/s/3"), skill("é", "/s/4")]}]}, "zé")
    out.append({"name": "catalog_missing", "kind": "skill_catalog", "payload": {"name": "x"}, "meta": {"provider": "codex"}})
    return out


def last_usage_cases():
    out = []

    def add(name, body, meta=None):
        case = {"name": name, "kind": "last_usage", "payload": {}, "meta": {"provider": "claude", "jsonl": "{ROOT}/t.jsonl", **(meta or {})}}
        if body is not None:
            case["files"] = {"t.jsonl": body}
        out.append(case)

    def row(**fields):
        return json.dumps(fields, ensure_ascii=False) + "\n"

    good = {"input_tokens": 10, "cache_read_input_tokens": 5, "output_tokens": 3}
    assistant = lambda usage, **extra: row(type="assistant", message={"usage": usage}, **extra)
    add("missing_transcript", None)
    add("directory_instead_of_file", {"dir": True})
    add("empty_file", {"text": ""})
    add("no_assistant_line", {"text": row(type="user", message={"content": "oi"})})
    add("last_assistant_wins", {"text": assistant({"input_tokens": 1}) + assistant(good)})
    add("zero_tokens_skipped", {"text": assistant(good) + assistant({"input_tokens": 0, "output_tokens": 9})})
    add("only_cache_creation_counts", {"text": assistant({"cache_creation_input_tokens": 7})})
    add("output_only_is_not_usage", {"text": assistant({"output_tokens": 7})})
    add("sidechain_skipped", {"text": assistant(good) + assistant({"input_tokens": 99}, isSidechain=True)})
    add("sidechain_false_counts", {"text": assistant({"input_tokens": 4}, isSidechain=False)})
    add("message_null", {"text": assistant(good) + row(type="assistant", message=None)})
    add("usage_not_a_dict", {"text": assistant(good) + assistant("lots")})
    add("assistant_word_in_user_line", {"text": assistant(good) + row(type="user", message={"content": '"assistant"'})})
    add("last_line_partial", {"text": assistant(good) + '{"type": "assistant", "message": {"usage": {"input_tok'})
    add("last_line_partial_with_newline", {"text": assistant(good) + '{"type": "assistant", "mess\n'})
    add("blank_and_garbage_lines", {"text": assistant(good) + "\n\nnot json \"assistant\"\n"})
    add("crlf_line_endings", {"text": assistant({"input_tokens": 2}).replace("\n", "\r\n") + assistant({"input_tokens": 3}).replace("\n", "\r\n")})
    add("unicode_separator_inside_string_splits_the_line",
        {"text": assistant(good) + '{"type": "assistant", "x": "a b", "message": {"usage": {"input_tokens": 50}}}\n'})
    add("unicode_text_kept", {"text": row(type="assistant", note="ação ✓", message={"usage": {"input_tokens": 1, "tag": "é"}})})
    add("float_and_nested_usage", {"text": assistant({"input_tokens": 1.5, "cache_read_input_tokens": 2, "iterations": [{"input_tokens": 1}]})})
    add("invalid_utf8_line_skipped", {"b64": base64.b64encode(assistant(good).encode() + b'{"type": "assistant", "x": "\xff\xfe"}\n').decode()})
    add("non_object_line_with_the_word", {"text": assistant(good) + '"assistant"\n'})
    add("message_is_a_string", {"text": assistant(good) + row(type="assistant", message="oops")})
    # A cauda é de 512 KiB: o que está antes dela não existe, e o corte pode partir uma linha ao meio.
    pad = row(type="user", message={"content": "x" * 1000})
    add("usage_before_the_tail_is_invisible", {"text": assistant(good) + pad * 600})
    add("usage_inside_the_tail", {"text": pad * 300 + assistant(good) + pad * 100})
    add("cut_lands_inside_a_line", {"text": row(type="assistant", message={"usage": {"input_tokens": 77}}, pad="y" * 524_000) + assistant(good)})
    add("cut_line_still_parses_if_whole", {"text": assistant({"input_tokens": 8}) + "z" * 524_000 + "\n"})
    add("exactly_512k_file", {"text": assistant({"input_tokens": 6}) + "w" * (512 * 1024 - len(assistant({"input_tokens": 6})) - 1) + "\n"})
    return out


def reload_cases():
    out = []
    cfg = lambda files: {f"cfg/{name}": {"text": text} for name, text in files.items()}
    mcp = json.dumps({"mcpServers": {"b": {"cmd": "x"}, "a": {"cmd": "y", "args": ["é"]}}, "other": 1})
    mcp_reordered = json.dumps({"other": 2, "mcpServers": {"a": {"args": ["é"], "cmd": "y"}, "b": {"cmd": "x"}}})
    settings = '{\n  "effortLevel": "high"\n}\n'

    def add(name, files, marca, config_dir="{ROOT}/cfg", meta=None):
        case_meta = {"provider": "claude", "config_dir": config_dir, "cano": {"config_marca": marca}, **(meta or {})}
        if marca is None:
            case_meta["cano"] = {}
        out.append({"name": name, "kind": "reload_stamp", "payload": {}, "meta": case_meta, "files": cfg(files) if files is not None else None})

    add("no_recorded_stamp", {"settings.json": settings}, None)
    add("empty_recorded_stamp", {"settings.json": settings}, "")
    add("matches", {".claude.json": mcp, "settings.json": settings}, "{STAMP}")
    add("settings_changed", {".claude.json": mcp, "settings.json": settings}, {"stamp_of": cfg({".claude.json": mcp, "settings.json": settings + " "})})
    add("mcp_changed", {".claude.json": mcp, "settings.json": settings}, {"stamp_of": cfg({".claude.json": mcp.replace('"y"', '"z"'), "settings.json": settings})})
    add("key_order_and_other_keys_do_not_matter", {".claude.json": mcp_reordered, "settings.json": settings},
        {"stamp_of": cfg({".claude.json": mcp, "settings.json": settings})})
    add("stale_literal", {".claude.json": mcp, "settings.json": settings}, "0" * 40)
    add("without_mcp_servers", {".claude.json": '{"other": 1}', "settings.json": settings}, {"stamp_of": cfg({"settings.json": settings})})
    add("without_mcp_vs_with", {".claude.json": mcp, "settings.json": settings}, {"stamp_of": cfg({".claude.json": '{"other": 1}', "settings.json": settings})})
    add("claude_json_not_an_object", {".claude.json": "[1, 2]", "settings.json": settings}, {"stamp_of": cfg({"settings.json": settings})})
    add("missing_both_files", {}, "{STAMP}")
    add("missing_both_files_vs_stale", {}, "1" * 40)
    add("missing_settings", {".claude.json": mcp}, "{STAMP}")
    add("missing_claude_json", {"settings.json": settings}, "{STAMP}")
    add("settings_crlf_is_read_as_lf", {"settings.json": settings.replace("\n", "\r\n")}, {"stamp_of": cfg({"settings.json": settings})})
    add("settings_lone_cr_is_read_as_lf", {"settings.json": "a\rb"}, {"stamp_of": cfg({"settings.json": "a\nb"})})
    add("settings_invalid_json_is_not_parsed", {"settings.json": "not json {"}, "{STAMP}")
    add("unicode_mcp_is_escaped", {".claude.json": json.dumps({"mcpServers": {"ç": "ã", "😀": [1.0, 2.5, None, True]}}, ensure_ascii=False)}, "{STAMP}")
    # Ilegível levanta no Python mesmo com marca gravada; a marca é literal porque o disco não se hasheia.
    add("claude_json_broken", {".claude.json": "{broken", "settings.json": settings}, "2" * 40)
    add("claude_json_empty", {".claude.json": "", "settings.json": settings}, "2" * 40)
    add("claude_json_with_bom", {".claude.json": "﻿" + mcp, "settings.json": settings}, "2" * 40)
    add("settings_with_bom_is_kept", {"settings.json": "﻿" + settings}, {"stamp_of": cfg({"settings.json": "﻿" + settings})})
    out.append({"name": "config_dir_empty_falls_back_to_home", "kind": "reload_stamp", "payload": {},
                "meta": {"provider": "claude", "config_dir": "", "cano": {"config_marca": "{HOME_STAMP}"}},
                "files": {"home/.claude/settings.json": {"text": settings + "x"}}})
    return out


def unknown_cases():
    out = []

    def add(name, steps, meta=None, files=None, read=None):
        # Cada caso tem a própria chave: o contador de teto é global no processo do Rust.
        case_meta = {"provider": "claude", "key": name, "generation": 3, "name": "sessao-x", **(meta or {})}
        out.append({"name": name, "kind": "unknown_private", "payload": {}, "meta": case_meta, "steps": steps, "files": files,
                    "read": read if read is not None else [CLAUDE_LOG], "now": NOW})

    event = lambda **fields: {"kind": "k", "event": fields}
    add("claude_first_line", [event(type="x", n=1)])
    add("codex_goes_to_its_own_file", [{"kind": "decode:thread/read", "event": {"a": 1}}], meta={"provider": "codex"}, read=[CODEX_LOG, CLAUDE_LOG])
    add("unicode_not_escaped", [{"kind": "ação/é", "event": {"texto": "olá ✓ 😀", "lista": [1, 2.5, None, True, "x"]}}])
    add("key_order_kept", [{"kind": "k", "event": {"z": 1, "a": {"y": 2, "b": 3}, "m": []}}])
    add("floats_and_ints", [{"kind": "k", "event": {"a": 1.0, "b": 1e22, "c": 0.1, "d": -0.0, "e": 12345678901234567890}}])
    add("session_name_null", [event(type="x")], meta={"name": None})
    add("appends_in_order", [{"kind": "k1", "event": {"i": 1}}, {"kind": "k2", "event": {"i": 2}}, {"kind": "k1", "event": {"i": 3}}])
    add("type_cap_30_per_kind", [{"kind": "k", "event": {"i": i}} for i in range(32)] + [{"kind": "other", "event": {"i": 99}}])
    add("existing_file_is_appended", [event(type="x")], files={LOG_REL_CLAUDE: {"text": '{"old": true}\n'}})
    add("file_at_the_cap_still_records", [event(type="x")], files={LOG_REL_CLAUDE: {"size": 10 << 20}}, read=[])
    add("file_over_the_cap_blocks", [event(type="x")], files={LOG_REL_CLAUDE: {"size": (10 << 20) + 1}}, read=[])
    add("other_provider_file_does_not_block", [event(type="x")], meta={"provider": "codex"},
        files={LOG_REL_CLAUDE: {"size": (10 << 20) + 1}}, read=[CODEX_LOG])
    add("kind_512_chars_ok", [{"kind": "é" * 512, "event": {}}])
    add("kind_513_chars_invalid", [{"kind": "é" * 513, "event": {}}], read=[])
    add("kind_not_a_string", [{"kind": 3, "event": {}}], read=[])
    add("kind_missing", [{"event": {}}], read=[])
    add("event_not_an_object", [{"kind": "k", "event": [1]}, {"kind": "k", "event": "x"}, {"kind": "k", "event": None}, {"kind": "k"}], read=[])
    add("invalid_step_does_not_count", [{"kind": "k", "event": 1}, event(type="ok")])
    return out


def parked_cases():
    out = []
    usage = json.dumps({"type": "assistant", "message": {"usage": {
        "input_tokens": 1500, "cache_creation_input_tokens": 1000, "cache_read_input_tokens": 0, "output_tokens": 500}}}) + "\n"
    sid = "11111111-2222-3333-4444-555555555555"
    base = {"cwd": "/work/proj", "session_id": sid, "provider": "claude", "headless": True}

    def add(name, sidecar, files=None, env=None):
        case = {"name": name, "kind": "parked_state", "payload": {}, "meta": {},
                "sidecar": None if sidecar is None else {**base, "name": name, **sidecar}}
        if files:
            case["files"] = files
        if env:
            case["env"] = env
        out.append(case)

    home_t = f"home/.claude/projects/-work-proj/{sid}.jsonl"
    add("plan_mode_with_previous", {"model": "claude-opus-5[1m]", "effort": "high", "context_window": 1000000,
        "permission_mode": "plan", "previous_non_plan": "acceptEdits"}, {home_t: {"text": usage}})
    add("default_mode_no_previous", {"model": "claude-sonnet-4-5-20250929", "effort": "low", "permission_mode": "default",
        "previous_non_plan": None})
    add("plan_without_previous", {"model": "claude-haiku-5", "permission_mode": "plan"})
    add("problem_with_detail", {"model": "claude-opus-5", "problema": ["limite_de_uso", "volta às 14:00"]})
    add("problem_without_detail", {"model": "claude-opus-5", "problema": ["credencial", None]})
    add("engine_with_account_prefix", {"model": "deepseek/deepseek-v4", "effort": "low", "engine": "ds", "engine_account": "ds",
        "context_window": 128000}, {home_t: {"text": usage}})
    add("engine_without_account_keeps_prefix", {"model": "deepseek/deepseek-v4", "effort": "low"})
    add("effort_from_account_settings", {"model": "claude-opus-5", "config_dir": "{ROOT}/cfg", "context_window": 200000},
        {"cfg/settings.json": {"text": json.dumps({"effortLevel": "medium"})},
         f"cfg/projects/-work-proj/{sid}.jsonl": {"text": usage}})
    add("effort_from_env", {"model": "claude-opus-5"}, env={"CLAUDE_CODE_EFFORT_LEVEL": "xhigh"})
    add("effort_from_home_settings", {"model": "claude-opus-5"},
        {"home/.claude/settings.json": {"text": json.dumps({"effortLevel": "max"})}})
    add("no_model_no_status", {"permission_mode": "default"})
    add("context_window_transcript_missing", {"model": "claude-opus-5", "context_window": 200000})
    add("context_window_transcript_without_usage", {"model": "claude-opus-5", "context_window": 200000},
        {home_t: {"text": json.dumps({"type": "user", "message": {"content": "oi"}}) + "\n"}})
    add("transcript_without_context_window", {"model": "claude-opus-5"}, {home_t: {"text": usage}})
    add("transcript_moved_to_worktree_folder", {"model": "claude-opus-5", "context_window": 1000000},
        {f"home/.claude/projects/-work-proj--worktrees-x/{sid}.jsonl": {"text": usage}})
    add("cwd_with_trailing_slash", {"model": "claude-opus-5", "context_window": 1000000, "cwd": "/work/proj/"}, {home_t: {"text": usage}})
    add("missing_sidecar_is_dead", None)
    return out


LOG_REL_CLAUDE = "home/.hangar/logs/privado/claude-headless-desconhecidos.jsonl"
CLAUDE_LOG = LOG_REL_CLAUDE
CODEX_LOG = "home/.hangar/logs/privado/codex-headless-desconhecidos.jsonl"


def write(out_dir):
    out_dir = Path(out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)
    os.environ["TZ"] = TZ
    time.tzset()
    groups = {"last_usage.json": last_usage_cases(), "reload_stamp.json": reload_cases(), "unknown_private.json": unknown_cases(),
              "prepare_prompt.json": prepare_cases(), "format_status_claude.json": status_cases(),
              "format_status_codex.json": codex_cases(), "skill_catalog.json": catalog_cases(),
              "parked_state.json": parked_cases()}
    for name, cases in groups.items():
        rows = [run_case(case) for case in cases]
        (out_dir / name).write_text(json.dumps({"tz": TZ, "cases": rows}, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    write(HERE / "local_policy")
