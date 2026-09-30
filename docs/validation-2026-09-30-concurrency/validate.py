import subprocess, os, time, pathlib, json, re
root=pathlib.Path('/tmp/codex-raknet-concurrency-3389irgc')
assert os.readlink('/proc/self/ns/net') != os.environ['TASK_HOST_NETNS']
assert os.geteuid()==0
os.environ['TOKIO_WORKER_THREADS']='4'
subprocess.run(['ip','link','set','lo','mtu','1500'],check=True)
def stop(p):
 p.terminate()
 try:p.wait(timeout=3)
 except subprocess.TimeoutExpired:p.kill();p.wait()
def stats(p):
 try:
  status=pathlib.Path(f'/proc/{p.pid}/status').read_text(); st=pathlib.Path(f'/proc/{p.pid}/stat').read_text().split()
  return {'cpu_s':(int(st[13])+int(st[14]))/os.sysconf('SC_CLK_TCK'),'peak_rss_kib':int(re.search(r'VmHWM:\s+(\d+)',status)[1])}
 except Exception:return {}
def udp_stats():
 lines=pathlib.Path('/proc/net/snmp').read_text().splitlines()
 lines=[line.split()[1:] for line in lines if line.startswith('Udp:')]
 return dict(zip(lines[0],map(int,lines[1])))
def stress(variant,count,messages,loss):
 subprocess.run(['tc','qdisc','replace','dev','lo','root','netem','limit','100000','loss',f'{loss}%'],check=True)
 target=root/('target' if variant=='work' else 'base-target')/'release'
 label=f'{variant}-{count}-{messages}-{loss}'
 with (root/f'logs/stress-{label}-server.log').open('w') as sl, (root/f'logs/stress-{label}-proxy.log').open('w') as pl:
  server=subprocess.Popen([str(target/'test_benchmark'),'--protocol','raknet','--type','server','--address','127.0.0.1:19200'],stdout=sl,stderr=sl)
  proxy=subprocess.Popen([str(target/'proxy'),'-l','127.0.0.1:19201','-r','127.0.0.1:19200'],stdout=pl,stderr=pl)
  try:
   time.sleep(.3)
   metrics=root/f'logs/stress-{label}-time.txt'
   before=udp_stats()
   result=subprocess.run(['/usr/bin/time','-f','%U %S %M','-o',str(metrics),str(root/'target/release/examples/concurrency'),'127.0.0.1:19201',str(count),str(messages)],capture_output=True,text=True,timeout=130)
   record={'kind':'proxy_concurrency','variant':variant,'connections':count,'messages_per_connection':messages,'loss_percent':loss,'rc':result.returncode,'stdout':result.stdout,'stderr':result.stderr,'server':stats(server),'proxy':stats(proxy),'client_usage':metrics.read_text(),'udp_delta':{k:v-before[k] for k,v in udp_stats().items()}}
   print(json.dumps(record),flush=True)
   return result.returncode==0
  finally:stop(proxy);stop(server)
if os.sys.argv[1:]==['stress']:
 results=[stress(*case) for case in [('base',512,30,0),('work',512,30,0),('work',1024,30,0),('work',128,30,1)]]
 raise SystemExit(0 if all(results) else 1)
if os.sys.argv[1:]==['stress-final']:
 results=[]
 for repeat in range(5):
  for variant in (['base','work'] if repeat%2==0 else ['work','base']):
   results.append(stress(variant,512,30,0))
 for case in [('work',1024,30,0),('work',128,30,1),('work',128,30,5)]:results.append(stress(*case))
 raise SystemExit(0 if all(results) else 1)
if os.sys.argv[1:]==['soak']:
 results=[stress('base',1024,300,0),stress('work',1024,300,0)]
 raise SystemExit(0 if all(results) else 1)
if os.sys.argv[1:]==['interop']:
 results=[]
 for addr in ['127.0.0.1:19210','[::1]:19210']:
  subprocess.run(['tc','qdisc','replace','dev','lo','root','netem','limit','100000','loss','0%'],check=True)
  for direction in ['go-server','rust-server']:
   args=[str(root/'go-peer'),'server',addr] if direction=='go-server' else [str(root/'target/release/test_benchmark'),'--protocol','raknet','--type','server','--address',addr]
   with (root/f'logs/interop-{addr.replace(":","_")}-{direction}.log').open('w') as log:
    server=subprocess.Popen(args,stdout=log,stderr=log)
    try:
     time.sleep(.3)
     args=[str(root/'go-peer'),'client',addr] if direction=='rust-server' else [str(root/'target/release/examples/interop'),addr]
     r=subprocess.run(args,capture_output=True,text=True,timeout=75)
     print(json.dumps({'kind':'interop','address':addr,'direction':direction,'rc':r.returncode,'stdout':r.stdout,'stderr':r.stderr}),flush=True);results.append(r.returncode==0)
    finally:stop(server)
 raise SystemExit(0 if all(results) else 1)
