#!/usr/bin/env python3
"""Opt-in real Codex MCP crash acceptance. Uses native auth; never reads credentials."""
import argparse
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import time


def process_table():
    output = subprocess.check_output(["ps", "-axo", "pid=,ppid=,pgid=,stat=,lstart=,comm="], text=True)
    table = {}
    for line in output.splitlines():
        parts = line.split(None, 9)
        if len(parts) == 10:
            pid, parent, group, state = parts[:4]
            started, command = " ".join(parts[4:9]), parts[9]
            table[int(pid)] = {"pid": int(pid), "parent": int(parent), "group": int(group),
                               "state": state, "command": command, "started": started}
    return table


def same_process(captured, current):
    return bool(current and all(current[key] == captured[key]
                                for key in ("pid", "started", "command", "group")))


def descendants(pid, table):
    found = {pid}
    while True:
        more = {key for key, row in table.items() if row["parent"] in found}
        if more <= found:
            return [row for key, row in table.items() if key in found and key != pid]
        found |= more


class Mcp:
    def __init__(self, binary, db, log):
        self.log = open(log, "ab")
        self.child = subprocess.Popen([binary, "mcp-agent", "--db-path", str(db)],
                                      stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.log)
        self.seq = 0
        self.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                 "clientInfo": {"name": "phase-three-crash", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.child.stdin.write((json.dumps(message) + "\n").encode())
        self.child.stdin.flush()

    def call(self, method, params):
        self.seq += 1
        self.send({"jsonrpc": "2.0", "id": self.seq, "method": method, "params": params})
        deadline = time.monotonic() + 35
        while time.monotonic() < deadline:
            if not select.select([self.child.stdout], [], [], max(0, deadline-time.monotonic()))[0]:
                break
            raw = self.child.stdout.readline()
            if not raw:
                raise RuntimeError("MCP unexpectedly closed stdout")
            response = json.loads(raw)
            if response.get("id") == self.seq:
                assert "error" not in response, response
                return response["result"]
        raise TimeoutError(method)

    def tool(self, name, arguments):
        result = self.call("tools/call", {"name": name, "arguments": arguments})
        value = json.loads(result["content"][0]["text"])
        assert not result.get("isError"), value
        return value

    def close(self):
        if self.child.poll() is None:
            self.child.kill()
        self.child.wait(timeout=10)
        self.log.close()


def wait_status(client, task, predicate, timeout=180):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        status = client.tool("agent_status", {"task_id": task, "events_limit": 500})
        if predicate(status):
            return status
        time.sleep(0.2)
    raise TimeoutError(f"task {task}: {status}")


def start_service(binary, db, log):
    child = subprocess.Popen([binary, "agent-service", "--db-path", str(db)],
                             stdin=subprocess.DEVNULL, stdout=log, stderr=log)
    token = db / "agent-service" / "token"
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        assert child.poll() is None, "service exited before ready"
        if token.exists():
            return child
        time.sleep(0.1)
    raise TimeoutError("service startup")


def execute(binary, root, scenario="all"):
    repo = root / "repo"
    repo.mkdir()
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    (repo / "evidence.txt").write_text("Crash acceptance: read-only fixture.\n")
    results = []
    for mode in ("service_sigkill", "codex_sigkill"):
        if scenario != "all" and mode != scenario:
            continue
        db = root / mode
        db.mkdir()
        client = None
        service = None
        children = []
        with open(root / (mode + ".log"), "ab") as log:
            try:
                service = start_service(binary, db, log)
                client = Mcp(binary, db, root / "mcp.log")
                submitted = client.tool("agent_run", {
                    "agent": "codex-native", "cwd": str(repo), "permission": "read_only",
                    "prompt": "Execute the shell command sleep 120 now. This is a cancellation "
                              "and process-lifecycle test. Do not modify files or delegate. "
                              "Wait for the command to finish before answering."})
                started = wait_status(client, submitted["task_id"], lambda s: any(
                    e["kind"] == "item/started" and "commandExecution" in e["message"]
                    and "sleep" in e["message"] for e in s["events"]))
                children = descendants(service.pid, process_table())
                assert any("sleep" in row["command"] for row in children), children
                # Client loss must not stop an in-flight task or its provider.
                client.close()
                client = Mcp(binary, db, root / "mcp.log")
                assert client.tool("agent_status", {"task_id": submitted["task_id"]})["state"] == "running"
                if mode == "service_sigkill":
                    service.kill()
                    service.wait(timeout=10)
                else:
                    candidates = [row for row in children if Path(row["command"]).name == "codex"]
                    assert candidates, children
                    # Select only this service's captured descendant, never a desktop process.
                    target = candidates[0]
                    assert same_process(target, process_table().get(target["pid"])), "provider identity changed"
                    os.kill(target["pid"], signal.SIGKILL)
                deadline = time.monotonic() + 15
                survivors = []
                while time.monotonic() < deadline:
                    current = process_table()
                    survivors = [row for row in children if same_process(row, current.get(row["pid"]))
                                 and not current[row["pid"]]["state"].startswith("Z")]
                    if not survivors:
                        break
                    time.sleep(0.1)
                if survivors:
                    (root / "failure.json").write_text(json.dumps({
                        "verdict": "FAIL", "mode": mode, "submitted": submitted,
                        "started": started, "processes_before": children, "survivors": survivors
                    }, ensure_ascii=False, indent=2))
                assert not survivors, {"mode": mode, "survivors": survivors}
                client.close()
                client = None
                if mode == "service_sigkill":
                    # Remove only a stale ready marker, to wait for actual new readiness.
                    (db / "agent-service" / "token").unlink(missing_ok=True)
                    service = start_service(binary, db, log)
                client = Mcp(binary, db, root / "mcp.log")
                terminal = wait_status(client, submitted["task_id"],
                                       lambda s: s["state"] in ("interrupted", "failed", "completed"))
                assert terminal["state"] == "interrupted", terminal
                continued = client.tool("agent_continue", {
                    "session_id": submitted["session_id"],
                    "prompt": "Reply with exactly RECOVERED. Do not run any commands."})
                final = wait_status(client, continued["task_id"],
                                    lambda s: s["state"] in ("completed", "failed", "interrupted"))
                assert final["state"] == "completed", final
                assert "RECOVERED" in final["final_answer"], final
                assert final["thread_id"] == started["thread_id"], (started, final)
                results.append({"mode": mode, "submitted": submitted, "started": started,
                                "processes_before": children, "survivors": survivors,
                                "recovered_task": terminal, "continued": final})
                (root / "evidence.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
            finally:
                if client:
                    client.close()
                if service and service.poll() is None:
                    service.terminate()
                    try:
                        service.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        service.kill()
                        service.wait(timeout=10)
                # Test cleanup only: captured identities in our private service tree.
                for row in reversed(children):
                    live = process_table().get(row["pid"])
                    if same_process(row, live):
                        try:
                            os.kill(row["pid"], signal.SIGKILL)
                        except ProcessLookupError:
                            pass
    return results


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--scenario", choices=["all", "service_sigkill", "codex_sigkill"], default="all")
    args = parser.parse_args()
    root = Path(args.output).resolve()
    root.mkdir(parents=True, exist_ok=True)
    evidence = execute(str(Path(args.binary).resolve()), root, args.scenario)
    print(json.dumps({"verdict": "PASS", "scenarios": len(evidence),
                      "evidence": str(root / "evidence.json")}, ensure_ascii=False))
