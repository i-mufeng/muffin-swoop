#!/usr/bin/env python3
"""Isolated real-app FTP control/queue/conflict smoke; does not automate native UI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import tempfile
import time


def stop(process):
    if process is not None and process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executable", type=Path)
    parser.add_argument("--output-dir", type=Path, default=Path("target/iteration-validation"))
    parser.add_argument("--python", type=Path, default=Path(os.environ.get("SWOOP_FTP_TEST_PYTHON", "target/ftp-fixture-venv/bin/python")))
    args = parser.parse_args()
    executable = args.executable.resolve(strict=True)
    cli = executable.with_name("swoop-cli.exe" if os.name == "nt" else "swoop-cli")
    if not cli.is_file() and executable.parent.name == "MacOS":
        cli = executable.parent.parent.parent.parent / "swoop-cli"
    assert cli.is_file(), cli
    args.output_dir.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="ftp-desktop-", dir=args.output_dir.resolve()))
    fixture_dir = root / "fixture"
    fixture_dir.mkdir()
    data = root / "data"
    data.mkdir()
    downloads = root / "downloads"
    downloads.mkdir()
    database = data / "swoop.db"
    settings = {
        "default_dir": str(downloads), "clipboard_watch": "0", "auto_resume": "0",
        "bridge_enabled": "0", "notify_complete": "0", "notify_failure": "0",
        "network_policy": json.dumps({"use_system_proxy": True, "retries": 0}),
        # Proves normal CLI add uses the same FTP-specific defaults as the GUI.
        "ftp_defaults": json.dumps({"proxy": "Direct", "tls": "Plain"}),
    }
    with sqlite3.connect(database) as db:
        db.execute("CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL)")
        db.executemany("INSERT INTO settings VALUES (?,?)", settings.items())

    def control(*values, code=0):
        result = subprocess.run([str(cli), "--data-dir", str(data), *values], capture_output=True, text=True, timeout=10)
        assert result.returncode == code, (values, result.returncode, result.stdout, result.stderr)
        return json.loads(result.stdout)

    def await_status(task_id, wanted):
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline:
            tasks = control("status")["data"]["tasks"]
            task = next(task for task in tasks if task["id"] == task_id)
            if task["status"] == wanted:
                return task
            if task["status"] == "failed" and wanted != "failed":
                raise AssertionError(task)
            time.sleep(0.05)
        raise TimeoutError(task)

    app = fixture = None
    try:
        with (root / "ftp.log").open("w") as ftp_log, (root / "application.log").open("w") as app_log:
            server = Path(__file__).resolve().parent.parent / "crates/ftp/tests/server.py"
            assert args.python.is_file(), args.python
            fixture = subprocess.Popen([str(args.python.absolute()), str(server), str(fixture_dir), "plain"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=ftp_log, text=True)
            ready = json.loads(fixture.stdout.readline())
            base = f"ftp://127.0.0.1:{ready['port']}"
            app = subprocess.Popen([str(executable)], env={**os.environ, "SWOOP_DATA_DIR": str(data)}, stdout=app_log, stderr=app_log)
            deadline = time.monotonic() + 25
            while not (data / ".swoop-control/locator.json").is_file():
                assert app.poll() is None, "application exited; inspect application.log"
                if time.monotonic() >= deadline:
                    raise TimeoutError("control service did not start")
                time.sleep(0.05)
            control("queue-stop", "1")
            task_id = control("add", base + "/payload.bin", "--name", "FTP 文件.bin")["data"]["id"]
            await_status(task_id, "queued")
            control("queue-start", "1")
            await_status(task_id, "completed")
            payload = (fixture_dir / "payload.bin").read_bytes()
            assert (downloads / "FTP 文件.bin").read_bytes() == payload
            old = downloads / "FTP 同名.bin"
            old.write_bytes(b"original user file")
            task_id = control("add", base + "/payload.bin", "--name", old.name)["data"]["id"]
            await_status(task_id, "waiting_for_confirmation")
            assert old.read_bytes() == b"original user file"
            control("resolve", str(task_id), "--action", "overwrite")
            await_status(task_id, "completed")
            assert old.read_bytes() == payload
            # Full bytes followed by FTP 426 must fail and preserve the old target.
            old.write_bytes(b"second user file")
            task_id = control("add", base + "/final-error.bin", "--name", old.name)["data"]["id"]
            await_status(task_id, "waiting_for_confirmation")
            control("resolve", str(task_id), "--action", "overwrite")
            await_status(task_id, "failed")
            assert old.read_bytes() == b"second user file"
            with sqlite3.connect(database) as db:
                records = db.execute("SELECT ftp_options FROM tasks").fetchall()
                assert all(json.loads(row[0])["proxy"] == "Direct" for row in records)
            report = {"result": "passed", "executable": str(executable), "isolated_data": str(data),
                      "bytes": len(payload), "sha256": hashlib.sha256(payload).hexdigest(),
                      "ftp_defaults": True, "queue": True, "file_conflict": True,
                      "failed_final_response_preserves_original": True, "native_ui_verified": False}
            (root / "report.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
            print(json.dumps(report, indent=2))
    finally:
        stop(app)
        if fixture is not None and fixture.poll() is None:
            try:
                fixture.stdin.write("stop\n")
                fixture.stdin.flush()
                fixture.wait(timeout=5)
            except (BrokenPipeError, subprocess.TimeoutExpired):
                stop(fixture)


if __name__ == "__main__":
    main()
