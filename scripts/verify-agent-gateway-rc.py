#!/usr/bin/env python3
"""macOS RC acceptance against an installed release binary.

Runs the real Codex / Claude Code CLIs through the installed `agentgate-serve`
MCP server. Uses fresh databases under --output and never reads credentials.

Process safety: signals are sent only to a Popen child created with
start_new_session=True (verified to be its own group leader) or to the exact
service child handle. Service discovery is read-only; this script never kills by
process name and never signals an inherited process group.

Scenarios:
  install  installed binary, MCP stdout, auto-start, service survives client exit
  codex    real Codex run / continue / pagination / cancel / read-only / restart
  claude   real Claude Code run / continue / cancel / read-only / restart
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import sqlite3
import subprocess
import threading
import time
import uuid

spec = importlib.util.spec_from_file_location(
    "crash", Path(__file__).with_name("verify-agent-gateway-crash.py"))
crash = importlib.util.module_from_spec(spec)
spec.loader.exec_module(crash)


class Mcp(crash.Mcp):
    def call_raw(self, method, params):
        import select
        self.seq += 1
        self.send({"jsonrpc": "2.0", "id": self.seq, "method": method, "params": params})
        deadline = time.monotonic() + 35
        while time.monotonic() < deadline:
            if not select.select([self.child.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
                break
            raw = self.child.stdout.readline()
            if not raw:
                raise RuntimeError("MCP unexpectedly closed stdout")
            response = json.loads(raw)
            if response.get("id") == self.seq:
                return response
        raise TimeoutError(method)

    def tool_raw(self, name, arguments):
        result = self.call("tools/call", {"name": name, "arguments": arguments})
        return result, json.loads(result["content"][0]["text"])


class ExistingMcp:
    """MCP client over an already-spawned child (used for terminal-group tests)."""

    def __init__(self, child):
        self.child = child
        self.seq = 0
        self.call("initialize", {"protocolVersion": "2025-06-18", "capabilities": {},
                                 "clientInfo": {"name": "rc-hangup", "version": "1"}})
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.child.stdin.write((json.dumps(message) + "\n").encode())
        self.child.stdin.flush()

    def call(self, method, params):
        import select
        self.seq += 1
        self.send({"jsonrpc": "2.0", "id": self.seq, "method": method, "params": params})
        deadline = time.monotonic() + 35
        while time.monotonic() < deadline:
            if not select.select([self.child.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
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


def service_sockets(db):
    paths = Path(db) / "agent-service"
    standard = paths / "agent-service.sock"
    if standard.exists():
        yield standard
    log = paths / "service.log"
    if log.exists():
        for line in reversed(log.read_text(errors="replace").splitlines()):
            try:
                socket_path = json.loads(line).get("socket")
            except (json.JSONDecodeError, AttributeError):
                continue
            if socket_path:
                candidate = Path(socket_path)
                if candidate != standard and candidate.exists():
                    yield candidate
                break


def ipc_call(db, method, params):
    import socket
    paths = Path(db) / "agent-service"
    token = (paths / "token").read_text().strip()
    last_error = None
    for socket_path in service_sockets(db):
        try:
            with socket.socket(socket.AF_UNIX) as connection:
                connection.settimeout(10)
                connection.connect(str(socket_path))
                with connection.makefile("rwb", buffering=0) as stream:
                    stream.write((json.dumps({"id": "hello", "method": "hello",
                                              "params": {"token": token}}) + "\n").encode())
                    hello = json.loads(stream.readline())
                    assert "error" not in hello, hello
                    stream.write((json.dumps({"id": "call", "method": method,
                                              "params": params}) + "\n").encode())
                    response = json.loads(stream.readline())
                    assert "error" not in response, response
                    return response["result"]
        except (FileNotFoundError, ConnectionRefusedError, OSError) as error:
            last_error = error
    raise FileNotFoundError(f"no reachable agent service socket for {db}: {last_error}")


def find_services(binary, db):
    binary = str(Path(binary).resolve())
    output = subprocess.check_output(["ps", "-axo", "pid=,command="], text=True)
    found = []
    for line in output.splitlines():
        pid, _, command = line.strip().partition(" ")
        if command.startswith(binary) and " agent-service " in f" {command} " \
                and f"--db-path {db}" in command:
            found.append({"pid": int(pid), "command": command})
    return found


def find_service(binary, db):
    found = find_services(binary, db)
    return found[0] if found else None


def wait_service_gone(db, timeout=20):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            ipc_call(db, "service_status", {})
        except (OSError, AssertionError, ValueError, FileNotFoundError):
            return True
        time.sleep(0.2)
    return False


def make_repo(path, nonce):
    path.mkdir(parents=True)
    subprocess.run(["git", "init", "-q", str(path)], check=True)
    subprocess.run(["git", "-C", str(path), "config", "user.email", "rc@example.test"], check=True)
    subprocess.run(["git", "-C", str(path), "config", "user.name", "RC"], check=True)
    (path / "fixture.txt").write_text(f"SOURCE_NONCE={nonce}\n", encoding="utf-8")
    (path / "notes.md").write_text("# Fixture\n\nA read-only acceptance repository.\n", encoding="utf-8")
    subprocess.run(["git", "-C", str(path), "add", "."], check=True)
    subprocess.run(["git", "-C", str(path), "commit", "-qm", "fixture"], check=True)
    return path


def db_task(db, task_id):
    connection = sqlite3.connect(f"file:{Path(db) / 'agentgate.db'}?mode=ro", uri=True)
    try:
        row = connection.execute(
            "SELECT status, final_answer, completed_at, session_id, provider_turn_id "
            "FROM agent_tasks WHERE task_id=?", (task_id,)).fetchone()
        assert row, f"task {task_id} missing from SQLite"
        return {"state": row[0], "final_answer": row[1], "finished_at": row[2],
                "session_id": row[3], "turn_id": row[4]}
    finally:
        connection.close()


def db_session(db, session_id):
    connection = sqlite3.connect(f"file:{Path(db) / 'agentgate.db'}?mode=ro", uri=True)
    try:
        row = connection.execute(
            "SELECT agent, cwd, active_task_id, provider_session_id "
            "FROM agent_sessions WHERE session_id=?", (session_id,)).fetchone()
        assert row, f"session {session_id} missing from SQLite"
        return {"agent": row[0], "cwd": row[1], "active_task_id": row[2], "thread_id": row[3]}
    finally:
        connection.close()


def wait_terminal(client, task_id, timeout=420):
    return crash.wait_status(
        client, task_id,
        lambda value: value["state"] in ("completed", "failed", "cancelled", "interrupted"),
        timeout=timeout)


def wait_started_command(client, task_id, timeout=240):
    return crash.wait_status(
        client, task_id,
        lambda value: any(event["kind"] == "item/started"
                          and "commandExecution" in event["message"] for event in value["events"]),
        timeout=timeout)


def paginate(client, task_id, limit):
    pages = []
    offset = 0
    for _ in range(10000):
        status = client.tool("agent_status", {
            "task_id": task_id, "final_answer_offset": offset,
            "final_answer_limit": limit})
        pages.append(status.get("final_answer") or "")
        if status.get("final_answer_complete"):
            return "".join(pages), status["final_answer_total_chars"]
        next_offset = status.get("final_answer_next_offset")
        assert next_offset is not None and next_offset > offset, status
        offset = next_offset
    raise AssertionError("pagination did not terminate")


def signal_own_group(child, sig):
    """Send a signal to a Popen child only after proving it owns its group."""
    pgid = os.getpgid(child.pid)
    assert pgid == child.pid, "child is not its own process-group leader"
    assert pgid != os.getpgid(os.getpid()), "refusing to signal the test runner group"
    os.killpg(pgid, sig)


def cancel_and_check(client, service_pid, task_id, evidence, label):
    children = [row for row in crash.descendants(service_pid, crash.process_table())
                if not row["state"].startswith("Z")]
    requested = client.tool("agent_cancel", {"task_id": task_id})
    terminal = wait_terminal(client, task_id, timeout=120)
    assert terminal["state"] == "cancelled", terminal
    deadline = time.monotonic() + 25
    survivors = []
    while time.monotonic() < deadline:
        current = crash.process_table()
        survivors = [row for row in children
                     if crash.same_process(row, current.get(row["pid"]))
                     and not current[row["pid"]]["state"].startswith("Z")]
        if not survivors:
            break
        time.sleep(0.1)
    evidence[label] = {"requested": requested, "terminal": terminal,
                       "captured_processes": children, "survivors": survivors}
    assert not survivors, {"label": label, "survivors": survivors}


def scenario_install(binary, root, codex, claude, evidence):
    db = root / "install" / "db"
    db.mkdir(parents=True)
    version = subprocess.run([binary, "--version"], capture_output=True, text=True)
    assert version.returncode == 0, version
    for command in ("agent-service", "mcp-agent"):
        help_text = subprocess.run([binary, command, "--help"], capture_output=True, text=True)
        assert help_text.returncode == 0, (command, help_text.stderr)
    assert not (db / "agent-service" / "agent-service.sock").exists()
    client = Mcp(binary, db, root / "install" / "mcp.log")
    try:
        tools = [tool["name"] for tool in client.call("tools/list", {})["tools"]]
        assert set(tools) == {"agent_list", "agent_run", "agent_status",
                              "agent_continue", "agent_cancel"}, tools
        agents = client.tool("agent_list", {})["agents"]
        ids = {agent["id"] for agent in agents}
        assert "codex-native" in ids, agents
        assert "claude-code" in ids, agents
        service = find_service(binary, db)
        assert service, "MCP did not auto-start an agent service"
        status = ipc_call(db, "service_status", {})
        assert status["service"] == "muxlayer-agent-service", status
        client.close()
        time.sleep(1.5)
        assert ipc_call(db, "service_status", {})["service"] == "muxlayer-agent-service", \
            "service exited with the MCP client"
        ipc_call(db, "service_shutdown", {})
        assert wait_service_gone(db), "service did not stop"
        # stdout purity with no protocol input
        quiet = subprocess.run([binary, "mcp-agent", "--db-path", str(db)],
                               input=b"", capture_output=True, timeout=30)
        assert quiet.stdout == b"", quiet.stdout[:200]
        race_db = root / "install" / "db-race"
        race_db.mkdir(parents=True)
        first = Mcp(binary, race_db, root / "install" / "race-1.log")
        second = Mcp(binary, race_db, root / "install" / "race-2.log")
        results = {}

        def query(name, client):
            results[name] = client.tool("agent_list", {})["agents"]

        try:
            threads = [threading.Thread(target=query, args=("a", first)),
                       threading.Thread(target=query, args=("b", second))]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join()
            assert set(results) == {"a", "b"}, results
            deadline = time.monotonic() + 10
            services = find_services(binary, race_db)
            while len(services) > 1 and time.monotonic() < deadline:
                time.sleep(0.1)
                services = find_services(binary, race_db)
            assert len(services) == 1, services
            evidence["install"] = {
                "binary": binary, "version": version.stdout.strip(),
                "tools": tools, "agents": agents, "service": status,
                "client_exit_kept_service": True,
                "stdout_bytes_on_empty_input": len(quiet.stdout),
                "autostart_race_clients": 2,
                "autostart_services": len(services),
                "autostart_agents_ok": all({agent["id"] for agent in value} >=
                                           {"codex-native", "claude-code"}
                                           for value in results.values()),
                "codex_override": codex, "claude_override": claude,
            }
        finally:
            first.close()
            second.close()
            try:
                ipc_call(race_db, "service_shutdown", {})
            except Exception:
                pass
            wait_service_gone(race_db)
    finally:
        try:
            ipc_call(db, "service_shutdown", {})
        except Exception:
            pass
        wait_service_gone(db)


def codex_catalog(codex):
    """Read the official app-server model catalog without touching credentials."""
    import select
    version = subprocess.run([codex, "--version"], capture_output=True, text=True)
    process = subprocess.Popen([codex, "app-server", "--listen", "stdio://"],
                               stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL)
    seq = 0

    def call(method, params):
        nonlocal seq
        seq += 1
        process.stdin.write((json.dumps({"id": seq, "method": method,
                                         "params": params}) + "\n").encode())
        process.stdin.flush()
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if not select.select([process.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
                break
            raw = process.stdout.readline()
            if not raw:
                raise RuntimeError("codex app-server closed stdout")
            message = json.loads(raw)
            if message.get("id") == seq:
                assert "error" not in message, message
                return message["result"]
        raise TimeoutError(method)

    try:
        call("initialize", {"clientInfo": {"name": "agentgate-rc", "title": "AgentGate RC",
                                           "version": "1"}, "capabilities": {}})
        process.stdin.write((json.dumps({"method": "initialized", "params": {}}) + "\n").encode())
        process.stdin.flush()
        config = call("config/read", {"includeLayers": False})["config"]
        models = call("model/list", {"includeHidden": False})["data"]
        default = next(model["model"] for model in models if model.get("isDefault"))
        provider = config.get("model_provider")
        entry = (config.get("model_providers") or {}).get(provider) or {}
        return {"cli": version.stdout.strip(), "provider": provider,
                "provider_base_url": entry.get("base_url"),
                "requires_openai_auth": entry.get("requires_openai_auth"),
                "default_model": default}
    finally:
        if process.poll() is None:
            process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()


def scenario_hangup(binary, root, evidence):
    """Terminal SIGINT/SIGHUP to the client group must not stop the service."""
    base = root / "hangup"
    db = base / "db"
    db.mkdir(parents=True)
    repo = make_repo(base / "repo", uuid.uuid4().hex)
    log = open(base / "mcp.log", "ab")
    # A session leader models the shell's foreground process group. The auto-started
    # service must not remain a member of that group.
    child = subprocess.Popen([binary, "mcp-agent", "--db-path", str(db)],
                             stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=log, start_new_session=True)
    client = ExistingMcp(child)
    client2 = None
    service = None
    try:
        client.tool("agent_list", {})
        service = find_service(binary, db)
        assert service, "MCP did not auto-start a service"
        identity = crash.process_table().get(service["pid"])
        assert identity, "service disappeared before the hangup test"
        client_group = os.getpgid(child.pid)
        assert client_group == child.pid, "test MCP must be its own group leader"
        assert identity["group"] != client_group, "service still shares the terminal group"
        task = client.tool("agent_run", {
            "agent": "codex-native", "cwd": str(repo), "permission": "read_only",
            "prompt": "Run exactly this command with exec_command and wait for it: sleep 120. "
                      "This is a terminal hangup acceptance test. Do not modify files."})
        started = wait_started_command(client, task["task_id"])
        signal_own_group(child, signal.SIGINT)
        time.sleep(2)
        after_sigint = ipc_call(db, "agent_status", {"task_id": task["task_id"]})
        assert after_sigint["state"] in ("running", "completed"), after_sigint
        current = crash.process_table().get(service["pid"])
        assert crash.same_process(identity, current), "SIGINT killed the agent service"
        client.close()
        client = None
        child2 = subprocess.Popen([binary, "mcp-agent", "--db-path", str(db)],
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=open(base / "mcp-2.log", "ab"),
                                  start_new_session=True)
        client2 = ExistingMcp(child2)
        client2_group = os.getpgid(child2.pid)
        assert client2_group == child2.pid, "second MCP must be its own group leader"
        assert identity["group"] != client2_group, "service shares the second client group"
        running = client2.tool("agent_status", {"task_id": task["task_id"]})
        assert running["state"] in ("running", "completed"), running
        signal_own_group(child2, signal.SIGHUP)
        time.sleep(2)
        after_sighup = ipc_call(db, "agent_status", {"task_id": task["task_id"]})
        assert after_sighup["state"] in ("running", "completed"), after_sighup
        current2 = crash.process_table().get(service["pid"])
        assert crash.same_process(identity, current2), "SIGHUP killed the agent service"
        client2.close()
        client2 = None
        client = Mcp(binary, db, base / "mcp-reconnect.log")
        running2 = client.tool("agent_status", {"task_id": task["task_id"]})
        assert running2["state"] in ("running", "completed"), running2
        cancel_and_check(client, service["pid"], task["task_id"], evidence, "hangup_cancel")
        evidence["hangup"] = {
            "service_before": identity,
            "client_group": client_group,
            "second_client_group": client2_group,
            "service_after_sigint": current,
            "service_after_sighup": current2,
            "task_after_sigint": after_sigint,
            "task_after_sighup": after_sighup,
            "reconnected": running2,
            "cancel": evidence.pop("hangup_cancel"),
        }
    finally:
        if client:
            client.close()
        if client2:
            client2.close()
        try:
            ipc_call(db, "service_shutdown", {})
        except Exception:
            pass
        deadline = time.monotonic() + 15
        while service and time.monotonic() < deadline:
            current = crash.process_table().get(service["pid"])
            if current is None or current["state"].startswith("Z"):
                break
            time.sleep(0.2)


def restart_service(binary, db, service, client, log_path):
    try:
        ipc_call(db, "service_shutdown", {})
    except Exception:
        pass
    if service.poll() is None:
        try:
            service.wait(timeout=20)
        except subprocess.TimeoutExpired:
            service.terminate()
            service.wait(timeout=10)
    assert wait_service_gone(db), "service did not stop"
    (db / "agent-service" / "token").unlink(missing_ok=True)
    service = crash.start_service(binary, db, open(log_path, "ab"))
    if client is not None:
        client.close()
    client = Mcp(binary, db, Path(str(log_path) + ".mcp.log"))
    return service, client


def scenario_cancel_start(binary, root, evidence):
    """Cancel immediately after submit, through the turn-start window."""
    base = root / "cancel-start"
    db = base / "db"
    db.mkdir(parents=True)
    repo = make_repo(base / "repo", uuid.uuid4().hex)
    service = crash.start_service(binary, db, open(base / "service.log", "ab"))
    client = Mcp(binary, db, base / "mcp.log")
    attempts = []
    try:
        for attempt in range(3):
            task = client.tool("agent_run", {
                "agent": "codex-native", "cwd": str(repo), "permission": "read_only",
                "prompt": "Run exactly this command with exec_command and wait for it: sleep 120. "
                          "This is a startup cancellation acceptance test. Do not modify files."})
            started = time.monotonic()
            error = None
            requested = None
            try:
                requested = client.tool("agent_cancel", {"task_id": task["task_id"]})
            except AssertionError as exc:
                error = str(exc)
            latency_ms = int((time.monotonic() - started) * 1000)
            terminal = wait_terminal(client, task["task_id"], timeout=120)
            attempts.append({
                "attempt": attempt, "task": task, "cancel_error": error,
                "requested_state": (requested or {}).get("state"),
                "cancel_latency_ms": latency_ms, "terminal_state": terminal["state"],
            })
            assert error is None, attempts[-1]
            assert terminal["state"] == "cancelled", attempts[-1]
        evidence["cancel_start"] = {"attempts": attempts}
    finally:
        client.close()
        if service.poll() is None:
            service.terminate()
            try:
                service.wait(timeout=10)
            except subprocess.TimeoutExpired:
                service.kill()
                service.wait(timeout=10)


def scenario_codex(binary, root, codex, evidence):
    base = root / "codex"
    db = base / "db"
    db.mkdir(parents=True)
    nonce = uuid.uuid4().hex
    reply = f"{nonce} 中文🙂"
    repo = make_repo(base / "repo", nonce)
    log = open(base / "service.log", "ab")
    service = crash.start_service(binary, db, log)
    client = Mcp(binary, db, base / "mcp.log")
    try:
        agents = client.tool("agent_list", {})["agents"]
        first = client.tool("agent_run", {
            "agent": "codex-native", "cwd": str(repo), "permission": "read_only",
            "prompt": f"Read fixture.txt in this repository with a shell command. "
                      f"Then reply with exactly this single line and nothing else: {reply}"})
        first_status = wait_terminal(client, first["task_id"])
        assert first_status["state"] == "completed", first_status
        answer = first_status["final_answer"]
        assert nonce in answer and "中文🙂" in answer, answer
        assert first_status["thread_id"], first_status
        stored = db_task(db, first["task_id"])
        assert stored["state"] == "completed", stored
        assert stored["final_answer"] == answer, (stored, answer)
        assert stored["finished_at"], stored
        session = db_session(db, first["session_id"])
        assert session["active_task_id"] is None, session
        assert session["thread_id"] == first_status["thread_id"], session

        continued = client.tool("agent_continue", {
            "session_id": first["session_id"],
            "prompt": f"Without reading files again, reply with exactly this line: {reply}"})
        second_status = wait_terminal(client, continued["task_id"], timeout=300)
        assert second_status["state"] == "completed", second_status
        assert second_status["thread_id"] == first_status["thread_id"], second_status
        assert second_status["session_id"] == first["session_id"]
        assert nonce in second_status["final_answer"], second_status

        reassembled, total = paginate(client, first["task_id"], 3)
        assert reassembled == answer, (len(reassembled), len(answer), total)
        assert total == len(answer), (total, len(answer))

        read_only = client.tool("agent_run", {
            "agent": "codex-native", "cwd": str(repo), "permission": "read_only",
            "prompt": "Use the exec_command tool to run exactly this command: "
                      "printf HACKED > fixture.txt && touch created.txt. "
                      "Report the exact stdout, stderr and exit code. Do not do anything else."})
        read_only_status = wait_terminal(client, read_only["task_id"])
        assert read_only_status["state"] in ("completed", "failed"), read_only_status
        assert (repo / "fixture.txt").read_text(encoding="utf-8") == f"SOURCE_NONCE={nonce}\n"
        assert not (repo / "created.txt").exists()
        git_status = subprocess.run(["git", "-C", str(repo), "status", "--porcelain"],
                                    capture_output=True, text=True).stdout
        assert git_status == "", git_status

        raw, payload = client.tool_raw("agent_run", {
            "agent": "codex-native", "cwd": str(base / "missing"), "permission": "read_only",
            "prompt": "test"})
        assert raw.get("isError") is True, raw
        assert payload["error"]["code"] == "invalid_request", payload
        raw, payload = client.tool_raw("agent_run", {
            "agent": "no-such-agent", "cwd": str(repo), "permission": "read_only",
            "prompt": "test"})
        assert raw.get("isError") is True and payload["error"]["code"] == "unknown_agent", payload
        response = client.call_raw("tools/call", {
            "name": "agent_run", "arguments": {
                "agent": "codex-native", "cwd": str(repo), "permission": "execute",
                "prompt": "test"}})
        assert response["error"]["code"] == -32602, response
        assert "unsupported permission" in response["error"]["message"], response

        service, client = restart_service(binary, db, service, client,
                                         base / "service-sleep.log")
        sleep = client.tool("agent_run", {
            "agent": "codex-native", "cwd": str(repo), "permission": "read_only",
            "prompt": "Run exactly this command with exec_command and wait for it to finish: "
                      "sleep 120. This is a process lifecycle acceptance test. Do not modify files."})
        started = wait_started_command(client, sleep["task_id"])
        assert started["state"] in ("queued", "running"), started
        raw, payload = client.tool_raw("agent_continue", {
            "session_id": sleep["session_id"],
            "prompt": "This must be rejected while the session is busy."})
        assert raw.get("isError") is True and payload["error"]["code"] == "busy", payload
        client.close()
        time.sleep(2)
        client = Mcp(binary, db, base / "mcp-reconnect.log")
        running = client.tool("agent_status", {"task_id": sleep["task_id"]})
        assert running["state"] in ("running", "completed"), running
        cancel_and_check(client, service.pid, sleep["task_id"], evidence, "codex_cancel")

        service, client = restart_service(binary, db, service, client,
                                          base / "service-restart.log")
        restored = client.tool("agent_status", {"task_id": first["task_id"]})
        assert restored["state"] == "completed", restored
        assert restored["final_answer"] == answer, "restarted readback differs"
        resumed = client.tool("agent_continue", {
            "session_id": first["session_id"],
            "prompt": "Reply with exactly the word PERSISTED and nothing else."})
        resumed_status = wait_terminal(client, resumed["task_id"], timeout=300)
        assert resumed_status["state"] == "completed", resumed_status
        assert resumed_status["thread_id"] == first_status["thread_id"], resumed_status

        evidence["codex"] = {
            "catalog": codex_catalog(codex),
            "run": first_status, "continue": second_status, "paginated_total_chars": total,
            "read_only": read_only_status,
            "read_only_fixture_unchanged": True, "git_status": git_status,
            "restart_readback": restored, "restart_continue": resumed_status,
            "cancel": evidence.pop("codex_cancel"),
            "sqlite_task": stored, "sqlite_session": session,
        }
    finally:
        client.close()
        if service.poll() is None:
            service.terminate()
            try:
                service.wait(timeout=10)
            except subprocess.TimeoutExpired:
                service.kill()


def scenario_claude(binary, root, evidence):
    base = root / "claude"
    db = base / "db"
    db.mkdir(parents=True)
    nonce = uuid.uuid4().hex
    repo = make_repo(base / "repo", nonce)
    outside = base / "outside.txt"
    outside.write_text(f"OUTSIDE_NONCE={nonce}\n", encoding="utf-8")
    log = open(base / "service.log", "ab")
    service = crash.start_service(binary, db, log)
    client = Mcp(binary, db, base / "mcp.log")
    try:
        agents = client.tool("agent_list", {})["agents"]
        assert any(agent["id"] == "claude-code" for agent in agents), agents
        first = client.tool("agent_run", {
            "agent": "claude-code", "cwd": str(repo), "permission": "read_only",
            "prompt": "Read fixture.txt and reply with its exact SOURCE_NONCE. Do not modify any file."})
        first_status = wait_terminal(client, first["task_id"], timeout=420)
        assert first_status["state"] == "completed", first_status
        assert nonce in first_status["final_answer"], first_status
        assert first_status["thread_id"], first_status
        stored = db_task(db, first["task_id"])
        assert stored["final_answer"] == first_status["final_answer"], stored

        continued = client.tool("agent_continue", {
            "session_id": first["session_id"],
            "prompt": "Without reading files again, return the same SOURCE_NONCE."})
        second_status = wait_terminal(client, continued["task_id"], timeout=420)
        assert second_status["state"] == "completed", second_status
        assert second_status["thread_id"] == first_status["thread_id"], second_status
        assert nonce in second_status["final_answer"], second_status

        service, client = restart_service(binary, db, service, client,
                                         base / "service-cancel.log")
        hanging = client.tool("agent_run", {
            "agent": "claude-code", "cwd": str(repo), "permission": "read_only",
            "prompt": "Read every file in this repository one by one and produce an exhaustive "
                      "report. Do not modify any file."})
        observe = time.monotonic()
        while True:
            status = client.tool("agent_status", {
                "task_id": hanging["task_id"], "events_limit": 10})
            if status["state"] == "running" and status.get("events_total", 0) >= 1:
                break
            assert status["state"] not in ("completed", "failed", "cancelled", "interrupted"), \
                f"task finished before cancellation: {status}"
            assert time.monotonic() - observe < 90, f"task never started: {status}"
            time.sleep(0.1)
        cancel_and_check(client, service.pid, hanging["task_id"], evidence, "claude_cancel")

        write_attempt = client.tool("agent_run", {
            "agent": "claude-code", "cwd": str(repo), "permission": "read_only",
            "prompt": "Create a file named claude_created.txt containing HACKED using a shell "
                      "command or the Write tool. You must attempt it and report the outcome."})
        write_status = wait_terminal(client, write_attempt["task_id"], timeout=420)
        assert write_status["state"] in ("completed", "failed"), write_status
        assert not (repo / "claude_created.txt").exists()
        git_status = subprocess.run(["git", "-C", str(repo), "status", "--porcelain"],
                                    capture_output=True, text=True).stdout
        assert git_status == "", git_status

        outside_run = client.tool("agent_run", {
            "agent": "claude-code", "cwd": str(repo), "permission": "read_only",
            "prompt": f"Read the file at absolute path {outside} and reply with its exact contents. "
                      "It is outside this repository."})
        outside_status = wait_terminal(client, outside_run["task_id"], timeout=420)
        outside_read = (outside_status["state"] == "completed"
                        and nonce in (outside_status.get("final_answer") or ""))

        raw, payload = client.tool_raw("agent_status", {"task_id": "does-not-exist"})
        assert raw.get("isError") is True and payload["error"]["code"] == "not_found", payload

        service, client = restart_service(binary, db, service, client,
                                          base / "service-restart.log")
        restored = client.tool("agent_status", {"task_id": first["task_id"]})
        assert restored["state"] == "completed", restored
        assert restored["final_answer"] == first_status["final_answer"], "restarted readback differs"
        resumed = client.tool("agent_continue", {
            "session_id": first["session_id"],
            "prompt": "Reply with exactly the word PERSISTED and nothing else."})
        resumed_status = wait_terminal(client, resumed["task_id"], timeout=420)
        assert resumed_status["state"] == "completed", resumed_status
        assert resumed_status["thread_id"] == first_status["thread_id"], resumed_status

        evidence["claude"] = {
            "run": first_status, "continue": second_status,
            "cancel": evidence.pop("claude_cancel"),
            "write_attempt": write_status, "write_fixture_unchanged": True,
            "git_status": git_status,
            "outside_cwd_file_read": outside_read,
            "outside_cwd_note": "Documented contract: Claude read tools are read-only but not "
                                "confined to cwd; this records the observed behavior.",
            "restart_readback": restored, "restart_continue": resumed_status,
            "sqlite_task": stored,
        }
    finally:
        client.close()
        if service.poll() is None:
            service.terminate()
            try:
                service.wait(timeout=10)
            except subprocess.TimeoutExpired:
                service.kill()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--scenario", choices=["install", "hangup", "cancel-start", "codex", "claude", "all"], default="all")
    parser.add_argument("--codex", default="/opt/homebrew/bin/codex")
    parser.add_argument("--claude", default=shutil.which("claude") or "claude")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    root = Path(args.output).resolve()
    root.mkdir(parents=True, exist_ok=True)
    evidence = {"started_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                "binary_sha256": subprocess.run(
                    ["shasum", "-a", "256", binary], capture_output=True, text=True
                ).stdout.split()[0]}
    try:
        if args.scenario in ("install", "all"):
            scenario_install(binary, root, args.codex, args.claude, evidence)
        if args.scenario in ("hangup", "all"):
            scenario_hangup(binary, root, evidence)
        if args.scenario in ("cancel-start", "all"):
            scenario_cancel_start(binary, root, evidence)
        if args.scenario in ("codex", "all"):
            scenario_codex(binary, root, args.codex, evidence)
        if args.scenario in ("claude", "all"):
            scenario_claude(binary, root, evidence)
        evidence["verdict"] = "PASS"
    except Exception as error:
        evidence["verdict"] = "FAIL"
        evidence["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        evidence["finished_at"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
        (root / "rc-evidence.json").write_text(
            json.dumps(evidence, ensure_ascii=False, indent=2))
        print(json.dumps({"verdict": evidence["verdict"],
                          "scenario": args.scenario,
                          "evidence": str(root / "rc-evidence.json")},
                         ensure_ascii=False))


if __name__ == "__main__":
    main()
