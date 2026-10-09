"""Preserva OAuth omp anterior através do escritor real Rust."""
import hashlib
import json
import os
import sqlite3
from contextlib import closing
from pathlib import Path
from urllib.request import Request

import pytest

from test_oauth_codex import _jwt, _wait_legacy, managed_device  # noqa: F401


def _private_device(server, action):
    health = server.request("GET", "/__hangar_server/health")
    assert health.status_code == 200
    address = health.json()["terminal_address"]
    request = Request(
        "http://" + address + "/__hangar_server/accounts",
        data=json.dumps({"device_action": action}).encode(), method="POST",
        headers={"Content-Type": "application/json",
                 "x-hangar-internal": "contract-internal"},
    )
    with server.opener.open(request, timeout=30) as response:
        assert response.status == 200
        return json.loads(response.read())


def _snapshot_omp(database):
    with closing(sqlite3.connect(database.as_uri() + "?mode=ro", uri=True)) as connection:
        rows = connection.execute(
            "SELECT id,provider,credential_type,data,identity_key,disabled_cause "
            "FROM auth_credentials ORDER BY id"
        ).fetchall()
    return rows, database.read_bytes()


@pytest.mark.parametrize("flow", ["login", "propagate"])
@pytest.mark.parametrize("has_previous_oauth", [True, False],
                         ids=["oauth-anterior", "somente-chave-opencode"])
def test_legacy_omp_oauth_preservation(flow, has_previous_oauth, managed_device):
    fixture = managed_device
    root = fixture.reference.root
    database = root / ".omp/agent/agent.db"
    previous = json.dumps({"access": _jwt("omp-before"),
                           "refresh": "synthetic-omp-before", "expires": 4102444800000,
                           "accountId": "omp-before", "extension": "conteúdo anterior"},
                          ensure_ascii=False, indent=2)
    with closing(sqlite3.connect(database)) as connection:
        connection.execute("ALTER TABLE auth_credentials ADD COLUMN disabled_cause TEXT")
        connection.execute(
            "INSERT INTO auth_credentials VALUES (23,'opencode','api_key',?,NULL,NULL)",
            ('{"key":"synthetic-opencode-control"}',),
        )
        if has_previous_oauth:
            connection.execute(
                "INSERT INTO auth_credentials VALUES (17,'openai-codex','oauth',?,'omp-before',NULL)",
                (previous,),
            )
        connection.commit()
    before_rows, before_bytes = _snapshot_omp(database)
    original_codex = json.dumps({"tokens": {"access_token": _jwt("codex-before"),
                                            "refresh_token": "synthetic-codex-before",
                                            "account_id": "codex-before"},
                                 "extension": "preservada"}).encode()
    (root / ".codex/auth.json").write_bytes(original_codex)
    (root / ".pi/agent/auth.json").write_text("{}", encoding="utf-8")
    if flow == "login":
        fixture.grant.set()
        response = fixture.server.request("POST", "/api/credenciais/codex/login")
        assert response.status_code == 200
        result = _wait_legacy(fixture.server, "concluido")["resultado"]
    else:
        vault = root / ".hangar/auth/openai-codex.json"
        vault.parent.mkdir(parents=True, exist_ok=True)
        vault.write_text(json.dumps({"access": _jwt(), "refresh": "fixture-refresh",
                                     "id_token": "fixture-id", "expires_ms": 4102444800000,
                                     "account_id": "acc-1", "plano": "plus"}), encoding="utf-8")
        result = _private_device(fixture.server, "propagate")
    assert result == {
        "codex": {"ok": True, "motivo": "ja-logado"},
        "pi": {"ok": True, "motivo": str(root / ".pi/agent/auth.json")},
        "omp": {"ok": True, "motivo": "ja-logado" if has_previous_oauth else str(database)},
    }
    after_rows, after_bytes = _snapshot_omp(database)
    identical_rows = after_rows == before_rows
    identical_bytes = after_bytes == before_bytes
    if has_previous_oauth:
        assert identical_rows, "A tabela anterior foi alterada ou duplicada"
        assert identical_bytes, "Os bytes do banco anterior foram alterados"
    else:
        assert len(after_rows) == len(before_rows) + 1
        key_preserved = [row for row in after_rows if row[0] == 23] == before_rows
        assert key_preserved, "A chave de controle foi alterada"
        oauth = [row for row in after_rows if row[1:3] == ("openai-codex", "oauth")]
        assert len(oauth) == 1 and oauth[0][4] == "acc-1"
        assert json.loads(oauth[0][3]) == {
            "access": _jwt(), "refresh": "fixture-refresh", "expires": 4102444800000,
            "accountId": "acc-1",
        }
    codex_preserved = (root / ".codex/auth.json").read_bytes() == original_codex
    assert codex_preserved, "A credencial Codex anterior foi alterada"
    vault = json.loads((root / ".hangar/auth/openai-codex.json").read_text(encoding="utf-8"))
    assert vault["account_id"] == "acc-1" and vault["refresh"] == "fixture-refresh"
    pi_path = root / ".pi/agent/auth.json"
    pi_before_repeat = pi_path.read_bytes()
    assert json.loads(pi_before_repeat)["openai-codex"] == {
        "type": "oauth", "access": _jwt(), "refresh": "fixture-refresh",
        "expires": 4102444800000, "accountId": "acc-1",
    }
    address = fixture.server.request("GET", "/__hangar_server/health").json()["terminal_address"]
    assert fixture.reference.request("POST", "/__contract__/claude-owner",
                                     {"address": address, "mode": "rust"}).status_code == 200
    repair = fixture.reference.request("POST", "/__contract__/device-repair", {"action": "oauth"})
    assert repair.status_code == 200
    parts = repair.json()["result"].split("; ")
    assert len(parts) == 3
    assert dict(part.split(": ", 1) for part in parts) == {
        "codex": "ja-logado", "pi": "ja-logado", "omp": "ja-logado",
    }
    repeated = _private_device(fixture.server, "propagate")
    assert repeated == {name: {"ok": True, "motivo": "ja-logado"}
                        for name in ("codex", "pi", "omp")}
    final_rows, final_bytes = _snapshot_omp(database)
    repeated_rows_preserved = final_rows == after_rows
    repeated_bytes_preserved = final_bytes == after_bytes
    assert repeated_rows_preserved and repeated_bytes_preserved, "A repetição mudou o banco"
    assert pi_path.read_bytes() == pi_before_repeat
    assert (root / ".codex/auth.json").read_bytes() == original_codex
    state = fixture.server.request("GET", "/api/credenciais/codex")
    assert state.status_code == 200 and state.json() == {
        "cofre": True, "plano": "plus", "expira_em": 4102444800000,
        "codex": True, "pi": True, "omp": True,
    }
    calls = fixture.reference.calls()
    # O destino Pi/omp é escrito pelo próprio Rust: nenhuma rota da ponte aposentada pode aparecer.
    retired = [row for row in calls if row["operation"] == "bridge.propagate_device_login"]
    assert not retired, "A escrita Pi/omp ainda passou pela ponte Python"
    public = json.dumps([result, repeated, repair.json(), state.json(), calls])
    for secret in (_jwt(), _jwt("omp-before"), "fixture-refresh", "fixture-id",
                   "synthetic-omp-before", "synthetic-codex-before", "synthetic-opencode-control"):
        assert secret not in public
    evidence = os.environ.get("HANGAR_PROOF_EVIDENCE")
    if evidence:
        destination = Path(evidence)
        destination.mkdir(parents=True, exist_ok=True)
        record = {"flow": flow, "previous_oauth": has_previous_oauth,
                  "initial_rows": len(before_rows), "final_rows": len(final_rows),
                  "preserved_identity": "omp-before" if has_previous_oauth else "acc-1",
                  "before_sha256": hashlib.sha256(before_bytes).hexdigest(),
                  "after_sha256": hashlib.sha256(after_bytes).hexdigest(),
                  "final_sha256": hashlib.sha256(final_bytes).hexdigest(),
                  "result": result, "repeated_result": repeated,
                  "retired_bridge_calls": len(retired),
                  "row_index": {stage: [
                      {"id": row[0], "provider": row[1], "credential_type": row[2],
                       "data_sha256": hashlib.sha256(row[3].encode()).hexdigest(),
                       "identity_key": row[4], "disabled_cause": row[5]}
                      for row in rows]
                      for stage, rows in (("before", before_rows), ("after", after_rows),
                                          ("final", final_rows))},
                  "row_preservation": identical_rows,
                  "byte_preservation": identical_bytes, "codex_preserved": codex_preserved}
        (destination / f"omp-{flow}-{has_previous_oauth}.json").write_text(
            json.dumps(record, ensure_ascii=False, indent=2), encoding="utf-8")
