//! Identificadores aleatórios.

pub fn random_hex(bytes: usize) -> String {
    use ring::rand::SecureRandom;
    let mut buffer = vec![0u8; bytes];
    ring::rand::SystemRandom::new().fill(&mut buffer).expect("fonte de aleatoriedade do sistema");
    buffer.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Nome de sessão do Hangar: 1 a 64 bytes de `[A-Za-z0-9._-]`.
pub fn valid_session_name(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}
