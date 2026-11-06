import urllib.request, urllib.error,json,time
base='http://127.0.0.1:8787'
def call(path,payload=None,headers={}):
 data=json.dumps(payload).encode() if payload is not None else None
 r=urllib.request.urlopen(urllib.request.Request(base+path,data=data,headers={'Content-Type':'application/json',**headers}))
 return r.status,r.read()
assert json.loads(call('/api/status')[1])['mode']=='simulation'
status,png=call('/api/preview',{'text':'Hello, Jake\nAPI preview','size':32});assert status==200 and png.startswith(b'\x89PNG')
open('/tmp/web-preview.png','wb').write(png)
status,data=call('/api/messages',{'text':'API message','size':40});assert status==202
job=json.loads(data)
for _ in range(40):
 j=json.loads(call('/api/messages/'+job['id'])[1])
 if j['status'] not in ('queued','sending'):break
 time.sleep(.1)
assert j['status']=='simulated',j
for payload in [{'text':' '},{'text':'a','size':1},{'text':'a\x00'}]:
 try:call('/api/messages',payload);raise AssertionError('Expected rejection')
 except urllib.error.HTTPError as e:assert e.code==400
try:call('/api/messages',{'text':'cross origin'}, {'Origin':'http://example.com'});raise AssertionError('Expected origin rejection')
except urllib.error.HTTPError as e:assert e.code==403
print('PASS: status, PNG preview, queued message completion, invalid input and origin rejection')
