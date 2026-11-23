//! Linhas da `GetExtendedTcpTable` lidas como palavras de 32 bits, sem ponteiro: o buffer é
//! `[dwNumEntries, linha, linha, …]`, cada linha com tamanho fixo.
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// `MIB_TCPROW_OWNER_PID`: estado, endereço, porta, endereço remoto, porta remota, pid.
pub(crate) const V4_WORDS: usize = 6;
/// `MIB_TCP6ROW_OWNER_PID`: endereço (4), escopo, porta, remoto (4), escopo, porta, estado, pid.
pub(crate) const V6_WORDS: usize = 14;

/// `(endereço, porta, pid)` de cada linha; endereço e porta chegam na ordem da rede.
pub(crate) fn rows(table: &[u32], words: usize) -> Vec<(IpAddr, u16, u32)> {
    let Some((&count, rest)) = table.split_first() else { return Vec::new() };
    rest.chunks_exact(words).take(count as usize).map(|row| {
        let port = |w: u32| u16::from_be(w as u16);
        if words == V4_WORDS {
            (IpAddr::V4(Ipv4Addr::from(row[1].to_ne_bytes())), port(row[2]), row[5])
        } else {
            let mut bytes = [0u8; 16];
            for (chunk, word) in bytes.chunks_exact_mut(4).zip(&row[..4]) {
                chunk.copy_from_slice(&word.to_ne_bytes());
            }
            (IpAddr::V6(Ipv6Addr::from(bytes)), port(row[5]), row[13])
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_come_out_with_address_port_and_pid() {
        let ne = |b: [u8; 4]| u32::from_ne_bytes(b);
        // 127.0.0.1:3000 (0x0BB8 na ordem da rede) do pid 4242.
        let v4 = [1, 2, ne([127, 0, 0, 1]), ne([0x0B, 0xB8, 0, 0]), 0, 0, 4242];
        assert_eq!(rows(&v4, V4_WORDS), vec![("127.0.0.1".parse().unwrap(), 3000, 4242)]);
        // [::1]:5432 (0x1538) do pid 77.
        let v6 = [1, 0, 0, 0, ne([0, 0, 0, 1]), 0, ne([0x15, 0x38, 0, 0]), 0, 0, 0, 0, 0, 0, 2, 77];
        assert_eq!(rows(&v6, V6_WORDS), vec![("::1".parse().unwrap(), 5432, 77)]);
    }
}
