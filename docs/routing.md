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

## Example

```text
DOMAIN-SUFFIX,example.com,group:Video
SOURCE,192.0.2.10/32,MATCH,group:Work
```

`Video` and `Work` must already exist. These addresses are examples only.
