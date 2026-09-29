"""Compare prebuilt revisions inside compare_revisions.sh's disposable namespace."""

import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parent
PROFILES = [
    ("clean", 0, 0, 800, 20000),
    ("loss1", 1, 0, 800, 20000),
    ("loss5", 5, 0, 800, 20000),
    ("small", 0, 0, 64, 30000),
    ("fragment", 0, 0, 4096, 5000),
    ("wan", 1, 5, 800, 3000),
]


def command(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def check_isolation():
    parent = os.environ.get("RAKNET_PARENT_NETNS")
    mapping = Path("/proc/self/uid_map").read_text().split()
    # A host user/root process cannot bypass this check by setting an environment
    # variable. A mapped user namespace also prevents administration of host netns.
    if (not parent or os.readlink("/proc/self/ns/net") == parent
            or os.geteuid() != 0 or len(mapping) != 3 or mapping[2] != "1"):
        raise RuntimeError("Run through compare_revisions.sh in its private user/network namespace")


def run_case(profile, repeat, variant, *, warmup=100, samples=300, pin=False):
    name, loss, delay, size, packets = profile
    subprocess.run(["tc", "qdisc", "del", "dev", "lo", "root"], capture_output=True)
    netem = ["tc", "qdisc", "add", "dev", "lo", "root", "netem",
             "limit", "100000", "loss", f"{loss}%"]
    if delay:
        netem += ["delay", f"{delay}ms"]
    command(*netem)
    binary = ROOT / ("work" if variant == "work" else "base")
    protocol = "tcp" if variant == "tcp" else "raknet"
    common = [str(binary), "--protocol", protocol, "--address", "127.0.0.1:19132"]
    record = dict(profile=name, variant=variant, repeat=repeat,
                  warmup=warmup, latency_samples=samples)
    cpus = sorted(os.sched_getaffinity(0))
    server_prefix = ["taskset", "-c", str(cpus[0])] if pin else []
    client_prefix = ["taskset", "-c", str(cpus[-1])] if pin else []
    if pin:
        if len(cpus) < 2:
            raise RuntimeError("The focused comparison requires two available CPUs")
        record["cpu_affinity"] = {"server": cpus[0], "client": cpus[-1]}
    metrics = ROOT / "client-time.txt"
    metrics.unlink(missing_ok=True)
    with (ROOT / "server.log").open("w+") as server_log:
        server = subprocess.Popen([*server_prefix, "nice", "-n", "10", *common, "--type", "server"],
                                  stdout=server_log, stderr=server_log)
        client = None
        try:
            time.sleep(0.2)
            started = time.monotonic()
            client = subprocess.Popen(
                ["/usr/bin/time", "-f", "%U %S %M", "-o", str(metrics),
                 *client_prefix, "nice", "-n", "10", *common, "--type", "client", "--packets", str(packets),
                 "--payload-size", str(size), "--warmup", str(warmup), "--latency-samples", str(samples)],
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
            try:
                stdout, stderr = client.communicate(timeout=150)
            except subprocess.TimeoutExpired:
                os.killpg(client.pid, signal.SIGKILL)
                stdout, stderr = client.communicate()
                record["error"] = "Client exceeded 150 seconds"
            record.update(returncode=client.returncode, stdout=stdout, stderr=stderr,
                          wall=time.monotonic() - started)
            if client.returncode == 0:
                record["mib_s"] = float(re.search(r"per direction\): ([\d.]+)", stdout)[1])
                for percentile in ("p50", "p95", "p99"):
                    record[percentile] = float(re.search(r"RTT " + percentile + r": ([\d.]+)", stdout)[1])
            if metrics.exists():
                record["usage"] = metrics.read_text()
            stat = Path(f"/proc/{server.pid}/stat").read_text().split()
            record["server_cpu_s"] = (int(stat[13]) + int(stat[14])) / os.sysconf("SC_CLK_TCK")
            status = Path(f"/proc/{server.pid}/status").read_text()
            record["server_rss_kib"] = int(re.search(r"VmHWM:\s+(\d+)", status)[1])
        except Exception as error:
            record["error"] = str(error)
        finally:
            if client is not None and client.poll() is None:
                os.killpg(client.pid, signal.SIGKILL)
                client.communicate()
            server.terminate()
            try:
                server.wait(timeout=3)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
            server_log.seek(0)
            record["server_log"] = server_log.read()[-4096:]
            record["qdisc"] = command("tc", "-s", "qdisc", "show", "dev", "lo")
    return record


def main():
    check_isolation()
    command("ip", "link", "set", "lo", "up")
    command("ip", "link", "set", "dev", "lo", "mtu", "1500",
            "gso_max_size", "1500", "gso_max_segs", "1", "gro_max_size", "1500")
    profiles, repeats, options = PROFILES, 3, {}
    if sys.argv[1:] == ["--latency"]:
        profiles, repeats = [("latency_clean", 0, 0, 800, 1000)], 5
        options = dict(warmup=1000, samples=10000, pin=True)
    elif sys.argv[1:] == ["--wan-latency"]:
        profiles = [("wan_latency", 1, 5, 800, 1000)]
        options = dict(warmup=100, samples=2000, pin=True)
    elif sys.argv[1:]:
        raise ValueError("Unknown comparison mode")
    failed = False
    for profile in profiles:
        for repeat in range(repeats):
            variants = ["base", "work"] if repeat % 2 == 0 else ["work", "base"]
            if repeat == 0:
                variants.append("tcp")
            for variant in variants:
                record = run_case(profile, repeat, variant, **options)
                failed |= record.get("returncode", 1) != 0 or "error" in record
                print(json.dumps(record), flush=True)
    return int(failed)


if __name__ == "__main__":
    sys.exit(main())
