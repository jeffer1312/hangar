"""Prova isolada da seleção e invocação reais do doctor do instalador nativo."""
import os,pathlib,subprocess,tempfile
root=pathlib.Path(__file__).resolve().parents[1]
source=(root/'desktop-native/src/app/setup/report.rs').read_text()
start=source.index('pub(crate) fn doctor_paths(')
end=source.index('fn run_with_timeout(',start)
fragment=source[start:end]
# O executor de prazo já tem testes no módulo; aqui capturamos o comando montado.
harness=r'''
use std::{path::{Path,PathBuf},process::Command,time::Duration};
#[derive(Clone,Copy)] enum Mode {Git,Package}
const DOCTOR_TIMEOUT:Duration=Duration::from_secs(40);
fn hidden(command:&mut Command)->&mut Command{command}
fn refreshed_path()->String{std::env::var("PATH").unwrap()}
fn run_with_timeout(command:Command,limit:Duration)->String {
 assert_eq!(limit,Duration::from_secs(40));
 format!("{}|{}|{}",command.get_program().to_string_lossy(),command.get_args().map(|v|v.to_string_lossy()).collect::<Vec<_>>().join(";"),command.get_current_dir().unwrap().display())
}
'''+fragment+r'''
#[test] fn native_doctor_uses_its_installation_without_python(){
 let root=PathBuf::from(std::env::var("FIXTURE_ROOT").unwrap());
 for (mode,windows,expected) in [(Mode::Git,false,"crates/target/release/hangar-server"),(Mode::Git,true,"crates/target/release/hangar-server.exe"),(Mode::Package,false,"current/bin/hangar-server"),(Mode::Package,true,"current/bin/hangar-server.exe")] {
  let (binary,backend)=doctor_paths(mode,&root,windows);
  assert_eq!(binary,root.join(expected));
  assert_eq!(backend,root.join(if matches!(mode,Mode::Package){"current/backend"}else{"backend"}));
 }
 let (binary,backend)=doctor_paths(Mode::Package,&root,false);
 std::fs::create_dir_all(binary.parent().unwrap()).unwrap();std::fs::write(&binary,b"fixture").unwrap();
 assert_eq!(doctor(Mode::Package,&root),Some(format!("{}|doctor;--backend;{}|{}",binary.display(),backend.display(),backend.display())));
}
'''
with tempfile.TemporaryDirectory() as temp:
 directory=pathlib.Path(temp);rust=directory/'test.rs';rust.write_text(harness)
 binary=directory/'test';subprocess.run(['rustc','--edition=2024','--test',str(rust),'-o',str(binary)],check=True)
 result=subprocess.run([str(binary),'--nocapture'],env={**os.environ,'FIXTURE_ROOT':temp,'HOME':temp,'USERPROFILE':temp})
 raise SystemExit(result.returncode)
