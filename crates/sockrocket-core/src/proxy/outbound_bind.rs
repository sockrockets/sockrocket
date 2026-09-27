//! Binds sockrocket's own outbound sockets to the physical network interface while
//! TUN mode is active.
//!
//! When TUN routes are installed, the system default route points into the
//! TUN device. Sockets created by sockrocket itself — above all `DirectOutbound`
//! connections for direct-routed (e.g. China) destinations — would follow
//! that default route straight back into the TUN, creating an infinite
//! session loop: SYN → TUN → new ipstack session → new SYN → … Direct
//! traffic dies; long-established pre-TUN connections keep working for a
//! while, which is why the failure surfaces as "direct stops working after
//! TUN has been on for some time".
//!
//! The standard fix (used by every TUN client) is to bind outbound sockets
//! to the physical interface: `IP_UNICAST_IF` on Windows,
//! `SO_BINDTODEVICE` on Linux, `IP_BOUND_IF` on macOS. This module holds
//! the interface to bind to (set when TUN routes are installed, cleared
//! when they are torn down) and applies it at socket-creation time.

use std::num::NonZeroU32;
use std::sync::RwLock;

/// Interface to bind outbound sockets to while TUN is active.
#[derive(Debug, Clone, Default)]
pub struct OutboundBind {
    /// Physical interface index for IPv4 (`IP_UNICAST_IF` / `IP_BOUND_IF`).
    pub if_index_v4: Option<NonZeroU32>,
    /// Physical interface index for IPv6 (`IPV6_UNICAST_IF` / `IPV6_BOUND_IF`).
    pub if_index_v6: Option<NonZeroU32>,
    /// Physical interface name (`SO_BINDTODEVICE` on Linux).
    pub if_name: Option<String>,
}

static OUTBOUND_BIND: RwLock<Option<OutboundBind>> = RwLock::new(None);

/// Activate outbound binding (called when TUN routes are installed).
pub fn set_outbound_bind(bind: OutboundBind) {
    if let Ok(mut guard) = OUTBOUND_BIND.write() {
        *guard = Some(bind);
    }
}

/// Deactivate outbound binding (called when TUN routes are torn down).
pub fn clear_outbound_bind() {
    if let Ok(mut guard) = OUTBOUND_BIND.write() {
        *guard = None;
    }
}

/// Current binding, if any.
pub fn outbound_bind() -> Option<OutboundBind> {
    OUTBOUND_BIND.read().ok().and_then(|g| g.clone())
}

/// Apply the active binding to a `tokio::net::TcpSocket` before connect.
/// No-op when TUN is not active.
pub fn apply_to_tcp_socket(socket: &tokio::net::TcpSocket, is_v6: bool) -> std::io::Result<()> {
    let Some(bind) = outbound_bind() else {
        return Ok(());
    };
    apply(socket, &bind, is_v6)
}

#[cfg(windows)]
fn apply(socket: &tokio::net::TcpSocket, bind: &OutboundBind, is_v6: bool) -> std::io::Result<()> {
    use std::os::windows::io::AsRawSocket;

    // ws2_32 is already loaded/linked by std::net; declare the one symbol we
    // need instead of taking a windows-sys dependency.
    #[link(name = "ws2_32")]
    unsafe extern "system" {
        fn setsockopt(s: usize, level: i32, optname: i32, optval: *const u8, optlen: i32) -> i32;
    }

    const IPPROTO_IP: i32 = 0;
    const IPPROTO_IPV6: i32 = 41;
    /// IP_UNICAST_IF / IPV6_UNICAST_IF — both take the interface index as a
    /// DWORD in network byte order (per MS docs).
    const UNICAST_IF: i32 = 31;

    // Fall back to the v4 index for v6 sockets when no v6 index is known —
    // same physical interface in practice.
    let index = (if is_v6 {
        bind.if_index_v6
    } else {
        bind.if_index_v4
    })
    .or(bind.if_index_v4);
    let Some(index) = index else {
        return Ok(());
    };

    let level = if is_v6 { IPPROTO_IPV6 } else { IPPROTO_IP };
    let value = index.get().to_be_bytes();
    let rc = unsafe {
        setsockopt(
            socket.as_raw_socket() as usize,
            level,
            UNICAST_IF,
            value.as_ptr(),
            value.len() as i32,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn apply(socket: &tokio::net::TcpSocket, bind: &OutboundBind, _is_v6: bool) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    // SO_BINDTODEVICE is family-agnostic; bind by interface name. Raw
    // setsockopt instead of socket2::SockRef::bind_device, which is gated
    // behind socket2's (unenabled) "all" feature.
    let Some(name) = &bind.if_name else {
        return Ok(());
    };
    unsafe extern "C" {
        fn setsockopt(s: i32, level: i32, optname: i32, optval: *const u8, optlen: u32) -> i32;
    }
    const SOL_SOCKET: i32 = 1;
    const SO_BINDTODEVICE: i32 = 25;

    let value = name.as_bytes();
    let rc = unsafe {
        setsockopt(
            socket.as_raw_fd(),
            SOL_SOCKET,
            SO_BINDTODEVICE,
            value.as_ptr(),
            value.len() as u32,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn apply(socket: &tokio::net::TcpSocket, bind: &OutboundBind, is_v6: bool) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    // IP_BOUND_IF / IPV6_BOUND_IF — raw setsockopt instead of
    // socket2::SockRef::bind_device_by_index_*, which need socket2's "all" feature.
    let index = if is_v6 {
        bind.if_index_v6
    } else {
        bind.if_index_v4
    };
    let Some(index) = index else {
        return Ok(());
    };
    unsafe extern "C" {
        fn setsockopt(s: i32, level: i32, optname: i32, optval: *const u8, optlen: u32) -> i32;
    }
    const IPPROTO_IP: i32 = 0;
    const IPPROTO_IPV6: i32 = 41;
    const IP_BOUND_IF: i32 = 25;
    const IPV6_BOUND_IF: i32 = 125;

    let level = if is_v6 { IPPROTO_IPV6 } else { IPPROTO_IP };
    let optname = if is_v6 { IPV6_BOUND_IF } else { IP_BOUND_IF };
    let value = index.get();
    let rc = unsafe {
        setsockopt(
            socket.as_raw_fd(),
            level,
            optname,
            (&value as *const u32).cast::<u8>(),
            std::mem::size_of_val(&value) as u32,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn apply(
    _socket: &tokio::net::TcpSocket,
    _bind: &OutboundBind,
    _is_v6: bool,
) -> std::io::Result<()> {
    Ok(())
}

/// Look up the interface index for the interface that owns `if_name`
/// (Windows: friendly name; falls back to matching the interface IPv4
/// address when `if_ip` is given). Used when installing TUN routes.
#[cfg(windows)]
pub fn interface_index_by_name(if_name: &str) -> Option<NonZeroU32> {
    interface_index_windows(if_name, None)
}

/// Look up the interface index of the adapter that owns the local IPv4
/// address `ip`. TUN route setup on Windows learns the physical interface
/// as a local IP (from `route print`), not a name.
#[cfg(windows)]
pub fn interface_index_by_ip(ip: std::net::Ipv4Addr) -> Option<NonZeroU32> {
    interface_index_windows("", Some(ip))
}

/// macOS: resolve an interface name ("en0") to its index for IP_BOUND_IF.
#[cfg(target_os = "macos")]
pub fn interface_index_by_name(name: &str) -> Option<NonZeroU32> {
    unsafe extern "C" {
        fn if_nametoindex(name: *const core::ffi::c_char) -> u32;
    }
    let c_name = std::ffi::CString::new(name).ok()?;
    NonZeroU32::new(unsafe { if_nametoindex(c_name.as_ptr()) })
}

/// Windows: resolve `if_name` (or the interface owning `if_ip`) to an
/// interface index via GetAdaptersAddresses — locale-independent, unlike
/// parsing `netsh` output.
#[cfg(windows)]
fn interface_index_windows(if_name: &str, if_ip: Option<std::net::Ipv4Addr>) -> Option<NonZeroU32> {
    #![allow(non_snake_case, non_camel_case_types, clippy::upper_case_acronyms)]

    type ULONG = u32;
    #[repr(C)]
    struct SOCKADDR {
        sa_family: u16,
        sa_data: [u8; 14],
    }
    #[repr(C)]
    struct SOCKET_ADDRESS {
        lp_sockaddr: *mut SOCKADDR,
        i_sockaddr_length: i32,
    }
    #[repr(C)]
    struct IP_ADAPTER_UNICAST_ADDRESS {
        length: ULONG,
        flags: u32,
        next: *mut IP_ADAPTER_UNICAST_ADDRESS,
        address: SOCKET_ADDRESS,
        // Remaining fields unused.
        _tail: [u8; 88],
    }
    // GetAdaptersAddresses layout follows the SDK (Windows Vista+); we only
    // read up to friendly_name plus next/if_index/first_unicast_address.
    #[repr(C)]
    struct IP_ADAPTER_ADDRESSES_FULL {
        length: ULONG,
        if_index: u32,
        next: *mut IP_ADAPTER_ADDRESSES_FULL,
        adapter_name: *mut i8,
        first_unicast_address: *mut IP_ADAPTER_UNICAST_ADDRESS,
        first_anycast_address: *mut u8,
        first_multicast_address: *mut u8,
        first_dns_server_address: *mut u8,
        dns_suffix: *mut u16,
        description: *mut u16,
        friendly_name: *mut u16,
        // …many more fields; we stop here.
    }

    #[link(name = "iphlpapi")]
    unsafe extern "system" {
        fn GetAdaptersAddresses(
            family: ULONG,
            flags: ULONG,
            reserved: *mut core::ffi::c_void,
            addresses: *mut IP_ADAPTER_ADDRESSES_FULL,
            size: *mut ULONG,
        ) -> ULONG;
    }

    const AF_INET: ULONG = 2;
    const ERROR_BUFFER_OVERFLOW: ULONG = 111;
    const ERROR_SUCCESS: ULONG = 0;
    // Skip parsing DNS servers etc.
    const GAA_FLAG_SKIP_ANYCAST: ULONG = 0x0002;
    const GAA_FLAG_SKIP_MULTICAST: ULONG = 0x0004;
    const GAA_FLAG_SKIP_DNS_SERVER: ULONG = 0x0008;

    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;

    unsafe {
        let mut size: ULONG = 0;
        let rc = GetAdaptersAddresses(
            AF_INET,
            flags,
            core::ptr::null_mut(),
            core::ptr::null_mut(),
            &mut size,
        );
        if rc != ERROR_BUFFER_OVERFLOW || size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        let rc = GetAdaptersAddresses(
            AF_INET,
            flags,
            core::ptr::null_mut(),
            buf.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_FULL,
            &mut size,
        );
        if rc != ERROR_SUCCESS {
            return None;
        }

        let mut cur = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_FULL;
        while !cur.is_null() {
            let adapter = &*cur;
            let friendly = if adapter.friendly_name.is_null() {
                String::new()
            } else {
                let mut len = 0;
                while *adapter.friendly_name.add(len) != 0 {
                    len += 1;
                }
                let slice = core::slice::from_raw_parts(adapter.friendly_name, len);
                String::from_utf16_lossy(slice)
            };

            let name_matches = !if_name.is_empty() && friendly == if_name;
            let ip_matches = if_ip.is_some_and(|want| {
                let mut ua = adapter.first_unicast_address;
                while !ua.is_null() {
                    let addr = &*ua;
                    if !addr.address.lp_sockaddr.is_null()
                        && (*addr.address.lp_sockaddr).sa_family == AF_INET as u16
                    {
                        let b = (*addr.address.lp_sockaddr).sa_data;
                        let ip = std::net::Ipv4Addr::new(b[2], b[3], b[4], b[5]);
                        if ip == want {
                            return true;
                        }
                    }
                    ua = addr.next;
                }
                false
            });

            if name_matches || ip_matches {
                return NonZeroU32::new(adapter.if_index);
            }
            cur = adapter.next;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_state_roundtrip() {
        clear_outbound_bind();
        assert!(outbound_bind().is_none());
        set_outbound_bind(OutboundBind {
            if_index_v4: NonZeroU32::new(12),
            if_index_v6: None,
            if_name: Some("eth0".to_string()),
        });
        let b = outbound_bind().expect("set");
        assert_eq!(b.if_index_v4, NonZeroU32::new(12));
        assert_eq!(b.if_name.as_deref(), Some("eth0"));
        clear_outbound_bind();
        assert!(outbound_bind().is_none());
    }

    #[test]
    fn apply_is_noop_when_unset() {
        clear_outbound_bind();
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        assert!(apply_to_tcp_socket(&socket, false).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn loopback_interface_has_index() {
        // Every Windows box has a loopback interface named like "Loopback
        // Pseudo-Interface 1" — but its friendly name is localized, so just
        // assert the lookup of a nonsense name returns None and doesn't crash.
        assert!(interface_index_by_name("definitely-not-an-interface-xyz").is_none());
    }

    #[cfg(windows)]
    #[test]
    fn loopback_ip_resolves_to_index() {
        // Positive case for the address-based lookup used by TUN setup:
        // 127.0.0.1 is owned by the loopback adapter on every Windows box.
        let idx = interface_index_by_ip(std::net::Ipv4Addr::LOCALHOST)
            .expect("loopback adapter must resolve to an interface index");
        assert!(idx.get() > 0);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn bound_socket_still_connects() {
        // Bind outbound sockets to the loopback interface, then verify a TCP
        // connect to a loopback listener still succeeds — exercises the real
        // setsockopt(IP_UNICAST_IF) path end to end.
        let idx = interface_index_by_ip(std::net::Ipv4Addr::LOCALHOST).expect("loopback index");
        set_outbound_bind(OutboundBind {
            if_index_v4: Some(idx),
            if_index_v6: None,
            if_name: None,
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        apply_to_tcp_socket(&socket, false).expect("apply bind");
        let connect = socket.connect(std::net::SocketAddr::from(([127, 0, 0, 1], port)));
        let (stream, _) = tokio::join!(connect, listener.accept());
        stream.expect("connect through bound socket");
        clear_outbound_bind();
    }
}
