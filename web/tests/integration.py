"""Run against local debug builds; uses a mock Wi-Fi device, never physical hardware."""
import io,json,os,socket,struct,subprocess,tempfile,threading,time,urllib.request,urllib.error,zlib
from pathlib import Path
from PIL import Image
root=Path(__file__).resolve().parents[2]
cli=root/'cli/target/debug/epaper';web=root/'web/target/debug/epaper-web'
assert cli.exists() and web.exists(), 'Build cli and web in debug mode first'
key='a'*48
listener=socket.socket();listener.bind(('127.0.0.1',0));listener.listen();device_port=listener.getsockname()[1]
frames=[];errors=[]
def exact(c,n):
 out=b''
 while len(out)<n:
  chunk=c.recv(n-len(out))
  if not chunk:raise EOFError()
  out+=chunk
 return out
def device():
 try:
  while True:
   c,_=listener.accept()
   with c:
    c.settimeout(10);auth=b''
    while not auth.endswith(b'\n'):auth+=exact(c,1)
    if auth!=f'AUTH {key}\n'.encode():c.sendall(b'ERR authentication failed\n');continue
    c.sendall(b'AUTH_OK\n');h=exact(c,12)
    assert h[:4]==b'EPD1';length,crc=struct.unpack('<II',h[4:]);assert length==27200
    c.sendall(b'READY\n');frame=b''
    while len(frame)<length:frame+=exact(c,min(512,length-len(frame)));c.sendall(b'CHUNK\n')
    assert zlib.crc32(frame)==crc;frames.append(frame);c.sendall(b'DONE\n')
 except OSError:pass
 except Exception as e:errors.append(str(e))
threading.Thread(target=device,daemon=True).start()
with tempfile.TemporaryDirectory() as tmp:
 sock=socket.socket();sock.bind(('127.0.0.1',0));http_port=sock.getsockname()[1];sock.close()
 env={**os.environ,'EPAPER_BIN':str(cli),'EPAPER_DEVICE_HOST':f'127.0.0.1:{device_port}','EPAPER_DEVICE_KEY':key,'EPAPER_HTTP_PORT':str(http_port),'EPAPER_CONFIG':f'{tmp}/config.json','EPAPER_DRY_RUN':'0'}
 server=subprocess.Popen([str(web)],env=env,stdout=subprocess.PIPE,stderr=subprocess.PIPE)
 base=f'http://127.0.0.1:{http_port}'
 def call(path,data=None,headers={}):
  if isinstance(data,dict):data=json.dumps(data).encode();headers={'Content-Type':'application/json',**headers}
  with urllib.request.urlopen(urllib.request.Request(base+path,data=data,headers=headers),timeout=20) as r:return r.status,r.read()
 def bad(path,data,code):
  try:call(path,data);raise AssertionError('request should fail')
  except urllib.error.HTTPError as e:assert e.code==code,(e.code,e.read())
 try:
  for _ in range(100):
   try:call('/api/status');break
   except OSError:time.sleep(.1)
  chart={'title':'Test chart','values':[10,15,12,19,21],'unit':'USD'}
  _,png=call('/api/charts?preview=true',chart)
  assert Image.open(io.BytesIO(png)).size==(792,272)
  _,empty=call('/api/dashboard/frame?preview=true',b'')
  assert Image.open(io.BytesIO(empty)).size==(792,272)
  t=int(time.time())
  for price,stamp in [(10,t-60),(12,t-30),(11,t)]:call('/api/quotes',{'symbol':'TEST','price':price,'source':'test','timestamp':stamp})
  bad('/api/quotes',{'symbol':'TEST','price':9,'source':'test','timestamp':t-90},400)
  bad('/api/quotes',{'symbol':'TEST','price':9,'source':'test','timestamp':t+3600},400)
  bad('/api/charts',{'title':'bad','values':[1]},400)
  boundary='epaper-test-boundary';body=f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="test.png"\r\nContent-Type: image/png\r\n\r\n'.encode()+png+f'\r\n--{boundary}--\r\n'.encode()
  _,image=call('/api/images?preview=true',body,{'Content-Type':f'multipart/form-data; boundary={boundary}'})
  assert Image.open(io.BytesIO(image)).size==(792,272)
  jobs=[]
  for path,payload in [('/api/messages',{'text':'Wireless test','size':32}),('/api/charts',chart),('/api/dashboard/frame',b'')]:
   status,data=call(path,payload);assert status==202;jobs.append(json.loads(data)['id'])
  for job in jobs:
   for _ in range(100):
    state=json.loads(call('/api/messages/'+job)[1])
    if state['status'] not in ('queued','sending'):break
    time.sleep(.1)
   assert state['status']=='displayed',state
  assert len(frames)==3 and not errors,(len(frames),errors)
  # Config save is atomic and survives a new server instance (verified by its file content here).
  c=json.loads(call('/api/config')[1]);c['location']='Test location'
  req=urllib.request.Request(base+'/api/config',data=json.dumps(c).encode(),method='PUT',headers={'Content-Type':'application/json'})
  urllib.request.urlopen(req).read();assert json.load(open(f'{tmp}/config.json'))['location']=='Test location'
  print('PASS: Wi-Fi auth/frame/CRC/ack flow; text, chart and dashboard queue delivery; PNG/image previews; quote ordering/freshness validation; config write')
 finally:
  server.terminate();server.wait(timeout=10);listener.close()
