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

/// Identidade textual de configuração: letra minúscula inicial, seguida de letras, números ou hífen.
pub fn valid_slug(value: &str, max: usize) -> bool {
    let mut bytes = value.bytes();
    value.len() <= max && bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes.all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}
