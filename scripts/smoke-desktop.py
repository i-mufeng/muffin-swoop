#!/usr/bin/env python3
"""Launch a built Swoop with isolated data and verify bridge -> engine -> disk.

This opens a real application process but does not automate native UI gestures.
No browser, system notification, or OS integration acceptance is implied.
"""
import argparse
import hashlib
import http.server
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request


PAYLOAD = bytes(range(256)) * 4096 + b"swoop desktop smoke"


class Fixture(http.server.BaseHTTPRequestHandler):
    slow_site = threading.Event()
    site_started = threading.Event()
    def log_message(self, *_args):
        pass

    def do_GET(self):
        path = self.path.partition("?")[0]
        if path in ("/robots.txt", "/site.html", "/child.html", "/pending.m3u8"):
            if path == "/site.html":
                self.site_started.set()
                deadline = time.monotonic() + 10
                while self.slow_site.is_set() and time.monotonic() < deadline:
                    time.sleep(0.02)
            if path == "/pending.m3u8":
                time.sleep(3)
            body = {"/robots.txt": b"User-agent: *\nAllow: /\n",
                    "/site.html": b"<html><a href='/child.html'>local fixture child</a></html>",
                    "/child.html": b"<html>local fixture content</html>",
                    "/pending.m3u8": b"#EXTM3U\n"}[path]
            self.send_response(200)
            self.send_header("Content-Type", "text/plain" if path == "/robots.txt" else "text/html")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except (BrokenPipeError, ConnectionResetError):
                pass
            return
        if path != "/payload.bin":
            self.send_error(404)
            return
        start, end = 0, len(PAYLOAD) - 1
        partial = self.headers.get("Range", "").startswith("bytes=")
        if partial:
            begin, finish = self.headers["Range"][6:].split("-", 1)
            start, end = int(begin), min(int(finish or end), end)
        body = PAYLOAD[start:end + 1]
        self.send_response(206 if partial else 200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("ETag", '"desktop-smoke-v1"')
        self.send_header("Accept-Ranges", "bytes")
        if partial:
            self.send_header("Content-Range", f"bytes {start}-{end}/{len(PAYLOAD)}")
        self.end_headers()
        self.wfile.write(body)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("--output-dir", type=Path, default=Path("target/iteration-validation"))
    parser.add_argument("--with-cli-actions", action="store_true", help="also exercise the packaged CLI and a harmless scanner fixture")
    args = parser.parse_args()
    executable = args.executable.resolve(strict=True)
    args.output_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="desktop-smoke-", dir=args.output_dir.resolve()))
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        bridge_port = sock.getsockname()[1]
    settings = {
        "default_dir": str(root / "downloads"), "clipboard_watch": "0", "auto_resume": "0",
        "bridge_mode": "silent", "bridge_port": str(bridge_port), "notify_complete": "0",
        "notify_failure": "0", "connections": "3",
        "network_policy": json.dumps({"use_system_proxy": False}),
        "categories": json.dumps({"enabled": True, "rules": [{"name": "Smoke", "extensions": "bin", "domains": "", "directory": "Fixtures"}]}),
    }
    cli = executable.with_name("swoop-cli.exe" if os.name == "nt" else "swoop-cli")
    if not cli.is_file() and executable.parent.name == "MacOS" and executable.parent.parent.name == "Contents":
        bundle = executable.parent.parent.parent
        if bundle.suffix == ".app":
            cli = bundle.parent / "swoop-cli"
    marker = root / "scanned.jsonl"
    if args.with_cli_actions:
        assert cli.is_file(), cli
        scanner = root / "scanner fixture.py"
        scanner.write_text(
            "import hashlib,json,sys\nfrom pathlib import Path\n"
            "p=Path(sys.argv[1])\n"
            "with Path(sys.argv[2]).open('a',encoding='utf-8') as f:\n"
            " f.write(json.dumps({'file':str(p),'sha256':hashlib.sha256(p.read_bytes()).hexdigest()})+'\\n')\n",
            encoding="utf-8",
        )
        settings["completion_actions"] = json.dumps({
            "scanner": {"program": str(Path(sys.executable).resolve()),
                        "arguments": [str(scanner), "{file}", str(marker)], "timeout_secs": 10},
            "file_action": "None", "show_dialog": False, "idle_action": "None", "countdown_secs": 30,
        })
    database = root / "swoop.db"
    with sqlite3.connect(database) as db:
        db.execute("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        db.executemany("INSERT INTO settings VALUES (?,?)", settings.items())
    fixture = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    threading.Thread(target=fixture.serve_forever, daemon=True).start()
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    base = f"http://127.0.0.1:{bridge_port}"

    def request(path, body=None):
        data = None if body is None else json.dumps(body).encode()
        req = urllib.request.Request(base + path, data, {"Content-Type": "application/json"})
        with opener.open(req, timeout=2) as response:
            return json.load(response)

    def control(*command, code=0):
        result = subprocess.run([str(cli), "--data-dir", str(root), *command], capture_output=True, text=True, timeout=10)
        assert result.returncode == code, (command, result.returncode, result.stdout, result.stderr)
        return json.loads(result.stdout)

    process = None
    try:
        with (root / "application.log").open("wb") as log:
            process = subprocess.Popen([str(executable)], env={**os.environ, "SWOOP_DATA_DIR": str(root)}, stdout=log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 25
            while True:
                if process.poll() is not None:
                    raise RuntimeError(f"application exited {process.returncode}; see {root / 'application.log'}")
                try:
                    ping = request("/ping")
                    break
                except (OSError, urllib.error.URLError):
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.1)
            assert ping["app"] == "swoop" and ping["media_handoff"] is True
            response = request("/tasks", {"url": f"http://127.0.0.1:{fixture.server_port}/payload.bin"})
            task_id = response["id"]
            while True:
                with sqlite3.connect(database) as db:
                    row = db.execute("SELECT status, dir, file_name, error FROM tasks WHERE id=?", (task_id,)).fetchone()
                if row and row[0] == "completed":
                    break
                if row and row[0] == "failed":
                    raise AssertionError(row[3])
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"download did not finish: {row}")
                time.sleep(0.05)
            path = Path(row[1]) / row[2]
            assert path.parent == root / "downloads" / "Fixtures", path
            assert path.read_bytes() == PAYLOAD
            expected_tasks = 1
            if args.with_cli_actions:
                control("--json", "--version")
                control("pause", "0", code=2)
                control("queue-stop", "1")
                added = control("add", f"http://127.0.0.1:{fixture.server_port}/payload.bin", "--name", "CLI 文件 02.bin")
                cli_id = added["data"]["id"]
                assert cli_id != task_id
                state = control("status")["data"]
                queued = next(task for task in state["tasks"] if task["id"] == cli_id)
                assert queued["status"] == "queued", queued
                control("queue-start", "1")
                while True:
                    with sqlite3.connect(database) as db:
                        row2 = db.execute("SELECT status, dir, file_name, error FROM tasks WHERE id=?", (cli_id,)).fetchone()
                    if row2 and row2[0] == "completed":
                        break
                    if row2 and row2[0] == "failed":
                        raise AssertionError(row2[3])
                    if time.monotonic() >= deadline:
                        raise TimeoutError(f"CLI download did not finish: {row2}")
                    time.sleep(0.05)
                path2 = Path(row2[1]) / row2[2]
                assert path2.parent == path.parent and path2.name == "CLI 文件 02.bin", path2
                assert path2.read_bytes() == PAYLOAD
                # A regular second launch must hand off, without opening a second engine.
                second = subprocess.run([str(executable)], env={**os.environ, "SWOOP_DATA_DIR": str(root)}, capture_output=True, timeout=10)
                assert second.returncode == 0 and process.poll() is None
                expected_tasks = 2
                while not marker.exists() or len(marker.read_text().splitlines()) != expected_tasks:
                    if time.monotonic() >= deadline:
                        raise TimeoutError("completion scanner fixture did not finish")
                    time.sleep(0.05)
                scans = [json.loads(line) for line in marker.read_text().splitlines()]
                assert {item["file"] for item in scans} == {str(path), str(path2)}
                assert all(item["sha256"] == hashlib.sha256(PAYLOAD).hexdigest() for item in scans)
            if args.with_cli_actions:
                def project_done(operation):
                    until = time.monotonic() + 15
                    while True:
                        state = control("grabber-status")["data"]
                        assert state["operation"]["id"] == operation, state
                        if not state["busy"]:
                            return state
                        if time.monotonic() > until:
                            raise TimeoutError(f"grabber operation did not finish: {state}")
                        time.sleep(0.05)

                def media_paused(job):
                    until = time.monotonic() + 10
                    while True:
                        state = control("media-status", str(job))["data"]
                        assert state["persistence_error"] is False, state
                        if state["tasks"][0]["status"] == "paused":
                            return state
                        if time.monotonic() > until:
                            raise TimeoutError(f"media stop did not finish: {state}")
                        time.sleep(0.05)

                fixture_base = f"http://127.0.0.1:{fixture.server_port}"
                media_path = root / "CLI 媒体.mp4"
                media_added = control("media-add", fixture_base + "/pending.m3u8?token=never-output", "--output", str(media_path))
                assert media_added["data"]["accepted"] is True
                media_id = media_added["data"]["id"]
                control("media-stop", str(media_id))
                media_paused(media_id)
                control("media-retry", str(media_id))
                control("media-stop", str(media_id))
                assert "never-output" not in json.dumps(media_paused(media_id))
                assert not media_path.exists()
                project = root / "CLI 站点项目"
                created = control("grabber-create", fixture_base + "/site.html?token=never-output", "--dir", str(project), "--depth", "1", "--pages", "10")
                ready = project_done(created["data"]["operation_id"])
                assert ready["operation"]["status"] == "succeeded" and ready["project"]["status"] == "ready", ready
                Fixture.slow_site.set()
                running = control("grabber-run")
                assert Fixture.site_started.wait(5)
                control("grabber-run", code=4)
                control("grabber-stop")
                cancelled = project_done(running["data"]["operation_id"])
                assert cancelled["operation"]["status"] == "cancelled", cancelled
                Fixture.slow_site.clear()
                opened = control("grabber-open", str(project))
                assert project_done(opened["data"]["operation_id"])["operation"]["status"] == "succeeded"
                running = control("grabber-run")
                done = project_done(running["data"]["operation_id"])
                assert done["project"]["status"] == "complete" and done["project"]["downloaded"] == 2, done
                assert "never-output" not in json.dumps(done) and "start_url" not in json.dumps(done)
                assert Path(done["project"]["entry_path"]).is_file()
                failed = control("grabber-open", str(root / "nonexistent project"))
                failed = project_done(failed["data"]["operation_id"])
                assert failed["operation"]["status"] == "failed" and failed["error"] is True, failed
                # A settings write failure must not be acknowledged as a successful add/save.
                with sqlite3.connect(database) as db:
                    db.execute("CREATE TRIGGER smoke_deny_settings BEFORE INSERT ON settings WHEN NEW.key IN ('media_history','grabber_config') BEGIN SELECT RAISE(FAIL, 'smoke settings failure'); END")
                rejected = control("media-add", fixture_base + "/pending.m3u8", "--output", str(root / "rejected.mp4"), code=4)
                assert rejected["ok"] is False
                control("media-retry", str(media_id), code=4)
                assert control("media-status", str(media_id))["data"]["tasks"][0]["status"] == "paused"
                assert len(control("media-status")["data"]["tasks"]) == 1
                unsaved = control("grabber-open", str(project))
                unsaved = project_done(unsaved["data"]["operation_id"])
                assert unsaved["operation"]["status"] == "failed" and unsaved["error"] is True, unsaved
                with sqlite3.connect(database) as db:
                    db.execute("DROP TRIGGER smoke_deny_settings")
                recovered = control("grabber-open", str(project))
                assert project_done(recovered["data"]["operation_id"])["operation"]["status"] == "succeeded"
                absent = root / "no running instance"
                result = subprocess.run([str(cli), "--data-dir", str(absent), "media-status"], capture_output=True, text=True, timeout=10)
                assert result.returncode == 3 and not absent.exists(), result.stdout
            if args.with_cli_actions:
                def task_state(task, status):
                    until = time.monotonic() + 15
                    while True:
                        state = control("status")["data"]
                        item = next(item for item in state["tasks"] if item["id"] == task)
                        if item["status"] == status:
                            return item
                        if item["status"] == "failed" or time.monotonic() > until:
                            raise TimeoutError(f"task state did not reach {status}: {item}")
                        time.sleep(0.05)

                assert control("conflict-policy")["data"]["policy"] == "ask"
                conflicts = root / "同名文件"
                conflicts.mkdir()
                for action in ("rename", "overwrite", "skip"):
                    old = conflicts / (action + ".bin")
                    old.write_bytes(b"original user bytes")
                    added = control("add", fixture_base + "/payload.bin", "--dir", str(conflicts), "--name", old.name)
                    duplicate = added["data"]["id"]
                    expected_tasks += 1
                    waiting = task_state(duplicate, "waiting_for_confirmation")
                    assert Path(waiting["path"]) == old and old.read_bytes() == b"original user bytes"
                    if action == "rename":
                        with sqlite3.connect(database) as db:
                            db.execute("CREATE TRIGGER smoke_deny_conflict BEFORE UPDATE ON tasks WHEN OLD.status='file_conflict' BEGIN SELECT RAISE(FAIL, 'smoke resolution failure'); END")
                            db.execute("CREATE TRIGGER smoke_deny_conflict_policy BEFORE INSERT ON settings WHEN NEW.key='file_conflict_policy' BEGIN SELECT RAISE(FAIL, 'smoke policy failure'); END")
                        control("resolve", str(duplicate), "--action", action, code=4)
                        control("conflict-policy", "overwrite", code=4)
                        assert control("conflict-policy")["data"]["policy"] == "ask"
                        task_state(duplicate, "waiting_for_confirmation")
                        assert old.read_bytes() == b"original user bytes"
                        with sqlite3.connect(database) as db:
                            db.execute("DROP TRIGGER smoke_deny_conflict")
                            db.execute("DROP TRIGGER smoke_deny_conflict_policy")
                    control("resolve", str(duplicate), "--action", action)
                    done = task_state(duplicate, "paused" if action == "skip" else "completed")
                    if action == "rename":
                        assert Path(done["path"]) != old and Path(done["path"]).read_bytes() == PAYLOAD
                        assert old.read_bytes() == b"original user bytes"
                    elif action == "overwrite":
                        assert Path(done["path"]) == old and old.read_bytes() == PAYLOAD
                    else:
                        assert old.read_bytes() == b"original user bytes"
                assert control("conflict-policy", "rename")["data"]["policy"] == "rename"
                with sqlite3.connect(database) as db:
                    assert db.execute("SELECT value FROM settings WHERE key='file_conflict_policy'").fetchone()[0] == "rename"
                assert control("conflict-policy", "ask")["data"]["policy"] == "ask"
            if args.with_cli_actions:
                for action in ("rename", "overwrite", "skip"):
                    old = root / ("媒体同名 " + action + ".mp4")
                    old.write_bytes(b"original media bytes")
                    added = control("media-add", fixture_base + "/pending.m3u8", "--output", str(old))
                    job = added["data"]["id"]
                    state = control("media-status", str(job))["data"]["tasks"][0]
                    assert state["status"] == "waiting_for_confirmation" and old.read_bytes() == b"original media bytes"
                    if action == "rename":
                        with sqlite3.connect(database) as db:
                            db.execute("CREATE TRIGGER smoke_deny_media_conflict BEFORE INSERT ON settings WHEN NEW.key='media_history' BEGIN SELECT RAISE(FAIL, 'smoke media resolution failure'); END")
                        control("media-resolve", str(job), "--action", action, code=4)
                        assert control("media-status", str(job))["data"]["tasks"][0]["status"] == "waiting_for_confirmation"
                        with sqlite3.connect(database) as db:
                            db.execute("DROP TRIGGER smoke_deny_media_conflict")
                    control("media-resolve", str(job), "--action", action)
                    if action != "skip":
                        control("media-stop", str(job))
                    final = media_paused(job)["tasks"][0]
                    assert old.read_bytes() == b"original media bytes"
                    if action == "rename":
                        assert Path(final["path"]) != old and not Path(final["path"]).exists()
                    if action == "overwrite":
                        control("media-retry", str(job))
                        assert control("media-status", str(job))["data"]["tasks"][0]["status"] == "waiting_for_confirmation"
                        control("media-resolve", str(job), "--action", "skip")
                control("conflict-policy", "rename")
                added = control("media-add", fixture_base + "/pending.m3u8", "--output", str(old))
                job = added["data"]["id"]
                state = control("media-status", str(job))["data"]["tasks"][0]
                assert state["status"] != "waiting_for_confirmation" and Path(state["path"]) != old
                control("media-stop", str(job))
                media_paused(job)
                assert old.read_bytes() == b"original media bytes"
                control("conflict-policy", "ask")
            # Media handoff is a queued UI draft, never an HTTP manifest task.
            accepted = request("/tasks", {"url": "https://media.example.test/master.m3u8", "media": "hls"})
            assert accepted["id"] is None and accepted["file_name"] == "master.mp4"
            with sqlite3.connect(database) as db:
                assert db.execute("SELECT count(*) FROM tasks").fetchone()[0] == expected_tasks
                assert db.execute("SELECT count(*) FROM queue_members WHERE task_id=?", (task_id,)).fetchone()[0] == 1
            report = {"result": "passed", "executable": str(executable), "isolated_data": str(root),
                      "bytes": len(PAYLOAD), "sha256": hashlib.sha256(PAYLOAD).hexdigest(),
                      "category_directory": str(path.parent), "media_handoff": True,
                      "cli_actions": args.with_cli_actions, "scanner_fixture": args.with_cli_actions,
                      "media_cli_control": args.with_cli_actions, "grabber_cli_control": args.with_cli_actions,
                      "cli_settings_failure": args.with_cli_actions, "file_conflict_cli": args.with_cli_actions,
                      "media_file_conflict_cli": args.with_cli_actions, "native_ui_verified": False}
            (root / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
            print(json.dumps(report, indent=2))
    finally:
        if process and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        fixture.shutdown()
        fixture.server_close()


if __name__ == "__main__":
    main()
