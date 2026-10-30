use super::*;

#[test]
fn administrative_file_reads_preserve_missing_and_broken_link_distinctions() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/native-file-reads.json")).unwrap();
    let mut differences=vec![];
    for case in fixture["cases"].as_array().unwrap(){
        let root=tempfile::tempdir().unwrap();let mut path=root.path().join("ação");
        match case["name"].as_str().unwrap(){
            "regular"=>fs::write(&path,[255,0]).unwrap(),
            "dangling"|"valid"=>{
                std::os::unix::fs::symlink("target",&path).unwrap();
                if case["name"]=="valid"{fs::write(root.path().join("target"),[255,0]).unwrap();}
            },
            "chain"=>{
                std::os::unix::fs::symlink("middle",&path).unwrap();
                std::os::unix::fs::symlink("absent",root.path().join("middle")).unwrap();
            },
            "missing_parent"=>path=root.path().join("absent/ação"),
            "missing"=>{},other=>panic!("Caso desconhecido: {other}"),
        }
        let actual=match read(&path){
            Ok(raw)=>json!({"ok":raw.map(|bytes|bytes.iter().map(|byte|format!("{byte:02x}")).collect::<String>())}),
            Err(error)=>json!({"class":match &error{Error::Io(_)=>"OSError",Error::Invalid(_)=>"ValueError",Error::Changed(_)|Error::Displaced(_)=>"AlteradoExternamente"},"error":error.to_string().replace(root.path().to_str().unwrap(),"$ROOT")}),
        };
        assert_eq!(path.is_symlink(),case["symlink"].as_bool().unwrap());
        if actual!=case["result"]{differences.push((case["name"].clone(),actual,case["result"].clone()));}
    }
    assert!(differences.is_empty(),"Leitura administrativa divergente: {differences:?}");
}
