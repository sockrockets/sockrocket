# Merlin 稳定性复查与处理方案

## 本轮确认的问题

| 优先级 | 问题 | 处理与验收 |
| --- | --- | --- |
| P1 | DNS 上游恢复失败仍可能删除备份、报告成功 | 原子替换；失败保留备份并传播错误；故障注入验证 |
| P1 | 误把固件 DNS 上游文件也设为凭据级私有权限，降权 dnsmasq 无法读取 | 固件运行文件保持服务可读；用户配置与备份保持私有；LAN UDP/TCP DNS 实测和权限回归 |
| P1 | 故障降级清理删除 Fake-IP 映射，客户端缓存跨重启失效 | 故障恢复保留映射，明确手动停止才清除；脚本回归及监督恢复验证 |
| P1 | 部署缺少本应用开机与 WAN 恢复钩子 | 私有备份后补齐自有块；保留其他内容；检查语法、幂等性与恢复行为 |
| P2 | 启动钩子无条件开启代理，不尊重之前手动停止 | API 独立启动，代理启动受停止标记约束 |
| P1 | 节点测试/路由器回环 SOCKS 成功被误作客户端全链路可用 | 分开验证 LAN HTTP CONNECT、SOCKS、DNS、透明转发，以及关闭本地 TUN 后的实际浏览器访问 |

## 验证原则

按照 [验收方案](merlin-acceptance-plan.md) 执行。先通过路由器独立数据面、生命周期与受控故障验证，再切换客户端。短时并发和数分钟资源采样只能发现即时回归，不能替代长时间运行与不同固件、内存规格、WAN 类型的覆盖。整机重启需要单独允许；未执行时明确标记待验证。

诊断不附原始订阅、节点、凭据、个人路径或设备地址。订阅及节点在部署、停止、恢复期间必须保持持久化；清理仅针对本应用的网络接管与运行缓存。

### Fake-IP return route failure (isolated router reproduction)

A synthetic TAP client reproduced a transparent-path failure while the real LAN proxy remained stopped. Baidu over a real address completed TCP and HTTP 200. Google over Fake-IP entered the userspace TUN handler but its SYN-ACK did not return to the client. Removing the unreachable route alone or setting only the temporary TUN's `rp_filter` to zero did not resolve it. Installing `198.18.0.0/16 dev sockrocket-tun` in the main table restored the Google handshake and HTTP 204, with Baidu still returning 200.

The active route must therefore point at TUN; unsupported UDP retains its explicit firewall rejection. Startup must fail open if the route cannot be installed, verification must check the actual TUN route, and full cleanup must remove that route. Earlier HTTP/SOCKS acceptance did not prove transparent LAN forwarding. This isolated result does not yet establish full LAN or long-term stability.
