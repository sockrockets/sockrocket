# Routing

Rule mode is the only mode that consults rules. Global sends every proxied connection to the node selected at the top of the page. Direct does not use a proxy node. A node group, and a rule pinned to one node, take effect only in rule mode.

## Three exits

A rule exit is one of these:

| Exit | What it follows |
|------|-----------------|
| Global node (`proxy`, labeled 代理 or 全局节点) | The node selected at the top of the page, including a health auto-switch |
| One node (`node:…`, labeled 节点 · name) | That node only. Changing the top selection leaves this rule where it is |
| A node group (`group:Name`, labeled 分组 · name) | The members and strategy of that group. Switching a member does not change the global node |

Pinning one node is enough when a single rule should stay on that line. A manual group of one node does the same for every rule that names the group, so later you change the node in one place.

## Node groups

Create a group under **Nodes**. The name may contain letters (including Chinese), numbers, hyphens and underscores. There are three strategies:

- **Manual.** One node. New connections use it. Merlin saves only that node. On the desktop you can keep several members and pick which one is current; the others are not a priority list.
- **Fallback.** Several nodes, in list order. For five minutes, new TCP starts at the last member that connected, then it starts at the first member again. If a member fails to establish, the rest are tried in order. UDP stays on the first member. An open connection is not moved.
- **URL test.** Several nodes. For five minutes, new TCP reuses the fastest recent handshake. After that the members race, and list order does not choose the winner. UDP uses that same pick.

A missing member fails closed. It is not silently replaced by the global node. Up to 64 groups, and 256 members in a fallback or URL-test group.

## Turn a group on

1. On the overview, choose **Rule**.
2. Create the group on the Nodes page.
3. On Routing rules, set that rule group's **exit** to `分组 · name`, or set one rule's action to the same group.

The rule-group exit rewrites rules that were already going through a proxy, a node, or a group. Direct and reject rules in that rule group stay as they are.

A rule group (the folder of rules on the Routing page) is not a node group. The folder organizes rules. The node group is an exit.

## Device policies in Merlin

The desktop app edits and applies global rules. When it opens a shared configuration, it preserves router-specific rules and device group bindings for use on Merlin.

Under **设备分流策略**, select a router client, a previously configured device, or a manual IP/network. The picker uses local DHCP leases and bridge neighbors. Names can be edited in the plugin. A discovered device's custom name is keyed by its MAC address, while routing uses its IP or subnet. Reserve a DHCP address if the policy should continue to apply to the same device.

Open **添加规则组（可多选）**, select one or more groups, and apply them together. Each new group initially uses its own rule actions. Select a group under **已关联的规则组** to change its exit for this device or remove its link independently. The link follows the group's enabled, global destination rules as you edit them. Explicit device destination rules take precedence, followed by linked groups, shared destination rules, domestic direct routing and the device/global fallback. Removing a link restores normal matching. Changing a device name does not restart the proxy.

## Import a rule list into a Merlin rule group

Select the rule group and choose **从链接添加**. Paste an HTTP(S) URL to a raw UTF-8 rule list, choose the import exit, then select **读取规则**. Review the editable text and choose **预览并应用**, followed by **确认应用**.

Lines such as `DOMAIN,example.com` inherit the selected global node, pinned node or node group. Explicit actions such as `DIRECT` and `REJECT` are preserved. The importer uses the existing bulk-text formats, including domain, domain suffix, IP/CIDR, GEOIP and SOURCE rules; it ignores blank lines and `#` comments and normalizes international domain names. If you change the exit after reading, read the list again or edit its explicit actions before applying.

This appends rules to the selected rule group. It preserves existing rules and skips duplicates with the same type, match and source, including duplicates already in another group. Group directives in the downloaded list cannot redirect the import to another group. An invalid line rejects the entire list and identifies its line number; reading or a failed download does not save configuration.

Imports are one-time copies, not automatically refreshed subscriptions. Each list is limited to 256 KiB and 4,096 rules, with a 30-second download timeout. Update the Merlin plugin's backend and WebUI together to use this feature.

## Example

```text
DOMAIN-SUFFIX,example.com,group:Video
SOURCE,192.0.2.10/32,MATCH,group:Work
```

`Video` and `Work` must already exist. These addresses are examples only.
