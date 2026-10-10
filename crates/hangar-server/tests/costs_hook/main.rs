//! Testes de custos que instalam o gancho de pânico do servidor (`install_panic_hook`): ele vale para o
//! processo inteiro e tiraria a mensagem de falha dos outros testes, por isso vivem fora do `it`.

#[path = "../it/common/mod.rs"]
mod common;

mod costs_collect;
mod costs_generation;
mod costs_index;
