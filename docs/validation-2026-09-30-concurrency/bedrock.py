import os, subprocess, pathlib, time, json, socket, re
root=pathlib.Path('/tmp/codex-raknet-concurrency-3389irgc');assert os.readlink('/proc/self/ns/net')!=os.environ['TASK_HOST_NETNS']
subprocess.run(['tc','qdisc','del','dev','lo','root'],capture_output=True)
os.environ['TOKIO_WORKER_THREADS']='4'
props=root/'bedrock/server.properties'; original=props.read_text()
def stop(p):
 p.terminate()
 try:p.wait(timeout=5)
 except subprocess.TimeoutExpired:p.kill();p.wait()
for transport in ['raknet','nethernet']:
 props.write_text(re.sub(r'^transport=.*$',f'transport={transport}',original,flags=re.M))
 logpath=root/f'logs/bedrock-{transport}.log'
 with logpath.open('w') as log:
  env=os.environ.copy();env['LD_LIBRARY_PATH']=str(root/'bedrock')
  server=subprocess.Popen([str(root/'bedrock/bedrock_server')],cwd=root/'bedrock',env=env,stdin=subprocess.PIPE,stdout=log,stderr=log,text=True)
  proxy=None
  try:
   for _ in range(120):
    if 'IPv4 supported, port:' in logpath.read_text() or 'Server started.' in logpath.read_text():break
    if server.poll() is not None:raise RuntimeError(logpath.read_text())
    time.sleep(.25)
   else:raise RuntimeError('Bedrock startup timed out')
   if transport=='raknet':
    proxy=subprocess.Popen([str(root/'target/release/proxy'),'-l','127.0.0.1:19144','-r','127.0.0.1:19142'],stdout=log,stderr=log)
    time.sleep(.3)
    for target in ['127.0.0.1:19142','127.0.0.1:19144']:
     r=subprocess.run([str(root/'target/release/examples/bedrock_probe'),target,'11','2193'],capture_output=True,text=True,timeout=20)
     print(json.dumps({'transport':transport,'target':target,'rc':r.returncode,'stdout':r.stdout,'stderr':r.stderr}),flush=True)
     if r.returncode:raise RuntimeError('Bedrock application protocol failed')
   else:
    proxy=subprocess.Popen([str(root/'target/release/nethernet-signaling-proxy'),'--listen','127.0.0.1:19144','--upstream','127.0.0.1:19142'],stdout=log,stderr=log)
    time.sleep(.3)
    responses=[]
    for port in [19142,19144]:
     with socket.create_connection(('127.0.0.1',port),timeout=5) as s:
      s.sendall(b'GET /v1/join HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n')
      response=b''
      while True:
       data=s.recv(65536)
       if not data:break
       response+=data
       if b'\r\n\r\n' in response:
        header,body=response.split(b'\r\n\r\n',1)
        length=next((int(line.split(b':',1)[1]) for line in header.split(b'\r\n') if line.lower().startswith(b'content-length:')),0)
        if len(body)>=length:break
     responses.append(response)
     print(json.dumps({'transport':transport,'port':port,'response':response.decode(errors='replace')}),flush=True)
    assert responses[0].startswith(b'HTTP/1.1') and responses[0]==responses[1],responses
  finally:
   if proxy:stop(proxy)
   if server.poll() is None:
    server.stdin.write('stop\n');server.stdin.flush()
    try:server.wait(timeout=10)
    except subprocess.TimeoutExpired:stop(server)
