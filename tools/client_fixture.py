"""Run a private Fabric client fixture and summarise its failures without leaking its log."""

import os
import platform
import re
import signal
import subprocess

try:
    from . import build_pack
except ImportError:
    import build_pack


class FixtureTimeout(RuntimeError):
    """Keep timeout output private while allowing selected caller diagnostics."""
    def __init__(self, run, private_log):
        super().__init__(f"Private fixture timed out; inspect {private_log}")
        self.run = run


def run_fixture(command, instance, environment):
    # Keep all children of this private fixture in a controllable process group.
    process = subprocess.Popen(command, cwd=build_pack.ROOT, env=environment, text=True,
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                               start_new_session=platform.system() != "Windows")
    def stop_tree(force=False):
        if platform.system() == "Windows":
            subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], capture_output=True, check=False)
        else:
            try: os.killpg(process.pid, signal.SIGKILL if force else signal.SIGTERM)
            except ProcessLookupError: pass
    try:
        output, _ = process.communicate(timeout=240)
    except subprocess.TimeoutExpired:
        stop_tree()
        try: output, _ = process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            stop_tree(True); output, _ = process.communicate(timeout=10)
        (instance / "fixture.log").write_text(output)
        raise FixtureTimeout(subprocess.CompletedProcess(command, process.returncode, output),
                             instance / 'fixture.log') from None
    (instance / "fixture.log").write_text(output)
    return subprocess.CompletedProcess(command, process.returncode, output)


def failure_summary(run, report_count):
    """Publish only exit/count and bounded Java type names, never log messages."""
    types = []
    pattern = r"(?:Caused by:|Exception in thread \"[^\"]*\")\s+([A-Za-z_$][\w$]*(?:\.[A-Za-z_$][\w$]*)+)(?=[:\s]|$)"
    for kind in re.findall(pattern, run.stdout, flags=re.ASCII):
        if len(kind) <= 200 and kind not in types:
            types.append(kind)
        if len(types) == 8:
            break
    return {"exit_code": run.returncode, "result_count": report_count, "java_exception_types": types}

