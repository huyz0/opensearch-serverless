from http.server import *
class H(BaseHTTPRequestHandler):
    def do(s):
        n=int(s.headers.get('content-length') or 0); s.rfile.read(n)
        if s.path.startswith('/redir'): s.send_response(302); s.send_header('Location','/ok'); s.end_headers(); return
        if s.path.startswith('/json'): b=b'{"out":[1,2,3],"echo":"ok"}'; s.send_response(200); s.send_header('content-type','application/json'); s.end_headers(); s.wfile.write(b); return
        code=int(s.path.split('/')[1]) if s.path.split('/')[1].isdigit() else 200
        s.send_response(code); s.send_header('content-type','text/plain'); s.end_headers(); s.wfile.write(b'hook says hi')
    do_POST=do_GET=do_PUT=do
    def log_message(s,*a): pass
HTTPServer(('127.0.0.1',18080),H).serve_forever()
