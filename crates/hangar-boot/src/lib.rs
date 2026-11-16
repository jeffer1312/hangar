//! Subida do `hangar-server` sem o Python na frente: configuração, vida do processo e aviso ao systemd.
pub mod life;
pub mod notify;
pub mod settings;

/// O Python que ainda sobe o servidor como filho passa o endereço dele; sem isso o servidor é o
/// processo do serviço e se configura sozinho.
pub fn python_parent() -> bool {
    parent_from(std::env::var("HANGAR_SERVER_UPSTREAM").ok())
}

/// Vazio vale ausente, como no `Config`.
pub fn parent_from(upstream: Option<String>) -> bool {
    upstream.is_some_and(|value| !value.is_empty())
}
