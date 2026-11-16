//! Subida do `hangar-server` sem o Python na frente: configuração, vida do processo e aviso ao systemd.
pub mod life;
pub mod notify;
pub mod settings;

/// O Python que ainda sobe o servidor como filho passa o endereço dele; sem isso o servidor é o
/// processo do serviço e se configura sozinho.
pub fn python_parent() -> bool {
    std::env::var_os("HANGAR_SERVER_UPSTREAM").is_some()
}
