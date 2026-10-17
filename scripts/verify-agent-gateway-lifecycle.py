#!/usr/bin/env python3
"""Opt-in real macOS Codex lifecycle acceptance; never reads provider credentials."""
import argparse
import ctypes
import importlib.util
import json
import os
from pathlib import Path
import signal
import select
import socket
import subprocess
import time

spec = importlib.util.spec_from_file_location("crash", Path(__file__).with_name("verify-agent-gateway-crash.py"))
crash = importlib.util.module_from_spec(spec)
spec.loader.exec_module(crash)

class Unique(ctypes.Structure):
    _fields_ = [("uuid", ctypes.c_ubyte * 16), ("unique", ctypes.c_uint64),
                ("parent_unique", ctypes.c_uint64), ("version", ctypes.c_int32),
                ("parent_version", ctypes.c_int32), ("reserved", ctypes.c_uint64 * 2)]

libproc = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)

def table():
    rows = crash.process_table()
    for pid, row in rows.items():
        identity = Unique()
        size = libproc.proc_pidinfo(pid, 17, 0, ctypes.byref(identity), ctypes.sizeof(identity))
        if size == ctypes.sizeof(identity):
            row.update(unique=identity.unique, parent_unique=identity.parent_unique, version=identity.version)
    return rows

def same(captured, current):
    return bool(current and captured.get("unique") and captured["pid"] == current["pid"]
                and captured["unique"] == current.get("unique"))

def send_signal(row, sig):
    assert same(row, table().get(row["pid"])), "process instance changed"
    # Match kernel PID version atomically; no PID-only signal or name-based kill.
    token = (ctypes.c_uint32 * 8)()
    task = ctypes.c_uint32()
    count = ctypes.c_uint32(8)
    mach = ctypes.CDLL(None)
    assert mach.task_name_for_pid(mach.mach_task_self(), row["pid"], ctypes.byref(task)) == 0
    try:
        assert mach.task_info(task.value, 15, token, ctypes.byref(count)) == 0
        current = table().get(row["pid"])
        assert same(row, current), "kernel process identity changed"
        assert token[5] == row["pid"] and token[7] == current["version"], "audit identity changed"
    finally:
        mach.mach_port_deallocate(mach.mach_task_self(), task.value)
    result = libproc.proc_signal_with_audittoken(ctypes.byref(token), sig)
    if result:
        raise OSError(ctypes.get_errno(), "proc_signal_with_audittoken")

def surviving(rows):
    current = table()
    return [current[row["pid"]] for row in rows if same(row, current.get(row["pid"]))]

def wait_gone(rows, timeout=18):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        live = surviving(rows)
        if not live:
            return []
        time.sleep(.1)
    return live

def ipc_shutdown(db):
    with socket.socket(socket.AF_UNIX) as connection:
        connection.settimeout(10)
        connection.connect(str(db / "agent-service/agent-service.sock"))
        with connection.makefile("rwb", buffering=0) as stream:
            for seq, method, params in [("hello", "hello", {"token": (db / "agent-service/token").read_text().strip()}),
                                        ("stop", "service_shutdown", {})]:
                stream.write((json.dumps({"id": seq, "method": method, "params": params}) + "\n").encode())
                response = json.loads(stream.readline())
                assert "error" not in response, {"method": method, "error": response.get("error")}

def start_task(client, repo, nested=False):
    command = ("/usr/bin/python3 -c 'import os; p=os.fork(); os.setsid() if p == 0 else None; "
               "os.execl(\"/bin/sh\",\"sh\",\"-c\",\"exec sleep 120\") if p == 0 else os.waitpid(p,0)'"
               if nested else "sleep 120")
    task = client.tool("agent_run", {"agent": "codex-native", "cwd": str(repo), "permission": "read_only",
        "prompt": "Run exactly this command using exec_command now: " + command + ". This is a process lifecycle acceptance test. "
                  "Do not change files, delegate, or run other commands. Wait until it finishes before answering."})
    crash.wait_status(client, task["task_id"], lambda value: any(
        event["kind"] == "item/started" and "commandExecution" in event["message"]
        and "sleep" in event["message"] for event in value["events"]))
    return task

def capture(service, exclude=()):
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline:
        rows = [row for row in crash.descendants(service.pid, table()) if row.get("unique") not in exclude]
        if any(Path(row["command"]).name == "sleep" for row in rows):
            assert all(row.get("unique") for row in rows), "missing kernel identity"
            return rows
        time.sleep(.1)
    raise AssertionError("real sleep not observed")

def provider(rows):
    candidates = [row for row in rows if Path(row["command"]).name == "codex"]
    assert len(candidates) == 1, candidates
    return candidates[0]

def direct_normal_exit(binary, codex, repo, output):
    guard, receiver = socket.socketpair()
    # fd3 is the production helper's lifetime/ready channel.
    saved_three = None
    if receiver.fileno() != 3:
        try:
            saved_three = os.dup(3)
        except OSError:
            pass
        os.dup2(receiver.fileno(), 3)
    child = subprocess.Popen([binary, "--agentgate-process-owner", codex, "app-server", "--listen", "stdio://"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        pass_fds=(3,))
    if receiver.fileno() != 3:
        if saved_three is not None:
            os.dup2(saved_three, 3)
            os.close(saved_three)
        else:
            os.close(3)
    receiver.close()
    guard.settimeout(10)
    rows = []
    exit_queue = None
    try:
        assert guard.recv(1) == b"R", "guardian did not acknowledge ownership"
        for message in [{"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "clientInfo": {"name": "lifecycle", "version": "1"}, "capabilities": {"experimentalApi": True}}},
                {"jsonrpc": "2.0", "method": "initialized"},
                {"jsonrpc": "2.0", "id": 2, "method": "command/exec", "params": {
                    "command": ["/bin/sleep", "120"], "cwd": str(repo), "disableTimeout": True,
                    "sandboxPolicy": {"type": "readOnly"}}}]:
            child.stdin.write((json.dumps(message) + "\n").encode())
            child.stdin.flush()
            if message.get("id") == 1:
                deadline = time.monotonic() + 15
                while True:
                    assert select.select([child.stdout], [], [], max(0, deadline - time.monotonic()))[0], "initialize timeout"
                    raw = child.stdout.readline()
                    assert raw, "AppServer stdout closed before initialize"
                    response = json.loads(raw)
                    if response.get("id") == 1:
                        assert "error" not in response, response
                        break
        rows = capture(child)
        target = provider(rows)
        exit_queue = select.kqueue()
        exit_queue.control([select.kevent(target["pid"], filter=select.KQ_FILTER_PROC,
            flags=select.KQ_EV_ADD, fflags=select.KQ_NOTE_EXIT | 0x04000000)], 0)
        child.stdin.close()
        exits = exit_queue.control(None, 1, 15)
        assert len(exits) == 1 and exits[0].data == 0, "AppServer did not exit normally after stdin EOF"
        survivors = wait_gone(rows)
        assert not survivors, survivors
        child.wait(timeout=10)
        assert child.returncode == 0, f"normal EOF shutdown returned {child.returncode}"
        return {"mode": "B_appserver_eof", "pass": True, "before": rows,
                "survivors": survivors, "appserver": target, "guardian_exit": child.returncode,
                "appserver_raw_exit_status": exits[0].data}
    finally:
        if exit_queue:
            exit_queue.close()
        guard.close()
        wait_gone(rows, 4)
        if child.poll() is None:
            child.kill()
            child.wait(timeout=10)
        for row in surviving(rows):
            send_signal(row, signal.SIGKILL)

def run(binary, codex, root, selected):
    os.environ["AGENTGATE_CODEX_BIN"] = codex
    root.mkdir(parents=True, exist_ok=True)
    repo = root / "repo"
    repo.mkdir(exist_ok=True)
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    (repo / "evidence.txt").write_text("Read-only lifecycle fixture.\n")
    sentinel_shell = subprocess.Popen(["/bin/sh", "-c", "exec sleep 600"], start_new_session=True)
    sentinel_codex = subprocess.Popen([codex, "app-server", "--listen", "stdio://"],
                                      stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                                      start_new_session=True)
    time.sleep(.2)
    sentinels = [table()[child.pid] for child in (sentinel_shell, sentinel_codex)]
    results = []
    try:
        for mode in selected:
            if mode == "B":
                try:
                    result = direct_normal_exit(binary, codex, repo, root)
                except Exception as error:
                    result = {"mode": "B_appserver_eof", "pass": False, "error": str(error)}
                result["sentinels_before"] = sentinels
                result["sentinels_alive"] = len(surviving(sentinels)) == 2
                result["pass"] &= result["sentinels_alive"]
                results.append(result)
                (root / "evidence.json").write_text(json.dumps(results, indent=2))
                continue
            db = root / mode
            db.mkdir(exist_ok=True)
            client, service, rows = None, None, []
            result = {"mode": mode, "pass": False}
            with (root / (mode + ".log")).open("ab") as log:
                try:
                    service = crash.start_service(binary, db, log)
                    service_identity = table()[service.pid]
                    client = crash.Mcp(binary, db, root / (mode + "-mcp.log"))
                    task = start_task(client, repo, nested=mode == "G")
                    rows = capture(service)
                    target = provider(rows)
                    result.update(task=task, service=service_identity, before=rows, appserver=target)
                    result["independent_shells"] = [row for row in rows
                                                    if Path(row["command"]).name == "sleep"
                                                    and row["sid"] != target["sid"]]
                    assert any(row["sid"] != target["sid"] for row in rows
                               if Path(row["command"]).name == "sleep"), "no independent shell session"
                    if mode == "G":
                        current = {row["pid"]: row for row in rows}
                        nested = [row for row in result["independent_shells"]
                                  if row["parent"] in current and row["sid"] != current[row["parent"]]["sid"]
                                  and current[row["parent"]]["sid"] != target["sid"]]
                        assert nested, "no nested independent Session below outer shell"
                        result["nested_independent_shells"] = nested
                        send_signal(target, signal.SIGKILL)
                    elif mode == "A":
                        client.tool("agent_cancel", {"task_id": task["task_id"]})
                    elif mode == "C":
                        send_signal(target, signal.SIGKILL)
                    elif mode == "D":
                        ipc_shutdown(db)
                    elif mode == "E":
                        exit_queue = select.kqueue()
                        watched = [row for row in rows if row["parent"] == service.pid]
                        exit_queue.control([select.kevent(row["pid"], filter=select.KQ_FILTER_PROC,
                            flags=select.KQ_EV_ADD, fflags=select.KQ_NOTE_EXIT | 0x04000000)
                            for row in watched], 0)
                        send_signal(service_identity, signal.SIGKILL)
                        service.wait(timeout=10)
                        exits = exit_queue.control(None, len(watched), 10)
                        result["guardian_exit_events"] = [{"pid": event.ident, "flags": event.fflags,
                                                          "raw_status": event.data} for event in exits]
                        exit_queue.close()
                        assert len(exits) == len(watched), "guardian did not exit"
                        assert all(event.data == 0 for event in exits), result["guardian_exit_events"]
                    elif mode == "F":
                        client.close()
                        client = None
                        time.sleep(.5)
                        assert len(surviving(rows)) == len(rows), "MCP disconnect killed owned processes"
                        client = crash.Mcp(binary, db, root / (mode + "-mcp-reconnect.log"))
                        assert client.tool("agent_status", {"task_id": task["task_id"]})["state"] == "running"
                        result["disconnect_preserved_processes"] = True
                        client.tool("agent_cancel", {"task_id": task["task_id"]})
                    elif mode == "H":
                        second = start_task(client, repo)
                        second_rows = capture(service, {row["unique"] for row in rows})
                        result.update(second_task=second, second_before=second_rows)
                        send_signal(target, signal.SIGKILL)
                        result["survivors"] = wait_gone(rows)
                        assert not result["survivors"], result["survivors"]
                        assert len(surviving(second_rows)) == len(second_rows), "other session was killed"
                        assert client.tool("agent_status", {"task_id": second["task_id"]})["state"] == "running"
                        result["concurrent_session_preserved"] = True
                        client.tool("agent_cancel", {"task_id": second["task_id"]})
                        result["second_survivors"] = wait_gone(second_rows)
                        assert not result["second_survivors"], result["second_survivors"]
                        rows += second_rows
                    result["survivors"] = wait_gone(rows)
                    assert not result["survivors"], result["survivors"]
                    if mode in ("A", "F", "H"):
                        status = crash.wait_status(client, (second if mode == "H" else task)["task_id"],
                                                  lambda value: value["state"] != "running")
                        result["cancel_state"] = status["state"]
                        assert status["state"] == "cancelled", status["state"]
                        if mode == "A":
                            continued = client.tool("agent_continue", {"session_id": task["session_id"],
                                "prompt": "Reply exactly SESSION_RECOVERED without tools."})
                            recovered = crash.wait_status(client, continued["task_id"],
                                lambda value: value["state"] in ("completed", "failed", "interrupted", "cancelled"))
                            assert recovered["state"] == "completed", recovered["state"]
                            assert recovered["thread_id"] == status["thread_id"], "continuation changed Thread"
                            assert "SESSION_RECOVERED" in recovered["final_answer"], recovered["final_answer"]
                            new_rows = crash.descendants(service.pid, table())
                            new_provider = provider(new_rows)
                            assert new_provider["unique"] != target["unique"], "continuation reused closed process"
                            result.update(continued_task=continued, continued_thread=recovered["thread_id"],
                                          resumed_provider=new_provider, resumed_scope=new_rows)
                            rows += new_rows
                        ipc_shutdown(db)
                        service.wait(timeout=10)
                        result["after_shutdown_survivors"] = wait_gone(rows)
                        assert not result["after_shutdown_survivors"], result["after_shutdown_survivors"]
                    result["pass"] = True
                except Exception as error:
                    result["error"] = str(error)
                    result["survivors"] = surviving(rows)
                finally:
                    result["sentinels_before"] = sentinels
                    result["sentinels_alive"] = len(surviving(sentinels)) == 2
                    result["pass"] &= result["sentinels_alive"]
                    if client:
                        try:
                            result["task_state"] = client.tool("agent_status", {"task_id": task["task_id"]})["state"]
                        except Exception:
                            pass  # Service exit is an explicit scenario; process checks remain authoritative.
                        client.close()
                    if service and service.poll() is None:
                        try:
                            ipc_shutdown(db)
                            service.wait(timeout=10)
                        except Exception:
                            identity = table().get(service.pid)
                            if identity and same(service_identity, identity):
                                send_signal(service_identity, signal.SIGKILL)
                            service.wait(timeout=10)
                    cleanup_rows = {row["unique"]: row for row in rows + result.get("second_before", [])}
                    for row in surviving(list(cleanup_rows.values())):
                        send_signal(row, signal.SIGKILL)
                    results.append(result)
                    (root / "evidence.json").write_text(json.dumps(results, indent=2))
                    print(json.dumps({"scenario": mode, "pass": result["pass"], "error": result.get("error")}), flush=True)
    finally:
        for row in surviving(sentinels):
            send_signal(row, signal.SIGKILL)
        for child in (sentinel_shell, sentinel_codex):
            child.wait(timeout=10)
    return results

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--codex", default="/opt/homebrew/bin/codex")
    parser.add_argument("--output", required=True)
    parser.add_argument("--scenario", default="ABCDEFGH")
    args = parser.parse_args()
    results = run(str(Path(args.binary).resolve()), args.codex, Path(args.output).resolve(), list(args.scenario))
    print(json.dumps({"verdict": "PASS" if all(row["pass"] for row in results) else "FAIL",
                      "scenarios": len(results), "evidence": str(Path(args.output).resolve() / "evidence.json")}))
    raise SystemExit(0 if all(row["pass"] for row in results) else 1)
