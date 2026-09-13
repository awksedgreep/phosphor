import http.server,json,sqlite3,pathlib,base64
root=pathlib.Path(pathlib.Path('/tmp/phosphor-review-location').read_text())
log=(root/'hrana-requests.jsonl').open('w')
def encode(v):
 if v is None:return {'type':'null'}
 if isinstance(v,int):return {'type':'integer','value':str(v)}
 if isinstance(v,float):return {'type':'float','value':v}
 if isinstance(v,bytes):return {'type':'blob','base64':base64.b64encode(v).decode()}
 return {'type':'text','value':v}
def decode(v):
 return {'null':lambda:None,'integer':lambda:int(v['value']),'float':lambda:v['value'],'text':lambda:v['value'],'blob':lambda:base64.b64decode(v['base64'])}[v['type']]()
class Handler(http.server.BaseHTTPRequestHandler):
 def log_message(self,*args):pass
 def do_POST(self):
  body=json.loads(self.rfile.read(int(self.headers['Content-Length'])))
  log.write(json.dumps(body)+'\n');log.flush()
  conn=sqlite3.connect(root/'hrana-fixture.db',isolation_level=None)
  conn.execute('PRAGMA foreign_keys=ON')
  results=[]
  for req in body['requests']:
   if req['type']=='close':
    conn.close();results.append({'type':'ok','response':{'type':'close'}});continue
   try:
    stmt=req['stmt'];cur=conn.execute(stmt['sql'],[decode(v) for v in stmt.get('args',[])])
    cols=[{'name':c[0]} for c in cur.description or []]
    rows=[[encode(v) for v in r] for r in cur.fetchall()]
    results.append({'type':'ok','response':{'type':'execute','result':{'cols':cols,'rows':rows,'affected_row_count':max(0,cur.rowcount),'last_insert_rowid':str(cur.lastrowid)}}})
   except sqlite3.Error as e:results.append({'type':'error','error':{'message':str(e)}})
  out=json.dumps({'results':results}).encode()
  self.send_response(200);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(out)));self.end_headers();self.wfile.write(out)
server=http.server.HTTPServer(('127.0.0.1',0),Handler)
pathlib.Path('/tmp/phosphor-review-hrana-url').write_text('http://127.0.0.1:'+str(server.server_port))
print('Local protocol fixture ready',flush=True)
server.serve_forever()
