use hangar_server::codex_setup::plugins::{self, Context, Runtime};
use std::{path::Path, sync::Mutex};
struct Fixture {
    name: String,
    calls: Mutex<Vec<String>>,
}
impl Runtime for Fixture {
    async fn prepare(&self, _root: &Path, _backups: &Path) -> Result<bool, String> {
        self.calls.lock().unwrap().push("ecc".into());
        if self.name == "ecc-failure" {
            Err("fixture ECC".into())
        } else {
            Ok(self.name == "ecc-ok")
        }
    }
}
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(48))]
    #[test]
    fn codex_plugins_reject_outside_cache_before_mutation(name in "[a-zA-Z0-9_-]{1,60}") {
        let root=crate::physical_tempdir();let account=root.path().join("account");let outside=root.path().join(format!("outside-{name}"));
        std::fs::create_dir_all(account.join("plugins/cache")).unwrap();std::fs::create_dir(&outside).unwrap();
        let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let runner=Fixture{name:"ecc-ok".into(),calls:Mutex::new(vec![])};
        let result=rt.block_on(plugins::adapt(&outside,&Context{account:&account,backups:&root.path().join("backups"),server:"/fixture/hangar-server",windows:false},|_|panic!("normalização fora do cache"),||panic!("alteração fora do cache"),&runner));
        proptest::prop_assert_eq!(result,Err("Plugin fora do cache do Codex".into()));
        proptest::prop_assert!(runner.calls.lock().unwrap().is_empty());
        proptest::prop_assert!(!root.path().join("backups").exists());
    }
}
