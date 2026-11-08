mod placeholder;
pub use placeholder::has_placeholder;

pub mod body;
mod terminal;
mod terminal_windows;
pub(crate) mod guest;
mod project;
mod focus;
pub mod http;

pub(crate) async fn close_for_session<F>(st:&crate::routes::AppState,name:&str,guard:F)->Vec<serde_json::Value> where F:Fn()->futures_util::future::BoxFuture<'static,bool> {guest::service(st).await.close_for_session(name,guard).await}
