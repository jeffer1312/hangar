use std::collections::HashSet;
use std::io;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER};
use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER};
use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};

use crate::win_table::{V4_WORDS, V6_WORDS, rows};
use crate::{Listener, Outside, Scan, reachable_locally, sorted};

pub(crate) fn scan(candidates: &[u32]) -> io::Result<Scan> {
    let wanted: HashSet<u32> = candidates.iter().copied().collect();
    let mut found = rows(&table(AF_INET)?, V4_WORDS);
    found.extend(rows(&table(AF_INET6)?, V6_WORDS));
    let (mine, others): (Vec<_>, Vec<_>) = found.into_iter()
        .filter(|&(addr, _, _)| reachable_locally(addr))
        .partition(|&(_, _, pid)| wanted.contains(&pid));
    let mut outside: Vec<(u16, std::net::IpAddr)> = others.into_iter().map(|(addr, port, _)| (port, addr)).collect();
    outside.sort_unstable();
    Ok(Scan {
        listeners: sorted(mine.into_iter().map(|(addr, port, pid)| Listener { port, pid, process: image_name(pid), addr })),
        outside: outside.into_iter().map(|(port, addr)| Outside { port, inode: None, addr }).collect(),
    })
}

fn table(family: u16) -> io::Result<Vec<u32>> {
    let mut size = 0u32;
    // A tabela pode crescer entre o pedido do tamanho e a leitura.
    for _ in 0..4 {
        let mut buffer = vec![0u32; (size as usize).div_ceil(4)];
        let pointer = if buffer.is_empty() { std::ptr::null_mut() } else { buffer.as_mut_ptr().cast() };
        // SAFETY: `pointer` é nulo ou aponta para `size` bytes alinhados a u32, como as linhas.
        let result = unsafe { GetExtendedTcpTable(pointer, &mut size, 0, family.into(), TCP_TABLE_OWNER_PID_LISTENER, 0) };
        match result {
            0 => return Ok(buffer),
            ERROR_INSUFFICIENT_BUFFER => continue,
            code => return Err(io::Error::from_raw_os_error(code as i32)),
        }
    }
    Err(io::Error::other("tcp_table_kept_growing"))
}

/// Nome do executável sem `.exe`, como o `comm` do Linux; vazio quando o processo não se deixa abrir.
fn image_name(pid: u32) -> String {
    let mut buffer = [0u16; 1024];
    let mut len = buffer.len() as u32;
    // SAFETY: o handle só é usado se não for nulo e é fechado logo; `buffer` tem `len` posições.
    let ok = unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return String::new();
        }
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        ok != 0
    };
    if !ok {
        return String::new();
    }
    let path = String::from_utf16_lossy(&buffer[..len as usize]);
    Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}
