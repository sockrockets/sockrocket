"""Run a real CLI daemon + CGI and a loopback-only VLESS echo peer, without TUN."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import socket
import struct
import tempfile
import time

HOST = "127.0.0.1"
DEST = "203.0.113.7"


async def vless_echo(reader, writer):
    try:
        first = await reader.read(1)
        if not first:  # Unused upstream TCP preconnection.
            return
        header = first + await reader.readexactly(17)
        assert header[:17] == bytes(17)
        await reader.readexactly(header[17])
        command, port, atyp = struct.unpack("!BHB", await reader.readexactly(4))
        assert (command, port, atyp) == (1, 443, 1)
        assert await reader.readexactly(4) == socket.inet_aton(DEST)
        writer.write(b"\x00\x00")
        while packet := await reader.read(4096):
            writer.write(packet)
            await writer.drain()
    finally:
        writer.close()


async def rule_list(reader, writer):
    try:
        await reader.readuntil(b"\r\n\r\n")
        body = f"IP-CIDR,{DEST}/32,PROXY\n".encode()
        writer.write(f"HTTP/1.1 200 OK\r\nContent-Length: {len(body)}\r\n\r\n".encode() + body)
        await writer.drain()
    finally:
        writer.close()


async def connect(port, kind):
    reader, writer = await asyncio.open_connection(HOST, port)
    try:
        if kind == "SOCKS5":
            writer.write(b"\x05\x01\x00")
            assert await reader.readexactly(2) == b"\x05\x00"
            writer.write(b"\x05\x01\x00\x01" + socket.inet_aton(DEST) + struct.pack("!H", 443))
            response = await reader.readexactly(10)
            assert response[0] == 5
            accepted = response[1] == 0
        else:
            writer.write(f"CONNECT {DEST}:443 HTTP/1.1\r\nHost: {DEST}:443\r\n\r\n".encode())
            response = await reader.readuntil(b"\r\n\r\n")
            assert response.startswith((b"HTTP/1.1 200 ", b"HTTP/1.1 502 "))
            accepted = response.startswith(b"HTTP/1.1 200 ")
        if accepted:
            return reader, writer
    except BaseException:
        writer.close()
        raise
    writer.close()
    return None


async def heartbeat(stream):
    reader, writer = stream
    count = 0
    try:
        while True:
            frame = struct.pack("!Q", count)
            writer.write(frame)
            await writer.drain()
            assert await asyncio.wait_for(reader.readexactly(len(frame)), 2) == frame
            count += 1
            await asyncio.sleep(.04)
    except asyncio.CancelledError:
        return count
    finally:
        writer.close()


async def run(binary, root, accelerated):
    env = dict(os.environ, SOCKROCKET_DIR=str(root), TMPDIR=str(root), TMP=str(root), TEMP=str(root))
    env.pop("GATEWAY_INTERFACE", None)
    # Linux uses the real procfs. Model its comm entry on development Macs.
    if not Path("/proc/self/comm").exists():
        env["SOCKROCKET_PROC_BASE"] = str(root / "proc")
    (root / "scripts").mkdir()
    calls = root / "lifecycle-called"
    (root / "scripts/sockrocket.sh").write_text(
        'echo "$1" >> "$SOCKROCKET_DIR/lifecycle-called"\n'
        '[ "$1" = refresh-rule-sources ]\n'
    )

    async def api(action, body=None):
        payload = json.dumps(body or {}).encode()
        proc = await asyncio.create_subprocess_exec(
            binary, "cgi", stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            env=dict(env, REQUEST_METHOD="POST", QUERY_STRING=f"action={action}",
                     CONTENT_LENGTH=str(len(payload))),
        )
        output, _ = await asyncio.wait_for(proc.communicate(payload), 10)
        assert proc.returncode == 0
        return json.loads(output.splitlines()[-1])

    async with await asyncio.start_server(vless_echo, HOST, 0) as peer:
        with socket.socket() as a, socket.socket() as b:
            a.bind((HOST, 0))
            b.bind((HOST, 0))
            ports = dict(SOCKS5=a.getsockname()[1], HTTP=b.getsockname()[1])
        config = dict(
            listen_addr=HOST, socks_port=ports["SOCKS5"], http_port=ports["HTTP"],
            proxy_mode="rule", subscriptions=[], active_node=0, explicit_group_bindings=True,
            transparent_proxy=accelerated, cn_ipset_direct=accelerated,
            nodes=[dict(
                name="local", server=HOST, port=peer.sockets[0].getsockname()[1],
                protocol=dict(type="vless", uuid="00000000-0000-0000-0000-000000000000"),
            )],
            rules=[
                dict(group="demo", rule_type="ip-cidr", pattern=pattern,
                     target=target, priority=priority, enabled=False)
                for pattern, target, priority in [
                    (f"{DEST}/32", "reject", 30),
                    ("203.0.113.6/31", "proxy", 20),
                    ("203.0.113.0/24", "reject", 10),
                ]
            ],
            device_rule_groups=[dict(source="", group="demo", priority=0, target="")],
        )
        if accelerated:
            # The real CGI sees a source-specific member enter and leave the
            # active set. Only the router kernel operation is stubbed on macOS.
            config["device_rule_groups"].append(dict(source="192.0.2.11", group="demo"))
            config["rules"].append(dict(group="demo", source="192.0.2.10/32",
                rule_type="domain", pattern="test.invalid", target="reject", enabled=False))
        path = root / "config.yaml"
        path.write_text(json.dumps(config))
        with (root / "daemon.log").open("wb") as log:
            daemon = await asyncio.create_subprocess_exec(binary, str(path), env=env, stdout=log, stderr=log)
        (root / "sockrocket.pid").write_text(str(daemon.pid))
        if env.get("SOCKROCKET_PROC_BASE") == str(root / "proc"):
            entry = root / "proc" / str(daemon.pid)
            entry.mkdir(parents=True)
            (entry / "comm").write_text("sockrocket-cli")
        streams = []
        try:
            async def ready(kind):
                while True:
                    if daemon.returncode is not None:
                        raise RuntimeError((root / "daemon.log").read_text())
                    try:
                        stream = await connect(ports[kind], kind)
                    except ConnectionRefusedError:
                        await asyncio.sleep(.05)
                        continue
                    assert stream is not None
                    return stream
            for kind in ports:
                streams.append(asyncio.create_task(heartbeat(await asyncio.wait_for(ready(kind), 10))))
            last_action = "delete last rule" if accelerated else "delete group"
            for action, rejected in [
                ("enable", True), ("disable", False), ("enable", True),
                ("delete rule", False), ("delete in batch editor", True),
                ("import URL", False), ("delete imported rule", True), (last_action, False),
            ]:
                snapshot = await api("get_rules")
                before = time.monotonic()
                members = [{k: v for k, v in row.items() if k != "index"}
                           for row in snapshot["rules"]]
                if action == "delete group":
                    reply = await api("delete_rule_group", dict(group="demo", expected_revision=snapshot["revision"]))
                elif action in ("delete rule", "delete imported rule", "delete last rule"):
                    row = snapshot["rules"][0]
                    reply = await api("del_rule", dict(index=row["index"], expected=members[0]))
                elif action == "import URL":
                    async with await asyncio.start_server(rule_list, HOST, 0) as server:
                        port = server.sockets[0].getsockname()[1]
                        preview = await api("fetch_rule_list", dict(group="demo", url=f"http://{HOST}:{port}/rules"))
                    assert preview["ok"] and preview["count"] == 1, preview
                    assert (await api("get_rules"))["revision"] == snapshot["revision"]
                    reply = await api("batch_add_rules", dict(
                        group="demo", group_only=True, text=preview["text"], priority=99,
                    ))
                elif action == "delete in batch editor":
                    reply = await api("replace_rule_group", dict(
                        group="demo", expected_rules=members, text="IP-CIDR,203.0.113.0/24,REJECT",
                    ))
                else:
                    reply = await api("set_rule_group_enabled", dict(
                        group="demo", enabled=rejected, expected_rules=members,
                    ))
                assert reply.get("ok") and reply.get("reload_pending"), reply

                async def policy_matches():
                    while True:
                        observed = []
                        for kind, port in ports.items():
                            stream = await connect(port, kind)
                            if stream:
                                reader, writer = stream
                                writer.write(b"probe")
                                assert await reader.readexactly(5) == b"probe"
                                writer.close()
                            observed.append(stream is not None)
                        if observed == [not rejected, not rejected]:
                            return
                        await asyncio.sleep(.1)

                await asyncio.wait_for(policy_matches(), 10)
                assert all(not task.done() for task in streams)
                print(f"{action}: new SOCKS5/HTTP connections {'rejected' if rejected else 'relayed'} after {time.monotonic()-before:.2f}s")
                if action.startswith("delete"):
                    current = await api("get_rules")
                    remaining = bool(current["rules"])
                    assert remaining == (action != last_action)
                    assert bool(current["global_rule_groups"]) == remaining
                    assert bool(current["device_rule_groups"]) == (remaining and accelerated)
            expected = ["refresh-rule-sources"] * 5 if accelerated else []
            assert (calls.read_text().splitlines() if calls.exists() else []) == expected
            assert daemon.returncode is None
            for task in streams:
                task.cancel()
            counts = await asyncio.gather(*streams)
            assert all(n > 5 for n in counts)
            print(f"PASS: same daemon, same two TCP connections, {counts} verified echoes, zero reconnects")
        finally:
            for task in streams:
                task.cancel()
            await asyncio.gather(*streams, return_exceptions=True)
            if daemon.returncode is None:
                daemon.terminate()
            await daemon.wait()


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("binary", nargs="?", default="target/debug/sockrocket-cli")
    binary = str(Path(parser.parse_args().binary).resolve())
    for accelerated in [False, True]:
        print("Mode:", "simulated CN acceleration" if accelerated else "explicit proxy")
        with tempfile.TemporaryDirectory() as tmp:
            asyncio.run(run(binary, Path(tmp), accelerated))
