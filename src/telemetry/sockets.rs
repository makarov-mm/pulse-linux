// SPDX-License-Identifier: GPL-3.0-or-later

//! Lists TCP and UDP sockets of this network namespace with the processes that hold them.

use std::collections::HashMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::i18n::mark;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn name(self) -> &'static str {
        match self {
            Self::Tcp => "TCP",
            Self::Udp => "UDP",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Socket {
    pub protocol: Protocol,
    pub local: SocketAddr,
    /// `None` for a socket that is not connected: a listener or an unconnected UDP socket.
    pub remote: Option<SocketAddr>,
    /// The TCP state in English, to be translated for display. Empty for UDP.
    pub state: &'static str,
    /// The kernel's socket inode, which tells apart sockets that share their addresses.
    pub inode: u64,
    /// The first process found holding the socket. `None` when its owner cannot be read,
    /// which is the case for other users' processes unless Pulse runs as root.
    pub pid: Option<i32>,
    pub process: String,
}

pub fn read() -> Vec<Socket> {
    let owners = owners();
    let mut sockets = Vec::new();
    for (path, protocol) in [
        ("/proc/net/tcp", Protocol::Tcp),
        ("/proc/net/tcp6", Protocol::Tcp),
        ("/proc/net/udp", Protocol::Udp),
        ("/proc/net/udp6", Protocol::Udp),
    ] {
        let Ok(table) = fs::read_to_string(path) else { continue };
        sockets.extend(table.lines().skip(1).filter_map(|line| parse(line, protocol, &owners)));
    }
    sockets
}

fn parse(line: &str, protocol: Protocol, owners: &HashMap<u64, i32>) -> Option<Socket> {
    // sl local_address rem_address st tx_queue:rx_queue tr:tm->when retrnsmt uid timeout inode ...
    let fields: Vec<&str> = line.split_ascii_whitespace().collect();
    let local = address(fields.get(1)?)?;
    let remote = address(fields.get(2)?)?;
    let state = u8::from_str_radix(fields.get(3)?, 16).ok()?;
    let inode = fields.get(9)?.parse().ok()?;
    let pid = owners.get(&inode).copied();
    Some(Socket {
        protocol,
        local,
        remote: (remote.port() != 0 || !remote.ip().is_unspecified()).then_some(remote),
        state: if protocol == Protocol::Tcp {
            tcp_state(state)
        } else {
            ""
        },
        inode,
        pid,
        process: pid
            .and_then(|pid| fs::read_to_string(format!("/proc/{pid}/comm")).ok())
            .map(|name| name.trim_end().to_owned())
            .unwrap_or_default(),
    })
}

/// Decodes `0100007F:0035` or its 32-digit IPv6 form.
///
/// The kernel prints each 32-bit word of the address as a native integer, so the bytes
/// of the parsed word in native order are the address bytes in network order.
fn address(text: &str) -> Option<SocketAddr> {
    let (host, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |index: usize| {
        Some(
            u32::from_str_radix(host.get(index * 8..index * 8 + 8)?, 16)
                .ok()?
                .to_ne_bytes(),
        )
    };
    let ip = match host.len() {
        8 => IpAddr::V4(Ipv4Addr::from(word(0)?)),
        32 => {
            let mut bytes = [0; 16];
            for index in 0..4 {
                bytes[index * 4..index * 4 + 4].copy_from_slice(&word(index)?);
            }
            // An IPv4 peer of a dual-stack socket reads better as plain IPv4.
            let ip = Ipv6Addr::from(bytes);
            ip.to_ipv4_mapped().map_or(IpAddr::V6(ip), IpAddr::V4)
        }
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

/// Names from the kernel's `tcp_states.h`.
fn tcp_state(state: u8) -> &'static str {
    match state {
        1 => mark("Established"),
        2 => mark("SYN sent"),
        3 => mark("SYN received"),
        4 => mark("FIN wait 1"),
        5 => mark("FIN wait 2"),
        6 => mark("Time wait"),
        7 => mark("Closed"),
        8 => mark("Close wait"),
        9 => mark("Last ACK"),
        10 => mark("Listening"),
        11 => mark("Closing"),
        _ => "",
    }
}

/// Maps socket inodes to a process that has the socket open, from the `socket:[inode]`
/// links in each readable `/proc/<pid>/fd`.
fn owners() -> HashMap<u64, i32> {
    let mut owners = HashMap::new();
    let Ok(processes) = fs::read_dir("/proc") else {
        return owners;
    };
    for pid in processes
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
    {
        let Ok(descriptors) = fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for descriptor in descriptors.flatten() {
            let Ok(target) = fs::read_link(descriptor.path()) else {
                continue;
            };
            let inode = target
                .to_str()
                .and_then(|target| target.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok());
            if let Some(inode) = inode {
                owners.entry(inode).or_insert(pid);
            }
        }
    }
    owners
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_decoded_from_native_words() {
        let loopback = u32::from_ne_bytes([127, 0, 0, 1]);
        assert_eq!(address(&format!("{loopback:08X}:0035")), "127.0.0.1:53".parse().ok());

        let mut words = String::new();
        for chunk in Ipv6Addr::LOCALHOST.octets().chunks(4) {
            words += &format!("{:08X}", u32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
        }
        assert_eq!(address(&format!("{words}:01BB")), "[::1]:443".parse().ok());
        assert_eq!(address("0100007F"), None);
    }

    #[test]
    fn a_listening_socket_of_this_process_is_found() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port is free");
        let local = listener.local_addr().expect("the listener has an address");
        let sockets = read();
        let own = sockets
            .iter()
            .find(|socket| socket.local == local && socket.protocol == Protocol::Tcp);
        let own = own.expect("the listener is listed");
        assert_eq!(
            (own.state, own.remote, own.pid),
            ("Listening", None, Some(std::process::id() as i32))
        );
    }
}
