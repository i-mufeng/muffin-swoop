"""Local challenge verifier: no external account, tickets or credentials are used."""
import base64
import hashlib
import hmac
import http.server
import json
import re
import struct
import sys
import threading
import urllib.parse

DATA = bytes((index % 251 for index in range(1048595)))
CHALLENGE = bytes.fromhex('0123456789abcdef')
REQUESTS = []
LOCK = threading.Lock()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *args):
        pass

    def respond(self, status, body=b'', **headers):
        self.send_response(status)
        self.send_header('Content-Length', str(len(body)))
        for name, value in headers.items():
            self.send_header(name.replace('_', '-'), value)
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        auth = self.headers.get('Authorization', '')
        with LOCK:
            REQUESTS.append({'path': self.path, 'auth': auth, 'cookie': self.headers.get('Cookie', ''),
                             'secret': self.headers.get('X-Private', '')})
        parsed = urllib.parse.urlsplit(self.path)
        if parsed.path == '/history':
            with LOCK:
                payload = json.dumps(REQUESTS).encode()
            return self.respond(200, payload)
        if parsed.path == '/redirect':
            return self.respond(302, Location=urllib.parse.parse_qs(parsed.query)['to'][0])
        if parsed.path == '/basic':
            if auth != 'Basic ' + base64.b64encode(b'user:Password').decode():
                return self.respond(401, WWW_Authenticate='Basic realm="Swoop"')
        if parsed.path == '/digest':
            items = {}
            for key, quoted, plain in re.findall(r'(\w+)=(?:"([^"]*)"|([^, ]+))', auth[7:]):
                items[key] = quoted or plain
            ha1 = hashlib.md5(b'user:Swoop:Password').hexdigest()
            ha2 = hashlib.md5(('GET:' + items.get('uri', '')).encode()).hexdigest()
            expected = hashlib.md5((ha1 + ':fixed-local-nonce:' + items.get('nc', '') + ':' +
                                   items.get('cnonce', '') + ':auth:' + ha2).encode()).hexdigest()
            if not (auth.startswith('Digest ') and items.get('username') == 'user' and
                    items.get('realm') == 'Swoop' and items.get('nonce') == 'fixed-local-nonce' and
                    items.get('uri') == self.path and hmac.compare_digest(items.get('response', ''), expected)):
                return self.respond(401, WWW_Authenticate='Digest realm="Swoop", nonce="fixed-local-nonce", algorithm=MD5, qop="auth"')
        if parsed.path == '/ntlm':
            try:
                message = base64.b64decode(auth[5:]) if auth.startswith('NTLM ') else b''
                kind = struct.unpack_from('<I', message, 8)[0] if message[:8] == b'NTLMSSP\0' else 0
                if kind == 1:
                    info = struct.pack('<HH', 2, 8) + 'TEST'.encode('utf-16le') + b'\0' * 4
                    packet = b'NTLMSSP\0' + struct.pack('<IHHII', 2, 0, 0, 48, 0x00888201)
                    packet += CHALLENGE + b'\0' * 8 + struct.pack('<HHI', len(info), len(info), 48) + info
                    return self.respond(401, WWW_Authenticate='NTLM ' + base64.b64encode(packet).decode())
                if kind != 3:
                    return self.respond(401, WWW_Authenticate='NTLM')
                def field(offset):
                    length, _, start = struct.unpack_from('<HHI', message, offset)
                    return message[start:start+length]
                nt_response = field(20)
                domain, user = field(28).decode('utf-16le'), field(36).decode('utf-16le')
                # Public test vector: NT hash of the fixture-only password "Password".
                nt_hash = bytes.fromhex('a4f49c406510bdcab6824ee7c30fd852')
                v2_hash = hmac.new(nt_hash, (user.upper() + domain).encode('utf-16le'), hashlib.md5).digest()
                expected = hmac.new(v2_hash, CHALLENGE + nt_response[16:], hashlib.md5).digest()
                if user != 'user' or domain != 'TEST' or len(nt_response) < 32 or not hmac.compare_digest(expected, nt_response[:16]):
                    return self.respond(403)
            except (ValueError, struct.error, UnicodeError):
                return self.respond(403)
        start, end = 0, len(DATA) - 1
        range_header = self.headers.get('Range', '')
        if range_header.startswith('bytes='):
            left, right = range_header[6:].split('-', 1)
            start, end = int(left), min(int(right) if right else end, end)
            return self.respond(206, DATA[start:end+1], Content_Range=f'bytes {start}-{end}/{len(DATA)}', ETag='"fixture-v1"')
        self.respond(200, DATA, ETag='"fixture-v1"')


class Server(http.server.ThreadingHTTPServer):
    def handle_error(self, request, client_address):
        if isinstance(sys.exception(), (BrokenPipeError, ConnectionResetError)):
            return
        super().handle_error(request, client_address)


server = Server(('127.0.0.1', 0), Handler)
server.daemon_threads = True
print(server.server_address[1], flush=True)
server.serve_forever()
