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
        // Leave room for ordinary multi-device LAN concurrency. The previous
        // 32/64/128 caps could reject new flows while ample memory remained.
        // At the 1 GiB tier, 512 TCP flows budget 96 MiB for two relay buffers
        // and the 64 KiB unacknowledged-data window; DNS has its own quota.
        if total <= 300 * 1024 {
            Self {
                tun_streams: 128,
                dns_queries: 32,
                dns_tcp_clients: 4,
                api_requests: 2,
            }
        } else if total <= 600 * 1024 {
            Self {
                tun_streams: 256,
                dns_queries: 64,
                dns_tcp_clients: 8,
                api_requests: 4,
            }
        } else {
            Self {
                tun_streams: 512,
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn small_router_budgets_bound_worst_case_stream_buffers() {
        for (memory, streams) in [(256, 128), (512, 256), (1024, 512)] {
            let budget = ResourceBudget::for_memory_kib(memory * 1024);
            assert_eq!(budget.tun_streams, streams);
            // Include two relay buffers plus the TCP unacked-data budget.
            assert!(budget.tun_streams as u64 * 192 < memory * 1024 / 8);
            assert!(budget.dns_queries <= budget.tun_streams);
            assert!(budget.api_requests <= 6);
        }
    }
}
