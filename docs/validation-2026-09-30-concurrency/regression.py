import pathlib, os, json, importlib.util, sys
root=pathlib.Path('/tmp/codex-raknet-concurrency-3389irgc')
spec=importlib.util.spec_from_file_location('compare',root/'compare.py');c=importlib.util.module_from_spec(spec);spec.loader.exec_module(c)
assert os.readlink('/proc/self/ns/net')!=os.environ['TASK_HOST_NETNS']
os.environ['TOKIO_WORKER_THREADS']='4'
c.command('ip','link','set','lo','mtu','1500','gso_max_size','1500','gso_max_segs','1','gro_max_size','1500')
mode=sys.argv[1]
if mode=='regression':
 profiles=[('fragment-long',0,0,4096,50000)];repeats=10;options={'samples':300}
elif mode=='final-throughput':
 profiles=[('clean-long',0,0,800,200000),('small-long',0,0,64,300000),('fragment-long',0,0,4096,50000),('loss1',1,0,800,20000),('loss5',5,0,800,10000),('wan',1,5,800,3000)];repeats=3;options={'samples':300}
elif mode=='throughput':
 profiles=[('clean',0,0,800,20000),('small',0,0,64,30000),('fragment',0,0,4096,5000),('loss1',1,0,800,20000),('loss5',5,0,800,20000),('wan',1,5,800,3000)];repeats=5;options={'samples':300}
elif mode in ['latency-pinned','latency-unpinned']:
 profiles=[(mode,0,0,800,1000)];repeats=5;options={'warmup':1000,'samples':10000,'pin':mode=='latency-pinned'}
elif mode=='wan':
 profiles=[('wan',1,5,800,1000)];repeats=3;options={'warmup':100,'samples':2000,'pin':True}
else:raise ValueError(mode)
failed=False
for profile in profiles:
 for repeat in range(repeats):
  variants=['base','work','tcp'] if repeat%2==0 else ['tcp','work','base']
  for variant in variants:
   record=c.run_case(profile,repeat,variant,**options)
   failed |= record.get('returncode',1)!=0 or 'error' in record
   print(json.dumps(record),flush=True)
sys.exit(int(failed))
