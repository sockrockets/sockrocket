//! Conservative process budgets shared by data and control-plane listeners.
#[derive(Clone, Copy, Debug)]
pub struct ResourceBudget {
    pub tun_streams: usize,
    pub dns_queries: usize,
    pub dns_tcp_clients: usize,
    pub api_requests: usize,
}
impl ResourceBudget {
    pub fn for_memory_kib(total: u64) -> Self {
        if total <= 300 * 1024 {
            Self {
                tun_streams: 32,
                dns_queries: 32,
                dns_tcp_clients: 4,
                api_requests: 2,
            }
        } else if total <= 600 * 1024 {
            Self {
                tun_streams: 64,
                dns_queries: 64,
                dns_tcp_clients: 8,
                api_requests: 4,
            }
        } else {
            Self {
                tun_streams: 128,
                dns_queries: 128,
                dns_tcp_clients: 16,
                api_requests: 6,
            }
        }
    }
    pub fn detect() -> Self {
        let total = std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|text| {
                text.lines().find_map(|line| {
                    line.strip_prefix("MemTotal:")?
                        .split_whitespace()
                        .next()?
                        .parse()
                        .ok()
                })
            })
            .unwrap_or(1024 * 1024);
        Self::for_memory_kib(total)
    }
}
