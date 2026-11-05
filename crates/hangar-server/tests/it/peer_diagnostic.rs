#[test]
fn update_declared_steps_remain_valid_and_unique_on_both_platforms() {
 let root=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/atualizacoes");
 assert!(root.is_dir());
 for windows in [false,true] {
  let (steps,invalid)=hangar_server::update::steps::read_steps(&root,windows);
  assert!(invalid.is_empty(),"passos inválidos: {invalid:?}");
  let mut ids=std::collections::HashSet::new();
  for step in steps {assert!(ids.insert(step.id.clone()),"id repetido: {}",step.id);}
 }
}
