"""Compare prebuilt async/blocking drivers in a task-local Linux network namespace."""

import json
import os
from pathlib import Path
import re
import resource
import signal
import subprocess
import sys
import time

root = Path(os.environ["TASK_ROOT"]).resolve()
parent = os.environ["RAKNET_PARENT_NETNS"]
mapping = Path("/proc/self/uid_map").read_text().split()
if (os.readlink("/proc/self/ns/net") == parent or os.geteuid() != 0
        or len(mapping) != 3 or mapping[2] != "1"
        or Path(os.environ["HOME"]).resolve() != root / "home"):
    raise RuntimeError("Use a task-local HOME and private user/network namespace")
resource.setrlimit(resource.RLIMIT_NOFILE, (8192, resource.getrlimit(resource.RLIMIT_NOFILE)[1]))
binaries = root / "target/release/examples"
wrapper = root / "measure-process"
logs = root / "logs"
logs.mkdir(exist_ok=True)
cpus = sorted(os.sched_getaffinity(0))
if len(cpus) < 8:
    raise RuntimeError("Eight CPUs are required for the published affinity layout")
server_cpus = ",".join(map(str, cpus[:4]))
client_cpus = ",".join(map(str, cpus[4:8]))
subprocess.run(["ip", "link", "set", "lo", "up"], check=True)
subprocess.run(["ip", "link", "set", "lo", "mtu", "1500", "gso_max_size", "1500", "gso_max_segs", "1", "gro_max_size", "1500"], check=True)
profiles = [
    (1, 64, 0, 0, 300000), (1, 800, 0, 0, 200000),
    (1, 4096, 0, 0, 50000), (1, 800, 1, 0, 50000),
    (1, 800, 5, 0, 20000), (1, 800, 1, 5, 3000),
    (64, 64, 0, 0, 30000), (64, 800, 0, 0, 30000),
    (64, 800, 1, 0, 10000), (128, 800, 0, 0, 30000),
    (512, 800, 0, 0, 10000),
]
if "--smoke" in sys.argv:
    profiles = [(1, 64, 0, 0, 1000), (4, 800, 1, 0, 1000)]
for profile_index, (count, size, loss, delay, messages) in enumerate(profiles):
    variants = [(api, batch) for api in (["async", "blocking"] if count <= 64 else ["async"]) for batch in [False, True]]
    for repeat in range(1 if "--smoke" in sys.argv else 3):
        offset = (profile_index + repeat) % len(variants)
        for api, batch in variants[offset:] + variants[:offset]:
            subprocess.run(["tc", "qdisc", "del", "dev", "lo", "root"], capture_output=True)
            netem = ["tc", "qdisc", "add", "dev", "lo", "root", "netem", "limit", "100000", "loss", f"{loss}%"]
            if delay:
                netem += ["delay", f"{delay}ms"]
            subprocess.run(netem, check=True)
            tag = f"api-{count}-{size}-{loss}-{delay}-{api}-{batch}-{repeat}"
            usage = {side: logs / f"{tag}-{side}.json" for side in ["client", "server"]}
            address = "127.0.0.1:19342"
            server_args = [str(binaries / "sharded_echo"), address, "4"] + (["--batch-messages"] if batch else [])
            client_args = [str(binaries / ("blocking_benchmark" if api == "blocking" else "concurrency_benchmark"))] + (["--batch"] if batch else []) + [address, str(count), str(messages), str(size), "--loaded-rtt"]
            row = dict(api=api, batch=batch, connections=count, payload=size, loss=loss, delay_ms=delay, messages=messages, repeat=repeat)
            env = os.environ.copy()
            env["TOKIO_WORKER_THREADS"] = "4"
            with (logs / f"{tag}-server.log").open("w+") as log:
                server = subprocess.Popen([str(wrapper), str(usage["server"]), "--", "nice", "-n", "10", "taskset", "-c", server_cpus, *server_args], stdout=log, stderr=log, env=env)
                client = None
                try:
                    time.sleep(0.3)
                    if server.poll() is not None:
                        raise RuntimeError("server exited before client start")
                    client = subprocess.Popen([str(wrapper), str(usage["client"]), "--", "nice", "-n", "10", "taskset", "-c", client_cpus, *client_args], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env, start_new_session=True)
                    stdout, stderr = client.communicate(timeout=300)
                    row.update(returncode=client.returncode, stdout=stdout, stderr=stderr)
                    if client.returncode != 0:
                        raise RuntimeError("client failed")
                    for key, pattern in [("mib_s", r"per direction\): ([\d.]+)"), ("p50_us", r"RTT p50: ([\d.]+)"), ("p99_us", r"RTT p99: ([\d.]+)")]:
                        row[key] = float(re.search(pattern, stdout)[1])
                    echoes = int(re.search(r"Verified ordered echoes: (\d+)", stdout)[1])
                    samples = int(re.search(r"RTT samples: (\d+)", stdout)[1])
                    if echoes != count * (messages + 20) or samples != count * messages:
                        raise RuntimeError("echo/sample count mismatch")
                    row["verified_echoes"] = echoes
                except Exception as error:
                    row["error"] = str(error)
                finally:
                    if client is not None and client.poll() is None:
                        os.killpg(client.pid, signal.SIGKILL)
                        client.communicate()
                    server.terminate()
                    try:
                        server.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait()
                    log.seek(0)
                    row["server_log"] = log.read()[-2000:]
            for side, path in usage.items():
                try:
                    data = json.loads(path.read_text())
                    if data["max_rss_kib"] <= 0:
                        raise RuntimeError("invalid RSS")
                    row[f"{side}_rss_mib"] = data["max_rss_kib"] / 1024
                except Exception as error:
                    row["error"] = str(error)
            row["qdisc"] = subprocess.check_output(["tc", "-s", "qdisc", "show", "dev", "lo"], text=True)
            print(json.dumps(row), flush=True)
            if "error" in row:
                sys.exit(1)
