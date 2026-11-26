#!/usr/bin/env python3
"""Sample a macOS terminal app or generate the same PTY workload in any app."""

import argparse
import ctypes
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import time


WEBKIT = {"com.apple.WebKit.WebContent", "com.apple.WebKit.GPU", "com.apple.WebKit.Networking"}
STAGES = ("idle-1", "idle-5", "idle-10", "burst", "soak", "post-close")


def processes():
    output = subprocess.check_output(
        ["/bin/ps", "-axww", "-o", "pid=,ppid=,rss=,pcpu=", "-o", "lstart=", "-o", "comm="],
        text=True,
    )
    rows = {}
    for line in output.splitlines():
        pid, ppid, rss, cpu, *identity = line.split(maxsplit=9)
        rows[int(pid)] = {
            "ppid": int(ppid), "rss_kib": int(rss), "cpu_percent": float(cpu),
            "started": " ".join(identity[:5]), "binary": identity[5],
        }
    return rows


def process_ids(rows, roots):
    found = rows.keys() & set(roots)
    while True:
        children = {pid for pid, row in rows.items() if row["ppid"] in found}
        if children <= found:
            return found
        found.update(children)


def footprint(pids):
    if not pids:
        return {}, None, None
    command = ["/usr/bin/footprint", "-f", "bytes"]
    for pid in sorted(pids):
        command.extend(("-p", str(pid)))
    result = subprocess.run(command, capture_output=True, text=True, timeout=15)
    per_pid = {}
    for pid, body in re.findall(
        r"(?m)^[^\n]+? \[(\d+)\]: [^\n]*Footprint:[^\n]*\n(.*?)(?=^[^\n]+? \[\d+\]: [^\n]*Footprint:|\Z)",
        result.stdout, flags=re.S,
    ):
        value = re.search(r"(?m)^\s*phys_footprint:\s+(\d+) B\s*$", body)
        if value:
            per_pid[int(pid)] = int(value[1])
    summary = re.search(r"(?m)^Summary Footprint: (\d+) B", result.stdout)
    error = None if result.returncode == 0 and per_pid.keys() == pids else "incomplete"
    return per_pid, int(summary[1]) if summary else None, error


def window_detector():
    core_graphics = ctypes.CDLL("/System/Library/Frameworks/CoreGraphics.framework/CoreGraphics")
    core_foundation = ctypes.CDLL("/System/Library/Frameworks/CoreFoundation.framework/CoreFoundation")
    core_graphics.CGWindowListCopyWindowInfo.argtypes = [ctypes.c_uint32, ctypes.c_uint32]
    core_graphics.CGWindowListCopyWindowInfo.restype = ctypes.c_void_p
    core_foundation.CFArrayGetCount.argtypes = [ctypes.c_void_p]
    core_foundation.CFArrayGetCount.restype = ctypes.c_long
    core_foundation.CFArrayGetValueAtIndex.argtypes = [ctypes.c_void_p, ctypes.c_long]
    core_foundation.CFArrayGetValueAtIndex.restype = ctypes.c_void_p
    core_foundation.CFDictionaryGetValue.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
    core_foundation.CFDictionaryGetValue.restype = ctypes.c_void_p
    core_foundation.CFNumberGetValue.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_void_p]
    core_foundation.CFNumberGetValue.restype = ctypes.c_bool
    core_foundation.CFRelease.argtypes = [ctypes.c_void_p]
    owner_key = ctypes.c_void_p.in_dll(core_graphics, "kCGWindowOwnerPID")
    layer_key = ctypes.c_void_p.in_dll(core_graphics, "kCGWindowLayer")

    def number(item, key):
        value = ctypes.c_int()
        ref = core_foundation.CFDictionaryGetValue(item, key)
        return value.value if ref and core_foundation.CFNumberGetValue(ref, 9, ctypes.byref(value)) else None

    def visible(pid):
        windows = core_graphics.CGWindowListCopyWindowInfo(1 | 16, 0)
        if not windows:
            return False
        try:
            for index in range(core_foundation.CFArrayGetCount(windows)):
                item = core_foundation.CFArrayGetValueAtIndex(windows, index)
                if number(item, owner_key) == pid and number(item, layer_key) == 0:
                    return True
            return False
        finally:
            core_foundation.CFRelease(windows)
    return visible


def socket_roots(socket):
    # v0.1.22 binds the core socket plus hc.holder[.ID].sock in its private directory.
    if socket is None:
        return set()
    socket = str(socket.resolve())
    holder = socket.removesuffix(".sock") + ".holder"
    command = ["/usr/sbin/lsof", "-nP", "-U", "-Fpn"]
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=5)
    except subprocess.TimeoutExpired:
        return set()
    found = set()
    for line in result.stdout.splitlines():
        if line.startswith("p"):
            pid = int(line[1:])
        elif line.startswith("n"):
            name = line[1:].split(" type=", 1)[0]
            if not name.startswith("/"):
                continue
            name = str(Path(name).resolve())
            if name in (socket, holder + ".sock") or (
                name.startswith(holder + ".") and name.endswith(".sock")
            ):
                found.add(pid)
    return found


def identities(rows, pids):
    return {(pid, rows[pid]["started"], rows[pid]["binary"]) for pid in pids if pid in rows}


def logical_size(path):
    files = (path,) if path.is_file() else path.rglob("*")
    sizes = (item.lstat().st_size for item in files if item.is_file() or item.is_symlink())
    return sum(sizes)


def workload(args):
    line = "\x1b[36muiterm-bench\x1b[0m 日本語 あＡα 👩‍💻 e\u0301 0123456789abcdef\n".encode()
    block = line * 1024
    start = time.monotonic()
    count = 0
    while count < args.mib * 1024**2 or (args.seconds and time.monotonic() - start < args.seconds):
        sys.stdout.buffer.write(block)
        sys.stdout.buffer.flush()
        count += len(block)
        if args.seconds:
            target = start + count / (args.rate_kib * 1024)
            time.sleep(max(0, target - time.monotonic()))
    print(f"BENCH_DONE bytes={count} seconds={time.monotonic() - start:.3f}", flush=True)


def measure(args):
    with (args.app / "Contents/Info.plist").open("rb") as stream:
        info = plistlib.load(stream)
    executable = args.app / "Contents/MacOS" / info["CFBundleExecutable"]
    apps = processes()
    own = {pid for pid, row in apps.items() if Path(row["binary"]).resolve() == executable.resolve()}
    if args.pid is None and own:
        raise SystemExit(f"既存の起動を検出しました。PID {sorted(own)} を指定してください。")

    print(json.dumps({
        "record": "environment", "bundle": str(args.app), "version": info.get("CFBundleShortVersionString"),
        "machine": os.uname().machine, "darwin": os.uname().release,
        "bundle_logical_bytes": logical_size(args.app),
        "extra_logical_bytes": {str(item): logical_size(item) for item in args.extra},
        "launch_label": args.label, "stage": args.stage, "condition": args.condition,
        "attribution": "root/descendant + explicit roots + isolated socket + optional WebKit delta",
    }), flush=True)
    core_sockets_before_launch = set(Path("/tmp").glob("uiterm.*/hc.sock"))
    if args.pid is None:
        visible = window_detector()
        start = time.monotonic()
        app = subprocess.Popen(
            [str(executable), *args.app_arg], stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        pid = app.pid
        while time.monotonic() - start < 30 and app.poll() is None:
            if visible(pid):
                print(json.dumps({"record": "window_visible", "app_pid": pid, "launch_ms": (time.monotonic() - start) * 1000}), flush=True)
                break
            time.sleep(0.02)
        else:
            print(json.dumps({"record": "window_visible", "app_pid": pid, "launch_ms": None}), flush=True)
            return
        if args.wait_ready:
            print("初回端末にプロンプトが表示されたら、この計測端末で Enter を押してください。", file=sys.stderr)
            input()
            print(json.dumps({"record": "prompt_seen_upper_bound", "launch_ms": (time.monotonic() - start) * 1000}), flush=True)
    else:
        pid = args.pid
        if pid not in own:
            raise SystemExit("指定 PID の実行ファイルが .app と一致しません。")

    if args.stage is None:
        return

    selected_socket = Path(args.core_socket) if args.core_socket and args.core_socket != "auto" else None
    ids_seen = set()
    started = time.monotonic()
    while True:
        rows = processes()
        if pid not in rows and args.stage != "post-close":
            raise SystemExit("計測中にアプリが終了しました。")
        if args.core_socket == "auto" and selected_socket is None:
            candidates = set(Path("/tmp").glob("uiterm.*/hc.sock")) - core_sockets_before_launch
            if len(candidates) > 1:
                raise SystemExit("対象の core を一意に特定できません。--core-socket に実パスを指定してください。")
            if candidates:
                selected_socket = candidates.pop()
        core_roots = socket_roots(selected_socket)
        roots = {pid, *args.include} | core_roots
        webkit = {
            child for child, row in rows.items()
            if Path(row["binary"]).name in WEBKIT and child not in apps
        } if args.webkit_delta else set()
        selected = process_ids(rows, roots | webkit)
        if args.stage == "post-close" and ids_seen:
            survivors = {
                child for child, row in rows.items()
                if (child, row["started"], row["binary"]) in ids_seen
            }
            if survivors:
                selected |= process_ids(rows, survivors)
        ids_seen.update(identities(rows, selected))
        elapsed = time.monotonic() - started
        try:
            physical, summary, footprint_error = footprint(selected)
        except subprocess.TimeoutExpired:
            physical, summary, footprint_error = {}, None, "timeout"
        print(json.dumps({
            "record": "sample", "stage": args.stage, "elapsed_s": elapsed,
            "time_unix_s": time.time(), "app_pid": pid, "app_alive": pid in rows,
            "rss_sum_kib": sum(rows[child]["rss_kib"] for child in selected),
            "cpu_percent_sum_ps": sum(rows[child]["cpu_percent"] for child in selected),
            "footprint_summary_bytes": summary, "footprint_status": footprint_error,
            "pid_identity_count": len(ids_seen), "webkit_delta_pids": sorted(webkit),
            "core_socket": str(selected_socket) if selected_socket else None,
            "core_socket_pids": sorted(core_roots),
            "absent_explicit_roots": sorted(set(args.include) - rows.keys()),
            "processes": [
                dict(rows[child], pid=child, phys_footprint_bytes=physical.get(child))
                for child in sorted(selected)
            ],
        }), flush=True)
        if time.monotonic() - started >= args.seconds:
            break
        time.sleep(max(0, started + elapsed + args.interval - time.monotonic()))


def self_test():
    rows = {1: {"ppid": 0}, 2: {"ppid": 1}, 3: {"ppid": 2}, 4: {"ppid": 0}}
    assert process_ids(rows, {1, 9}) == {1, 2, 3}
    assert process_ids(rows, {1, 4}) == {1, 2, 3, 4}
    assert os.getpid() in processes()
    if sys.platform == "darwin":
        window_detector()(os.getpid())
        physical, _, status = footprint({os.getpid()})
        assert status is None and physical[os.getpid()] > 0


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("self-test")
    load = commands.add_parser("workload")
    load.add_argument("--mib", type=int, default=5)
    load.add_argument("--seconds", type=int, default=0)
    load.add_argument("--rate-kib", type=int, default=256)
    sample = commands.add_parser("measure")
    sample.add_argument("--app", required=True, type=lambda path: Path(path).resolve(strict=True))
    sample.add_argument("--pid", type=int)
    sample.add_argument("--label", choices=("first-after-login", "warm"), default="warm")
    sample.add_argument("--wait-ready", action="store_true")
    sample.add_argument("--app-arg", action="append", default=[])
    sample.add_argument("--include", action="append", type=int, default=[])
    sample.add_argument("--core-socket", help="Socket path, or auto for a newly launched uiterm only")
    sample.add_argument("--webkit-delta", action="store_true")
    sample.add_argument("--extra", action="append", type=lambda path: Path(path).resolve(strict=True), default=[])
    sample.add_argument("--stage", choices=STAGES)
    sample.add_argument("--condition", required=True, help="record the build, terminal size, shell, and visibility")
    sample.add_argument("--seconds", type=float, default=30)
    sample.add_argument("--interval", type=float, default=2)
    args = parser.parse_args()
    if args.command == "self-test":
        self_test()
    elif args.command == "workload":
        if args.mib < 0 or args.seconds < 0 or args.rate_kib <= 0:
            load.error("負のサイズ・時間、または 0 以下の出力レートは指定できません。")
        workload(args)
    else:
        if sys.platform != "darwin":
            sample.error("アプリ計測には macOS が必要です。")
        if (args.webkit_delta or args.core_socket == "auto") and args.pid is not None:
            sample.error("WebKit の差分検出とソケットの自動検出には起動前の一覧が必要です。")
        if args.seconds <= 0 or args.interval <= 0:
            sample.error("計測時間と間隔は正の数を指定してください。")
        measure(args)


if __name__ == "__main__":
    main()
