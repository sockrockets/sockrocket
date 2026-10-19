# Merlin log review — 2026-10-16

Read-only review of retained application, crash-history and firmware system logs. Raw logs remain in the ignored private workspace; this report contains no subscription URLs, node names, endpoints or client identifiers. No deployment, daemon start, network changes or reboot occurred during this review.

## Findings

1. **High priority: transparent TCP reply path fails.** Retained application logs contain 277 `SynReceived` timeouts, versus one timeout in `Established`, four in `FinWait2` and seven in `CloseWait`. The handshake-heavy failure pattern agrees with the previous isolated TAP reproduction: Google Fake-IP traffic reached the TUN stream handler but SYN-ACK did not reach the simulated client. A main-table Fake-IP route through TUN restored handshake and HTTP 204. This establishes a specific defect, not proof that every historical failure has the same cause. The route correction is local and has not been deployed.
2. **High priority: historical dnsmasq permissions broke DNS.** Firmware logs contain six failures to read the upstream DNS file because of permissions. Such a failure can affect domestic and overseas resolution together. The existing local correction preserves firmware-readable mode 0644 while retaining private backups at 0600. At review time the router file was 0644 and contained no local proxy DNS pin. Some recorded events coincide with prior controlled diagnostics; counts cannot be treated as spontaneous outage counts.
3. **Needs targeted reproduction: dead per-flow channels.** Application logs contain 181 `process_device_read error: channel closed` warnings. In ipstack 1.0.0, sending an inbound packet to a closed session channel generates this warning; the outer device loop logs it and continues. It does not itself prove global TUN shutdown or daemon crash. Investigation should distinguish late packets during teardown from persistent stale sessions under active traffic. Do not merely silence the warning.
4. **IPv6 capability mismatch.** There are 22 TUN IPv6 address-setup permission warnings followed by explicit IPv4-only continuation. This is not evidence of a process crash. Product behavior still needs validation for LAN IPv6 policy and fallback before claiming full dual-stack support.
5. **Private historical log permissions.** The older crash-history file is world-readable/writable on the router. Future repair should restrict private diagnostic artifacts and rotate logs without exposing node/client metadata. This read-only review did not change permissions.

## Current state

The service is manually stopped. The remaining CLI process is the management API, with approximately 3.3 MiB resident memory. The proxy policy table is empty; no TUN interface or Fake-IP main route was present. The management HTTP endpoint returned 200. Available memory was approximately 513 MiB. Collected system logs and the current kernel ring buffer contained no OOM, killed-process, segfault or kernel-panic matches. Missing/rotated logs limit historical conclusions.

## Proposed order

- Keep the local client and physical network unchanged; leave router LAN interception stopped.
- Review and validate the local Fake-IP route correction, including installation failure, firewall rebuild, stop and crash recovery.
- Reproduce closed-session behavior in an isolated client harness; verify a failed flow cannot poison later connections or accumulate sessions.
- Validate DNS readability under dnsmasq's actual account, permission preservation after firmware regeneration, and direct DNS restoration after shutdown.
- Only then deploy with a rollback, and validate real LAN transparent HTTPS, DNS, sustained connections and lifecycle recovery separately from explicit HTTP/SOCKS proxy tests. Router reboot remains outside the authorized scope.

## Local repair status

- Active Fake-IP traffic now has a main-table route through the application TUN; route installation failure prevents attaching LAN interception. Verification checks this route, and full shutdown removes it. TUN-only shutdown explicitly rejects cached Fake-IP destinations instead of silently blackholing packets.
- Matching DNS pin contents no longer bypass permission repair. Old 0600 firmware inputs are repaired to 0644 and dnsmasq receives one reload; already-correct inputs do not receive repeated reloads.
- Service-created artifacts default to 0600 via umask 077. Existing runtime/crash logs are restricted before rotation or crash collection. Firmware DNS inputs retain explicit 0644 exceptions.
- Linux respects per-interface IPv6 disable policy instead of attempting an IPv6 address operation known to fail. This does not add LAN dual-stack support.
- A packet-preserving, network-free regression accepted 32 successive TCP sessions with delayed ACKs after each closed flow. No global TUN acceptance failure was reproduced. No dependency change or warning suppression was applied; broader stale-session behavior remains an observation item.

Validation: 368 core-library tests passed. The 23 shell fault scenarios include DNS permissions and one-time reload, private logs, Fake-IP route failure before interception, and cleanup regressions. ARM64 musl release compilation succeeded. No deployment or live router state change was performed for these repairs.
