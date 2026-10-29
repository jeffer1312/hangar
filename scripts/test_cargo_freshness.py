"""Prova real de duas árvores com uma biblioteca local e target compartilhado."""
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT=Path(__file__).resolve().parents[1]


def test_cargo_pinned_rebuilds_local_library_from_other_tree(tmp_path):
    target=Path(os.environ.get('CARGO_TARGET_DIR',tmp_path/'shared-target'))
    roots=[]
    for name in ['first','second']:
        root=tmp_path/name;roots.append(root)
        (root/'scripts').mkdir(parents=True)
        for script in ['cargo-pinned','pin-auth-pair.py','cargo-freshness.py']:
            if (ROOT/'scripts'/script).exists():shutil.copyfile(ROOT/'scripts'/script,root/'scripts'/script)
        subprocess.run(['git','init','-q',str(root)],check=True)
        workspace=root/'crates';workspace.mkdir()
        (workspace/'Cargo.toml').write_text('[workspace]\nmembers=["library","app"]\nresolver="3"\n')
        for package in ['library','app']:(workspace/package/'src').mkdir(parents=True)
        (workspace/'library/Cargo.toml').write_text('[package]\nname="cargo-freshness-library"\nversion="0.1.0"\nedition="2024"\n[lib]\ncrate-type=["rlib","cdylib"]\n')
        (workspace/'library/src/lib.rs').write_text(f'pub fn value()->&\'static str{{"{name}"}}\n')
        (workspace/'app/Cargo.toml').write_text('[package]\nname="cargo-freshness-app"\nversion="0.1.0"\nedition="2024"\n[dependencies]\ncargo-freshness-library={path="../library"}\n')
        (workspace/'app/src/main.rs').write_text('fn main(){println!("{}",cargo_freshness_library::value());}\n')
    for path in (roots[1]/'crates').rglob('*'):
        if path.is_file():os.utime(path,(time.time()-600,time.time()-600))
    def build(root):
        result=subprocess.run(['bash',str(root/'scripts/cargo-pinned'),'build','--offline','-p','cargo-freshness-app'],
                              cwd=root,env={**os.environ,'CARGO_TARGET_DIR':str(target)},capture_output=True,text=True,timeout=120)
        assert result.returncode==0,result.stdout+result.stderr
        executable=root/'crates/target/debug/cargo-freshness-app'
        actual=subprocess.check_output([str(executable)],text=True).strip()
        return actual,result
    first,_=build(roots[0]);assert first=='first'
    before=(roots[0]/'crates/target/debug/cargo-freshness-app').read_bytes()
    second,_=build(roots[1])
    assert second=='second','o Cargo reutilizou a biblioteca local de outra árvore'
    assert (roots[0]/'crates/target/debug/cargo-freshness-app').read_bytes()==before
    repeated,result=build(roots[1]);assert repeated=='second'
    assert 'Compiling cargo-freshness-library' not in result.stderr,'a mesma árvore deve conservar seu cache'


def test_freshness_keeps_external_artifacts_and_same_tree_cache(tmp_path):
    import importlib.util
    spec=importlib.util.spec_from_file_location('cargo_freshness',ROOT/'scripts/cargo-freshness.py')
    helper=importlib.util.module_from_spec(spec);spec.loader.exec_module(helper)
    target=tmp_path/'target';build=tmp_path/'build'
    local=build/'debug/.fingerprint/local-package-012abc'
    cross=build/'triple/release/.fingerprint/local-package-012abc'
    dependency=build/'debug/.fingerprint/local-dependency-012abc'
    external=build/'debug/.fingerprint/serde-012abc'
    similar=build/'debug/.fingerprint/local-package-extra-012abc'
    artifact=build/'debug/deps/library.rlib'
    for path in [local,cross,dependency,external,similar]:path.mkdir(parents=True);(path/'marker').write_text('preservado')
    artifact.parent.mkdir(parents=True,exist_ok=True);artifact.write_bytes(b'artifact')
    metadata={'workspace_root':str(tmp_path/'first'),'target_directory':str(target),'build_directory':str(build),
              'packages':[{'name':'local-package','source':None,'dependencies':[{'name':'local-dependency','path':'local'}]}]}
    assert helper.prepare(metadata,target)==3
    assert not local.exists() and not cross.exists() and not dependency.exists()
    assert external.is_dir() and similar.is_dir() and artifact.read_bytes()==b'artifact'
    local.mkdir();(local/'marker').write_text('compilado')
    assert helper.prepare(metadata,target)==0 and local.is_dir()
    assert helper.prepare({**metadata,'workspace_root':str(tmp_path/'second')},target)==1
    assert not local.exists() and external.is_dir() and artifact.read_bytes()==b'artifact'
