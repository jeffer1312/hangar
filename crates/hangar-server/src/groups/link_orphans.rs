//! Filha ligada que perdeu o pai (ele fechou ou morreu): a marca em `link-orphans.json`
//! (`hangar_delegation::orphans`), gravada em toda saída do grupo e tirada quando a filha fecha, é
//! religada ou some da lista.
use std::collections::BTreeMap;

use super::link_state::blocking;
use super::model::Sidecar;
use super::service::GroupService;
use super::store::file_stem;
use crate::routes::AppState;
use hangar_delegation::orphans;

const FAILED_EVENT: &str = "rust.link_orphan_failed";

/// Filha → pai pelos arquivos de grupo. O arquivo tem o nome saneado; o `harness` guarda o da sessão.
pub(crate) fn linked_parents(all: &[(String, Sidecar)]) -> BTreeMap<String, String> {
    all.iter().filter_map(|(stem, sidecar)| {
        let parent = sidecar.linked_parent()?;
        let name = sidecar.harness.keys().find(|n| file_stem(n) == *stem).cloned().unwrap_or_else(|| stem.clone());
        Some((name, parent.to_owned()))
    }).collect()
}

/// As filhas que vão perder o pai `name`, lidas antes da saída dele, que apaga o `linked_by` delas.
/// O pai sai com o nome que a filha guardou.
pub(crate) struct Leaving { parent: String, children: Vec<String> }

/// `by_stem`: só a varredura, que pode conhecer só o stem saneado do pai morto. Quem fecha sabe o nome
/// cru, e pelo stem levaria as filhas de outra sessão viva (`a b` para `a-b`).
pub(crate) async fn leaving(groups: &GroupService, name: &str, by_stem: bool) -> Result<Leaving, ()> {
    let all = groups.sidecars().await.map_err(|_| ())?;
    let links = linked_parents(&all);
    let exact = !by_stem || links.values().any(|p| p == name);
    let mut parent = name.to_owned();
    let children = links.into_iter().filter(|(_, p)| if exact { p == name } else { file_stem(p) == name })
        .map(|(child, p)| { parent = p; child }).collect();
    Ok(Leaving { parent, children })
}

/// Grava as órfãs da saída que já valeu. A marca é só o rótulo da linha: a falha vai ao diário, nunca à tela.
pub(crate) async fn mark(st: &AppState, name: &str, leaving: Result<Leaving, ()>) {
    let Ok(Leaving { parent, children }) = leaving else { return failed(st, name, "link_orphans_groups_unreadable", NOT_RECORDED) };
    if children.is_empty() { return; }
    let (dir, at) = (st.hangar_dir.clone(), crate::state::monitor::wall_now());
    let first = children[0].clone();
    match blocking(move || orphans::record(&dir, &parent, &children, at)).await {
        // A saída já mandou a lista reler; a marca chega depois dela.
        Ok(()) => st.list.invalidate(),
        Err(_) => failed(st, &first, "link_orphans_record_failed", NOT_RECORDED),
    }
}

/// A saída que falhou pode ter valido antes do erro (o contrato que não foi para o arquivo): marca só as filhas que
/// já perderam o vínculo; a saída desfeita deixa todas ligadas e não marca nada.
pub(crate) async fn mark_after_failed_leave(st: &AppState, groups: &GroupService, name: &str, leaving: Result<Leaving, ()>) {
    let Ok(leaving) = leaving else { return };
    if leaving.children.is_empty() { return; }
    let Ok(all) = groups.sidecars().await else { return failed(st, name, "link_orphans_groups_unreadable", NOT_RECORDED) };
    mark(st, name, Ok(unlinked(&linked_parents(&all), leaving))).await;
}

/// As filhas lidas antes da saída que não estão mais ligadas a `parent`.
fn unlinked(links: &BTreeMap<String, String>, Leaving { parent, children }: Leaving) -> Leaving {
    let children = children.into_iter().filter(|child| links.get(child) != Some(&parent)).collect();
    Leaving { parent, children }
}

/// A filha que fechou não é mais órfã de ninguém. Falha só vai ao diário: o fechamento já valeu.
pub(crate) async fn closed(st: &AppState, name: &str) {
    if clear(st, vec![name.to_owned()]).await.is_err() { failed(st, name, "link_orphans_clear_failed", "a marca de filha sem pai não saiu no fechamento"); }
}

/// Nome reusado por uma sessão nova: a marca da vida que acabou não passa para ela. Falha só vai ao
/// diário: a abertura segue.
pub async fn reused(st: &AppState, name: &str) {
    if clear(st, vec![name.to_owned()]).await.is_err() {
        st.diag.report(FAILED_EVENT, name, "link_orphans_clear_failed", "a marca de filha sem pai do nome reusado não saiu");
    }
}

/// Religadas a um pai novo: a marca antiga sai. Falha só vai ao diário: a ligação já valeu.
pub(crate) async fn relinked(st: &AppState, children: &[String]) {
    if clear(st, children.to_vec()).await.is_err() {
        st.diag.report(FAILED_EVENT, children.first().map_or("", String::as_str), "link_orphans_clear_failed", "a marca de filha sem pai não saiu na religação");
    }
}

/// Filha renomeada: a marca segue o nome novo. Falhar não desfaz a troca de nome, já feita.
pub(crate) async fn renamed(st: &AppState, old: &str, new: &str) {
    let (dir, from, to) = (st.hangar_dir.clone(), old.to_owned(), new.to_owned());
    if blocking(move || orphans::rename(&dir, &from, &to)).await.is_err() {
        st.diag.report(FAILED_EVENT, new, "link_orphans_rename_failed", "a marca de filha sem pai não seguiu o nome novo");
    }
}

/// As marcadas, para a varredura conferir se ainda estão na lista. Ilegível = nenhuma, no diário.
pub(crate) async fn marked(st: &AppState) -> Vec<String> {
    let dir = st.hangar_dir.clone();
    match blocking(move || orphans::read(&dir)).await {
        Ok(all) => all.into_keys().collect(),
        Err(_) => {
            if crate::warn_limit::allow(None, "link_orphans_unreadable") {
                st.diag.report(FAILED_EVENT, "", "link_orphans_unreadable", "marcas de filha sem pai ilegíveis");
            }
            Vec::new()
        }
    }
}

/// As que a varredura deu por fora da lista. Falha só vai ao diário: a próxima rodada tenta de novo.
pub(crate) async fn gone(st: &AppState, names: &[String]) {
    if clear(st, names.to_vec()).await.is_err() {
        st.diag.report(FAILED_EVENT, names.first().map_or("", String::as_str), "link_orphans_clear_failed", "a marca de filha sem pai que sumiu da lista não saiu");
    }
}

/// `orphans::clear` fora do runtime: E/S de disco não segura a tarefa.
async fn clear(st: &AppState, names: Vec<String>) -> std::io::Result<()> {
    let dir = st.hangar_dir.clone();
    blocking(move || orphans::clear(&dir, &names)).await
}

const NOT_RECORDED: &str = "a marca de filha sem pai não foi gravada";

fn failed(st: &AppState, name: &str, code: &str, reason: &'static str) {
    st.diag.report(FAILED_EVENT, name, code, reason);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_children_that_lost_the_link_are_marked_after_a_failed_leave() {
        let leaving = || Leaving { parent: "pai".into(), children: vec!["a".into(), "b".into()] };
        // A saída valeu antes do erro: as duas perderam o vínculo.
        assert_eq!(unlinked(&BTreeMap::new(), leaving()).children, ["a", "b"]);
        // A saída foi desfeita: as duas seguem ligadas ao pai.
        let restored = BTreeMap::from([("a".to_owned(), "pai".to_owned()), ("b".to_owned(), "pai".to_owned())]);
        assert!(unlinked(&restored, leaving()).children.is_empty());
        // Religada a outro pai no meio: também não é mais filha deste.
        let other = BTreeMap::from([("a".to_owned(), "outro".to_owned()), ("b".to_owned(), "pai".to_owned())]);
        assert_eq!(unlinked(&other, leaving()).children, ["a"]);
    }
}
