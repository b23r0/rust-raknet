import contextlib, io, importlib.util, json, os, sys
from pathlib import Path
root=Path(os.environ['TASK_ROOT'])
spec=importlib.util.spec_from_file_location('compare',root/'work/example/test_benchmark/compare_revisions.py');m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
os.environ['RAKNET_PARENT_NETNS']=os.readlink('/proc/self/ns/net')
try:m.check_isolation()
except RuntimeError:print('PASS: direct worker invocation rejected before network changes')
else:raise AssertionError('worker guard failed')
m.check_isolation=lambda:None;m.command=lambda *args:''
for mode,expected in [([],54),(['--throughput-long'],54),(['--latency'],15),(['--latency-unpinned'],15),(['--wan-latency'],9)]:
 calls=[]
 def run_case(profile,repeat,variant,**options):
  calls.append((profile,repeat,variant,options));return {'returncode':0}
 m.run_case=run_case;sys.argv=['compare',*mode]
 with contextlib.redirect_stdout(io.StringIO()) as output:assert m.main()==0
 assert len(calls)==expected
 for profile in {c[0] for c in calls}:
  counts=[sum(c[0]==profile and c[2]==v for c in calls) for v in ['base','work','tcp']]
  assert counts[0]==counts[1]==counts[2]
 if mode==['--latency-unpinned']:assert all(c[3]['pin']==False and c[3]['samples']==10000 for c in calls)
 print('PASS:',mode or ['default'],expected,'synthetic cases; equal TCP repetitions')
