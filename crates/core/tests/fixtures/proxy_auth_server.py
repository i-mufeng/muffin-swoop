"""Loopback-only HTTP proxy + SOCKS5 password fixtures; credentials are public test values."""
import base64
import hashlib
import hmac
import http.client
import http.server
import json
import re
import select
import socket
import socketserver
import struct
import threading
import urllib.parse

DATA = bytes(index % 251 for index in range(262163))
HISTORY = []
LOCK = threading.Lock()
CHALLENGE = bytes.fromhex('0123456789abcdef')


def record(value):
    with LOCK:
        HISTORY.append(value)


class HTTP(http.server.BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *args):
        pass

    def respond(self, status, body=b'', **headers):
        self.send_response(status)
        self.send_header('Content-Length', str(len(body)))
        for key, value in headers.items():
            self.send_header(key.replace('_', '-'), value)
        self.end_headers()
        try:
            self.wfile.write(body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def do_GET(self):
        parsed = urllib.parse.urlsplit(self.path)
        if parsed.path == '/history':
            with LOCK:
                body = json.dumps(HISTORY).encode()
            return self.respond(200, body)
        record({'kind': 'origin', 'port': self.server.server_port, 'path': self.path,
                'proxy_auth': self.headers.get('Proxy-Authorization', ''),
                'auth': self.headers.get('Authorization', ''), 'cookie': self.headers.get('Cookie', ''),
                'private': self.headers.get('X-Private', '')})
        if parsed.path == '/redirect':
            return self.respond(302, Location=urllib.parse.parse_qs(parsed.query)['to'][0])
        if parsed.path == '/basic' and self.headers.get('Authorization') != 'Basic ' + base64.b64encode(b'user:Password').decode():
            return self.respond(401, WWW_Authenticate='Basic realm="Origin"')
        start, end = 0, len(DATA) - 1
        if self.headers.get('Range', '').startswith('bytes='):
            left, right = self.headers['Range'][6:].split('-', 1)
            start, end = int(left), min(int(right) if right else end, end)
            return self.respond(206, DATA[start:end+1], Content_Range=f'bytes {start}-{end}/{len(DATA)}', ETag='"proxy-test-v1"')
        self.respond(200, DATA)


class Proxy(HTTP):
    def authenticated(self):
        method = self.server.method
        value = self.headers.get('Proxy-Authorization', '')
        record({'kind': 'proxy', 'method': method, 'path': self.path, 'auth_scheme': value.split(' ', 1)[0]})
        if method == 'basic':
            if value == 'Basic ' + base64.b64encode(b'user:Password').decode():
                return True
            self.respond(407, Proxy_Authenticate='Basic realm="Swoop"')
        elif method == 'digest':
            items = {key: quoted or plain for key, quoted, plain in re.findall(r'(\w+)=(?:"([^"]*)"|([^, ]+))', value[7:])}
            ha1 = hashlib.md5(b'user:Swoop:Password').hexdigest()
            ha2 = hashlib.md5(('GET:' + items.get('uri', '')).encode()).hexdigest()
            expected = hashlib.md5((ha1 + ':fixed-local-nonce:' + items.get('nc', '') + ':' + items.get('cnonce', '') + ':auth:' + ha2).encode()).hexdigest()
            if value.startswith('Digest ') and items.get('uri') in (self.path, urllib.parse.urlsplit(self.path).path + (('?' + urllib.parse.urlsplit(self.path).query) if urllib.parse.urlsplit(self.path).query else '')) and items.get('username') == 'user' and hmac.compare_digest(items.get('response', ''), expected):
                return True
            self.respond(407, Proxy_Authenticate='Digest realm="Swoop", nonce="fixed-local-nonce", algorithm=MD5, qop="auth"')
        elif method == 'ntlm':
            try:
                msg = base64.b64decode(value[5:]) if value.startswith('NTLM ') else b''
                kind = struct.unpack_from('<I', msg, 8)[0] if msg[:8] == b'NTLMSSP\0' else 0
                if kind == 1:
                    info = struct.pack('<HH', 2, 8) + 'TEST'.encode('utf-16le') + b'\0' * 4
                    packet = b'NTLMSSP\0' + struct.pack('<IHHII', 2, 0, 0, 48, 0x00888201)
                    packet += CHALLENGE + b'\0' * 8 + struct.pack('<HHI', len(info), len(info), 48) + info
                    self.respond(407, Proxy_Authenticate='NTLM ' + base64.b64encode(packet).decode())
                    return False
                if kind == 3:
                    def field(offset):
                        length, _, start = struct.unpack_from('<HHI', msg, offset)
                        return msg[start:start+length]
                    response = field(20)
                    domain, user = field(28).decode('utf-16le'), field(36).decode('utf-16le')
                    nt_hash = bytes.fromhex('a4f49c406510bdcab6824ee7c30fd852')
                    v2_hash = hmac.new(nt_hash, (user.upper() + domain).encode('utf-16le'), hashlib.md5).digest()
                    expected = hmac.new(v2_hash, CHALLENGE + response[16:], hashlib.md5).digest()
                    if user == 'user' and domain == 'TEST' and len(response) >= 32 and hmac.compare_digest(expected, response[:16]):
                        return True
                self.respond(407, Proxy_Authenticate='NTLM')
            except (ValueError, struct.error, UnicodeError):
                self.respond(407, Proxy_Authenticate='NTLM')
        return False

    def do_GET(self):
        if not self.authenticated():
            return
        target = urllib.parse.urlsplit(self.path)
        if target.hostname not in ('127.0.0.1', 'localhost'):
            return self.respond(502)
        headers = {key: value for key, value in self.headers.items() if key.lower() not in ('proxy-authorization', 'proxy-connection', 'connection', 'host')}
        connection = http.client.HTTPConnection(target.hostname, target.port, timeout=5)
        try:
            connection.request('GET', urllib.parse.urlunsplit(('', '', target.path, target.query, '')), headers=headers)
            response = connection.getresponse()
            body = response.read()
            self.respond(response.status, body, **{key: value for key, value in response.getheaders() if key.lower() not in ('content-length', 'server', 'date')})
        finally:
            connection.close()

    def do_CONNECT(self):
        if not self.authenticated():
            return
        host, port = self.path.rsplit(':', 1)
        if host not in ('127.0.0.1', 'localhost'):
            return self.respond(502)
        with socket.create_connection((host, int(port)), timeout=5) as upstream:
            self.send_response(200)
            self.end_headers()
            relay(self.connection, upstream)
        self.close_connection = True


def receive(connection, count):
    result = b''
    while len(result) < count:
        part = connection.recv(count-len(result))
        if not part:
            raise ConnectionError('closed')
        result += part
    return result


def relay(first, second):
    while True:
        readable, _, _ = select.select([first, second], [], [], 5)
        if not readable:
            return
        for source in readable:
            data = source.recv(65536)
            if not data:
                return
            (second if source is first else first).sendall(data)


class Socks(socketserver.BaseRequestHandler):
    def handle(self):
        connection = self.request
        try:
            version, count = receive(connection, 2)
            methods = receive(connection, count)
            if version != 5 or 2 not in methods:
                connection.sendall(b'\x05\xff')
                return
            connection.sendall(b'\x05\x02')
            version, count = receive(connection, 2)
            username = receive(connection, count)
            password = receive(connection, receive(connection, 1)[0])
            accepted = version == 1 and username == b'user' and password == b'Password'
            record({'kind': 'socks', 'accepted': accepted})
            connection.sendall(b'\x01' + (b'\x00' if accepted else b'\x01'))
            if not accepted:
                return
            version, command, _, address_type = receive(connection, 4)
            if address_type == 1:
                host = socket.inet_ntoa(receive(connection, 4))
            elif address_type == 3:
                host = receive(connection, receive(connection, 1)[0]).decode()
            else:
                return
            port = struct.unpack('!H', receive(connection, 2))[0]
            if version != 5 or command != 1 or host not in ('127.0.0.1', 'localhost'):
                return
            with socket.create_connection((host, port), timeout=5) as upstream:
                connection.sendall(b'\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00')
                relay(connection, upstream)
        except (OSError, ConnectionError):
            pass


class HTTPServer(http.server.ThreadingHTTPServer):
    daemon_threads = True
    def handle_error(self, request, address):
        pass


class SocksServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


servers = {name: HTTPServer(('127.0.0.1', 0), HTTP) for name in ('origin', 'second')}
for method in ('basic', 'digest', 'ntlm'):
    servers[method] = HTTPServer(('127.0.0.1', 0), Proxy)
    servers[method].method = method
servers['socks'] = SocksServer(('127.0.0.1', 0), Socks)
for server in servers.values():
    threading.Thread(target=server.serve_forever, daemon=True).start()
print(json.dumps({name: server.server_address[1] for name, server in servers.items()}), flush=True)
threading.Event().wait()
