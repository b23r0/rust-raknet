"""Run prebuilt echo comparisons inside a task-local private network namespace."""
import os, pathlib, subprocess, time, json, re, signal, sys, resource
from datetime import datetime, timezone
r = pathlib.Path(os.environ['TASK_ROOT'])
assert pathlib.Path.cwd() == r / 'work'
mapping = pathlib.Path('/proc/self/uid_map').read_text().split()
assert os.readlink('/proc/self/ns/net') != os.environ['TASK_HOST_NETNS'] and os.geteuid() == 0 and (mapping[2] == '1')
assert os.environ['CARGO_HOME'] == str(r / 'cargo') and os.environ['HOME'] == str(r / 'home')
resource.setrlimit(resource.RLIMIT_NOFILE, (min(16384, resource.getrlimit(resource.RLIMIT_NOFILE)[1]), resource.getrlimit(resource.RLIMIT_NOFILE)[1]))
e = os.environ.copy()
e.update(TOKIO_WORKER_THREADS='4', GOMAXPROCS='4')
for k in list(e):
    if k.lower() in ['http_proxy', 'https_proxy', 'all_proxy', 'ftp_proxy', 'no_proxy']:
        e.pop(k)

def cmd(*a):
    return subprocess.check_output(a, text=True, stderr=subprocess.STDOUT)
cmd('ip', 'link', 'set', 'dev', 'lo', 'mtu', '1500', 'gso_max_size', '1500', 'gso_max_segs', '1', 'gro_max_size', '1500')
cpus = sorted(os.sched_getaffinity(0))
assert len(cpus) >= 8
physical = {}
for c in cpus:
    p = pathlib.Path(f'/sys/devices/system/cpu/cpu{c}/topology')
    physical.setdefault(((p / 'physical_package_id').read_text(), (p / 'core_id').read_text()), c)
cores = sorted(physical.values())
cores = cores if len(cores) >= 8 else cpus
sc = ','.join(map(str, cores[:4]))
cc = ','.join(map(str, cores[-4:]))
print(json.dumps({'metadata': {'date': datetime.now(timezone.utc).isoformat(), 'server_cpus': sc, 'client_cpus': cc, 'netns': os.readlink('/proc/self/ns/net'), 'kernel': os.uname().release, 'cpu': cmd('lscpu'), 'kcp_revision': (r / 'kcp/revision.txt').read_text(), 'rust': cmd('rustc', '--version').strip(), 'quic_go': 'v0.63.0', 'go': cmd(str(r / 'go/bin/go'), 'version').strip(), 'raknet': re.search(r'^version\s*=\s*"([^"]+)"', (r / 'work/Cargo.toml').read_text(), re.M)[1], 'source_revision': os.environ.get('RAKNET_REVISION', 'task copy; record its revision separately'), 'memory_method': 'Linux wait4 ru_maxrss, whole-process peak, client/server separately', 'loaded_rtt': os.environ.get('LOADED_RTT') == '1', 'explicit_batch': True, 'batch_policy': 'max 8 frames; permanent individual sends after actual NACK or reliable timeout', 'receive_batching': False, 'idle_maintenance': False}}), flush=True)

def counters():
    lines = pathlib.Path('/proc/net/snmp').read_text().splitlines()
    for i, a in enumerate(lines):
        if a.startswith('Udp:'):
            return dict(zip(a.split()[1:], map(int, lines[i + 1].split()[1:])))

def run(mode, profile, variant, repeat):
    size, count, loss, delay, n = profile
    warmup, samples = ((1000, 30000) if os.environ.get('LONG_LATENCY') == '1' else (1000, 10000)) if mode == 'latency' else (100, 300)
    subprocess.run(['tc', 'qdisc', 'del', 'dev', 'lo', 'root'], capture_output=True)
    cmd('tc', 'qdisc', 'add', 'dev', 'lo', 'root', 'netem', 'limit', '100000', *(['delay', str(delay) + 'ms'] if delay else []), 'loss', str(loss) + '%')
    before = counters()
    addr = '127.0.0.1:19342'
    b = r / 'target/release'
    row = dict(mode=mode, payload=size, messages=count, loss=loss, delay_ms=delay, connections=n, variant=variant, repeat=repeat)
    if mode == 'concurrent':
        common = [addr, str(n), str(count), str(size)]
        if variant in ['kcp', 'quic-go']:
            binary = r / 'bin' / ('kcp-concurrent' if variant == 'kcp' else 'quic-concurrent')
            sa = [str(binary), 'server', *common] if variant == 'kcp' else [str(binary), 'server', addr]
            ca = [str(binary), 'client', *common]
        elif variant == 'tcp':
            sa = [str(b / 'examples/concurrency_benchmark'), '--tcp-server', addr]
            ca = [str(b / 'examples/concurrency_benchmark'), '--tcp', *common]
        else:
            server_binary = b / 'examples/sharded_echo'
            client_binary = b / 'examples/concurrency_benchmark'
            sa = [str(server_binary), addr, '4', *(['--batch-messages'] if variant == 'batch' else [])]
            ca = [str(client_binary), *(['--batch'] if variant == 'batch' else []), *common]
        if os.environ.get('LOADED_RTT') == '1':
            ca += ['--loaded-rtt']
        sp = ['taskset', '-c', sc]
        cp = ['taskset', '-c', cc]
        expected = n * (count + 20)
    else:
        if variant == 'quic-go':
            sa = [str(r / 'bin/quic-single'), 'server', addr]
            ca = [str(r / 'bin/quic-single'), 'single', addr, str(count), str(size), str(warmup), str(samples)]
        else:
            binary = r / 'bin/kcp-single' if variant == 'kcp' else b / 'test_benchmark'
            protocol = 'raknet' if variant in ['base', 'default', 'batch'] else variant
            common = [str(binary), '--protocol', protocol, '--address', addr, '--payload-size', str(size)]
            if variant in ['base', 'default', 'batch']:
                common += ['--raknet-mtu', '1428']
            if variant == 'batch':
                common += ['--raknet-batch-size', '16']
            sa = common + ['--type', 'server']
            ca = common + ['--type', 'client', '--packets', str(count), '--warmup', str(warmup), '--latency-samples', str(samples)]
        sp = ['taskset', '-c', str(cores[0])] if mode == 'latency' else []
        cp = ['taskset', '-c', str(cores[-1])] if mode == 'latency' else []
        expected = count + warmup + samples
    run_kind = "smoke" if os.environ.get("SMOKE") == "1" else "measured"
    tag = f"{run_kind}-{mode}-{os.environ.get('LOADED_RTT', '0')}-{variant}-{loss}-{size}-{n}-{repeat}"
    logfile = r / 'logs' / (tag + '.log')
    wrapper = str(r / 'bin/measure-process')
    usage = {side: r / 'logs' / (tag + '-' + side + '.json') for side in ['client', 'server']}
    for path in usage.values():
        path.unlink(missing_ok=True)
    row['loaded_rtt'] = os.environ.get('LOADED_RTT') == '1'
    with logfile.open('w+') as log:
        server = subprocess.Popen([wrapper, str(usage['server']), '--', 'nice', '-n', '10', *sp, *sa], env=e, stdout=log, stderr=log)
        client = None
        try:
            time.sleep(0.3)
            assert server.poll() is None, 'server exited'
            client = subprocess.Popen([wrapper, str(usage['client']), '--', 'nice', '-n', '10', *cp, *ca], env=e, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, start_new_session=True)
            try:
                out, err = client.communicate(timeout=450)
            except subprocess.TimeoutExpired:
                os.killpg(client.pid, signal.SIGKILL)
                out, err = client.communicate()
                row['error'] = '450 s timeout'
            row.update(returncode=client.returncode, stdout=out, stderr=err)
            assert client.returncode == 0, f'client exited {client.returncode}'
            for key, pat in [('mib_s', 'per direction\\): ([\\d.]+)'), ('elapsed_s', 'Elapsed: ([\\d.]+)'), ('p50_us', 'RTT p50: ([\\d.]+)'), ('p95_us', 'RTT p95: ([\\d.]+)'), ('p99_us', 'RTT p99: ([\\d.]+)')]:
                row[key] = float(re.search(pat, out)[1])
            if mode == 'concurrent' or variant == 'quic-go':
                assert int(re.search('Verified ordered echoes: (\\d+)', out)[1]) == expected
            else:
                assert int(re.search('Packets: (\\d+)', out)[1]) == count and int(re.search('RTT samples: (\\d+)', out)[1]) == samples
            if mode == 'concurrent':
                assert int(re.search('RTT samples: (\\d+)', out)[1]) == n * (count if row['loaded_rtt'] else 20)
            row['verified_echoes'] = expected
        except Exception as ex:
            row['error'] = str(ex)
        finally:
            if client and client.poll() is None:
                os.killpg(client.pid, signal.SIGKILL)
                client.communicate()
            server.terminate()
            try:
                server.wait(timeout=5)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
            log.seek(0)
            row['server_log'] = log.read()[-2000:]
    for side, path in usage.items():
        try:
            u = json.loads(path.read_text())
            assert u['max_rss_kib'] > 0
            row[side + '_peak_rss_mib'] = u['max_rss_kib'] / 1024
            row[side + '_usage'] = u
        except Exception as ex:
            row['error'] = str(ex)
    row['qdisc'] = cmd('tc', '-s', 'qdisc', 'show', 'dev', 'lo')
    after = counters()
    row['udp_delta'] = {k: after[k] - before[k] for k in before}
    return row
mode = sys.argv[1]
if mode == 'single':
    profiles = [(800, 200000, 0, 0, 1), (800, 50000, 1, 0, 1), (800, 50000, 5, 0, 1), (64, 300000, 0, 0, 1), (64, 100000, 1, 0, 1), (64, 100000, 5, 0, 1), (4096, 50000, 0, 0, 1), (800, 3000, 1, 5, 1)]
elif mode == 'latency':
    profiles = [(800, 1000, 0, 0, 1)]
elif mode == 'concurrent' and os.environ.get('LOADED_RTT') == '1':
    profiles = [(size, 262144 // n, loss, 0, n) for size in [64, 800] for loss in [0, 1] for n in [64, 1024]]
elif mode == 'concurrent':
    profiles = [(800, 1048576 // n, loss, 0, n) for loss in [0, 1] for n in [64, 256, 1024, 2048]] + [(64, 1048576 // n, loss, 0, n) for loss in [0, 1] for n in [64, 1024]]
else:
    raise SystemExit('mode must be single, concurrent or latency')
variants = ['tcp', 'default', 'batch', 'kcp', 'quic-go']
if os.environ.get('SMOKE') == '1':
    profiles = [(64, 64, 1, 0, 64)] if mode == 'concurrent' else [(800, 1000, 0, 0, 1)]
for index, profile in enumerate(profiles):
    for repeat in range(1 if os.environ.get('SMOKE') == '1' else 5 if mode == 'latency' else 3):
        offset = (index + repeat) % len(variants)
        order = variants[offset:] + variants[:offset]
        for variant in order:
            row = run(mode, profile, variant, repeat)
            print(json.dumps(row), flush=True)
            if 'error' in row:
                sys.exit(1)
