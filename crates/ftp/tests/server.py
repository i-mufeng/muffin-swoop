"""Loopback-only integration fixture. No installed service, accounts, or external hosts."""
import datetime
import ipaddress
import json
import logging
from pathlib import Path
import select
import socket
import socketserver
import ssl
import struct
import sys
import threading

from cryptography import x509
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import rsa
from cryptography.x509.oid import NameOID
from pyftpdlib.authorizers import DummyAuthorizer
from pyftpdlib.handlers import FTPHandler, TLS_FTPHandler
from pyftpdlib.servers import FTPServer

logging.disable(logging.CRITICAL)
root = Path(sys.argv[1])
mode = sys.argv[2]
events_path = root / "events.jsonl"
events_lock = threading.Lock()


def event(kind):
    # Windows CRT append writes from the FTP and proxy threads can race.
    with events_lock, events_path.open("a", encoding="utf-8") as output:
        output.write(json.dumps(kind) + "\n")


def make_certificate():
    key = rsa.generate_private_key(public_exponent=65537, key_size=2048)
    hostname = "wrong-host.invalid" if mode == "explicit-mismatch" else "localhost"
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, hostname)])
    names = [x509.DNSName(hostname)]
    if mode != "explicit-mismatch":
        names.append(x509.IPAddress(ipaddress.ip_address("127.0.0.1")))
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name)
            .public_key(key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - datetime.timedelta(days=1))
            .not_valid_after(now + datetime.timedelta(days=7))
            .add_extension(x509.BasicConstraints(ca=True, path_length=None), critical=True)
            .add_extension(x509.SubjectAlternativeName(names), critical=False).sign(key, hashes.SHA256()))
    (root / "ca.pem").write_bytes(cert.public_bytes(serialization.Encoding.PEM))
    (root / "key.pem").write_bytes(key.private_bytes(
        serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8,
        serialization.NoEncryption()))


make_certificate()
payload = bytes(range(256)) * 4096
for file in ["payload.bin", "final-error.bin", "retry-once.bin", "short.bin", "delayed-probe.bin"]:
    (root / file).write_bytes(payload)
(root / "empty.bin").write_bytes(b"")
(root / "中文 空格.bin").write_bytes(b"unicode filename content")


class Behavior:
    auth_failed_timeout = 0
    _last_retr = ""
    _attempt = 0
    retr_attempts = {}

    def on_connect(self):
        event("connect")

    def on_disconnect(self):
        event("disconnect")

    def pre_process_command(self, line, cmd, arg):
        event(cmd)  # Command names only: never log USER/PASS arguments.
        return super().pre_process_command(line, cmd, arg)

    def ftp_SIZE(self, path):
        if path.endswith("delayed-probe.bin"):
            event("probe-waiting")
            self.ioloop.call_later(2, lambda: super(Behavior, self).ftp_SIZE(path))
            return
        return super().ftp_SIZE(path)

    def ftp_REST(self, value):
        # libcurl 元数据探测可能用 REST 0 查询能力；只记录是否非零，不记录参数。
        if value != "0":
            event("REST-nonzero")
        return super().ftp_REST(value)

    def ftp_RETR(self, path):
        self._last_retr = path
        self._attempt = self.retr_attempts.get(path, 0) + 1
        self.retr_attempts[path] = self._attempt
        if path.endswith("short.bin"):
            return self.push_dtp_data(b"short", isproducer=False, cmd="RETR")
        return super().ftp_RETR(path)

    def respond(self, response, *args, **kwargs):
        if response.startswith("226") and (self._last_retr.endswith("final-error.bin") or
                (self._last_retr.endswith("retry-once.bin") and self._attempt == 1)):
            response = "426 server-secret-leak must never appear in client errors"
        if response.startswith("530"):
            response = "530 server-secret-leak invalid credentials"
        return super().respond(response, *args, **kwargs)


class PlainHandler(Behavior, FTPHandler):
    pass


class NoRestHandler(PlainHandler):
    def ftp_REST(self, value):
        self.respond("502 REST unavailable")


class PasvHandler(PlainHandler):
    masquerade_address = "203.0.113.1"  # Must be ignored by the client.

    def ftp_EPSV(self, value):
        self.respond("500 EPSV unavailable")


class SecureHandler(Behavior, TLS_FTPHandler):
    certfile = str(root / "ca.pem")
    keyfile = str(root / "key.pem")
    tls_control_required = True
    tls_data_required = True


class ImplicitHandler(SecureHandler):
    def handle(self):
        self.secure_connection(self.ssl_context)

    def handle_ssl_established(self):
        # Implicit TLS encrypts the welcome message as well.
        FTPHandler.handle(self)


handler = {"plain": PlainHandler, "explicit": SecureHandler, "implicit": ImplicitHandler,
           "explicit-mismatch": SecureHandler, "no-rest": NoRestHandler, "pasv": PasvHandler}[mode]
authorizer = DummyAuthorizer()
authorizer.add_anonymous(str(root), perm="elr")
authorizer.add_user("fixture-user", "fixture-password", str(root), perm="elr")
handler.authorizer = authorizer
server = FTPServer(("127.0.0.1", 0), handler)


def bridge(left, right):
    try:
        while True:
            readable, _, _ = select.select([left, right], [], [], 0.2)
            for source in readable:
                data = source.recv(16384)
                if not data:
                    return
                (right if source is left else left).sendall(data)
    except (OSError, ValueError):
        pass


def upstream(host, port):
    # Fixture proxies never reach external networks, even if the implementation regresses.
    if host not in ("127.0.0.1", "::1", "localhost"):
        raise ValueError("fixture proxy only allows loopback")
    return socket.create_connection(("127.0.0.1", port), timeout=3)


class Tunnel(socketserver.StreamRequestHandler):
    def handle(self):
        command = self.rfile.readline().decode("ascii").strip().split()
        if len(command) != 3 or command[0] != "CONNECT":
            return
        host, port = command[1].rsplit(":", 1)
        while self.rfile.readline() not in (b"\r\n", b"\n", b""):
            pass
        with upstream(host, int(port)) as remote:
            event("http-tunnel")
            self.connection.sendall(b"HTTP/1.1 200 Connection established\r\n\r\n")
            bridge(self.connection, remote)
        if isinstance(self.connection, ssl.SSLSocket):
            # End the HTTPS tunnel with close_notify. Closing the underlying
            # socket directly truncates TLS and Schannel rejects the FTP EOF.
            self.connection.settimeout(1)
            try:
                self.connection.unwrap().close()
            except (OSError, ValueError):
                pass


class Socks(socketserver.StreamRequestHandler):
    def handle(self):
        version, methods = self.rfile.read(2)
        if version != 5 or 0 not in self.rfile.read(methods):
            return
        self.connection.sendall(b"\x05\x00")
        version, command, _, kind = self.rfile.read(4)
        if version != 5 or command != 1:
            return
        if kind == 1:
            host = socket.inet_ntoa(self.rfile.read(4))
        elif kind == 3:
            host = self.rfile.read(self.rfile.read(1)[0]).decode("ascii")
        elif kind == 4:
            host = socket.inet_ntop(socket.AF_INET6, self.rfile.read(16))
        else:
            return
        port = struct.unpack("!H", self.rfile.read(2))[0]
        with upstream(host, port) as remote:
            event("socks-domain" if kind == 3 else "socks-ip")
            self.connection.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
            bridge(self.connection, remote)


class ProxyServer(socketserver.ThreadingTCPServer):
    daemon_threads = True
    block_on_close = False
    allow_reuse_address = True


class SecureProxy(ProxyServer):
    def get_request(self):
        connection, address = super().get_request()
        return proxy_tls.wrap_socket(connection, server_side=True), address


proxy_tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
proxy_tls.load_cert_chain(root / "ca.pem", root / "key.pem")
http_proxy = ProxyServer(("127.0.0.1", 0), Tunnel)
https_proxy = SecureProxy(("127.0.0.1", 0), Tunnel)
socks_proxy = ProxyServer(("127.0.0.1", 0), Socks)
proxies = [http_proxy, https_proxy, socks_proxy]
for proxy in proxies:
    threading.Thread(target=proxy.serve_forever, kwargs={"poll_interval": 0.02}, daemon=True).start()
print(json.dumps({"port": server.socket.getsockname()[1],
                  "http_proxy": http_proxy.server_address[1],
                  "https_proxy": https_proxy.server_address[1],
                  "socks_proxy": socks_proxy.server_address[1]}), flush=True)


def shutdown():
    sys.stdin.readline()
    server.ioloop.call_later(0, server.close_all)


threading.Thread(target=shutdown, daemon=True).start()
server.serve_forever(timeout=0.02, blocking=True, handle_exit=True)
for proxy in proxies:
    proxy.shutdown()
    proxy.server_close()
