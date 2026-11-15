"""Controle do arquivo privado (0600 / DACL) usado nas provas do cofre do login OAuth do ChatGPT."""
import json
import os

import pytest


def _write_private(path, data):
    path.write_text(json.dumps(data))
    os.chmod(path, 0o600)


def _assert_private_file(path):
    """No Windows, compara a DACL com a escrita atômica da base no mesmo diretório."""
    import hashlib
    import inspect
    import os
    import subprocess

    assert path.is_file()
    assert path.read_bytes(), "O arquivo persistido não pôde ser lido"
    if os.name != "nt":
        assert oct(path.stat().st_mode & 0o777) == "0o600"
        return
    baseline = path.with_name(path.name + ".acl-baseline")
    _write_private(baseline, {"controle": "acl"})
    script = r"""
$ErrorActionPreference = 'Stop'
$paths = [Console]::In.ReadToEnd() | ConvertFrom-Json
$items = @(foreach ($path in $paths) {
    $acl = Get-Acl -LiteralPath $path
    $rules = @($acl.Access | ForEach-Object {
        @{ sid = $_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
           rights = [long]$_.FileSystemRights; type = $_.AccessControlType.ToString() }
    })
    @{ owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
       dacl = $acl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::Access)
       rules = $rules }
})
@{ identity = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value
   files = $items } | ConvertTo-Json -Compress -Depth 5
"""
    try:
        result = subprocess.run(
            ["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
            input=json.dumps([str(baseline), str(path)]), text=True,
            capture_output=True, timeout=20, check=True,
        )
        proof = json.loads(result.stdout)
        base, candidate = proof["files"]
        assert base["dacl"] and candidate["dacl"], "Consulta nativa não retornou DACL"
        assert base["rules"] and candidate["rules"], "DACL vazia não prova proteção"
        assert candidate["owner"] == base["owner"], "A gravação mudou o proprietário"
        def rights(item, kind):
            masks = {}
            for rule in item["rules"]:
                if rule["type"] == kind:
                    masks[rule["sid"]] = masks.get(rule["sid"], 0) | (rule["rights"] & 0xffffffff)
            return masks
        base_allow, candidate_allow = rights(base, "Allow"), rights(candidate, "Allow")
        base_deny, candidate_deny = rights(base, "Deny"), rights(candidate, "Deny")
        assert all(mask & ~base_allow.get(sid, 0) == 0 for sid, mask in candidate_allow.items()), "A gravação ampliou direitos"
        assert all(mask & ~candidate_deny.get(sid, 0) == 0 for sid, mask in base_deny.items()), "A gravação retirou restrições"
        proof["writer_sha256"] = hashlib.sha256(inspect.getsource(_write_private).encode()).hexdigest()
        proof["method"] = "DACL por SID/máscara; mesmo diretório, identidade e processo"
        print("Prova nativa de ACL: " + json.dumps(proof, sort_keys=True))
    finally:
        baseline.unlink()


def test_private_file_control_rejects_real_permission_expansion(tmp_path):
    import os
    import subprocess
    path = tmp_path / "controle.json"
    _write_private(path, {"controle": "acl"})
    if os.name == "nt":
        script = r"""
$ErrorActionPreference = 'Stop'
$path = [Console]::In.ReadToEnd()
$acl = Get-Acl -LiteralPath $path
$sid = New-Object Security.Principal.SecurityIdentifier('S-1-1-0')
$rule = New-Object Security.AccessControl.FileSystemAccessRule($sid, 'Read', 'Allow')
$acl.AddAccessRule($rule)
Set-Acl -LiteralPath $path -AclObject $acl
"""
        subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", script],
                       input=str(path), text=True, capture_output=True, timeout=20, check=True)
        with pytest.raises(AssertionError, match="ampliou direitos"):
            _assert_private_file(path)
    else:
        path.chmod(0o644)
        with pytest.raises(AssertionError):
            _assert_private_file(path)
