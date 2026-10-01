"""Real HTTP and MQTT 3.1.1 smoke, using the independent Eclipse Paho client.

Starts its own binary on ephemeral ports with disposable data, then restarts it.
Never contacts a board or alters the ordinary local prototype data directory.
"""
import gzip
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import paho.mqtt.client as mqtt

ROOT = Path(__file__).resolve().parents[1]
TOKEN = "integration-test-admin-token"


def port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait(check, seconds=8):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, AssertionError):
            pass
        time.sleep(0.05)
    raise AssertionError("Condition did not become true before deadline")


def main():
    http_port, mqtt_port = port(), port()
    base = f"http://127.0.0.1:{http_port}"
    binary = Path(os.environ.get("CARGO_TARGET_DIR", str(ROOT / "target"))) / "debug" / ("minicloud.exe" if os.name == "nt" else "minicloud")

    def http(path, method="GET", value=None, raw=None, mime="application/json", auth=True, extra=None):
        headers = {"Content-Type": mime}
        if auth:
            headers["Authorization"] = "Bearer " + TOKEN
        headers.update(extra or {})
        body = json.dumps(value).encode() if value is not None else raw
        request = Request(base + path, data=body, headers=headers, method=method)
        try:
            response = urlopen(request, timeout=4)
        except HTTPError as error:
            response = error
        with response:
            data = response.read()
            result = json.loads(data) if response.headers.get_content_type() == "application/json" and data else data
            return response.status, response.headers, result

    def notice(event, identity):
        return {"event_id": event, "source": "nanacoin", "id": identity, "recipient": "Katie", "text": "You have new mail.", "size": "large"}

    with tempfile.TemporaryDirectory(prefix="minicloud-smoke-") as directory:
        env = dict(os.environ, MINICLOUD_PORT=str(http_port), MINICLOUD_MQTT_PORT=str(mqtt_port), MINICLOUD_DATA=directory, MINICLOUD_ADMIN_TOKEN=TOKEN, MINICLOUD_BIND="127.0.0.1")
        log = open(Path(directory) / "process.log", "w+")
        process = None

        def start():
            nonlocal process
            process = subprocess.Popen([str(binary)], cwd=ROOT, env=env, stdout=log, stderr=log)
            wait(lambda: http("/api/status", auth=False)[0] == 200)

        clients = []
        try:
            start()
            status, _, _ = http("/api/blobs", auth=False)
            assert status == 401
            file_bytes = bytes(range(256)) * 1024
            status, _, upload = http("/api/blobs/photos/test.bin", "PUT", raw=file_bytes, mime="application/octet-stream")
            assert status == 201, upload
            status, headers, data = http(upload["url"], auth=False)
            assert status == 200 and data == file_bytes
            assert headers.get_content_type() == "application/octet-stream"
            assert headers["Content-Disposition"] == 'inline; filename="test.bin"'
            assert http(upload["url"], auth=False, extra={"If-None-Match": headers["ETag"]})[0] == 304
            assert http(upload["url"], method="HEAD", auth=False)[2] == b""
            assert http("/api/blobs/photos/%2e%2e/escape", "PUT", raw=b"bad", mime="text/plain")[0] == 400
            assert http("/api/blobs/photos/too-big", "PUT", raw=file_bytes+b"x", mime="text/plain")[0] == 413
            status, headers, data = http("/", auth=False, extra={"Accept-Encoding": "gzip"})
            assert status == 200 and b"app-root" in gzip.decompress(data)

            messages = []
            connected = threading.Event()
            subscribed = threading.Event()
            observer = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id="smoke-observer", protocol=mqtt.MQTTv311)
            observer.username_pw_set("household", TOKEN)
            observer.on_connect = lambda c, u, flags, code, props: connected.set() if not code.is_failure else None
            observer.on_subscribe = lambda c, u, mid, codes, props: subscribed.set()
            observer.on_message = lambda c, u, message: messages.append((message.topic, message.payload))
            observer.connect("127.0.0.1", mqtt_port, keepalive=10)
            observer.loop_start()
            clients.append(observer)
            assert connected.wait(3)
            observer.subscribe("minicloud/#", qos=1)
            assert subscribed.wait(3)

            payload = notice("mqtt-first", "mail-1")
            publication = observer.publish("minicloud/screen/notify", json.dumps(payload), qos=1)
            publication.wait_for_publish(timeout=3)
            assert publication.is_published(), "QoS1 must acknowledge durable queue acceptance"
            wait(lambda: len(http("/api/screen", auth=False)[2]["notices"]) == 1)
            wait(lambda: any(t == "minicloud/jobs/result" for t, p in messages))
            observer.publish("minicloud/screen/notify", json.dumps(payload), qos=1).wait_for_publish(timeout=3)
            assert len(http("/api/screen", auth=False)[2]["notices"]) == 1

            # Public dismissal is authoritative and persists across restarts.
            assert http("/api/screen/nanacoin/mail-1/dismiss", "POST", value={}, auth=False)[0] == 200
            assert not http("/api/screen", auth=False)[2]["notices"]
            read = {"event_id": "read-second", "source": "nanacoin", "id": "mail-2"}
            observer.publish("minicloud/screen/read", json.dumps(read), qos=1).wait_for_publish(timeout=3)
            wait(lambda: any(r["id"] == "screen:read-second" for r in http("/api/jobs")[2]["receipts"]))
            http("/api/screen/notify", "POST", value=notice("http-second", "mail-2"), auth=False)
            wait(lambda: any(r["id"] == "screen:http-second" for r in http("/api/jobs")[2]["receipts"]))
            assert not http("/api/screen", auth=False)[2]["notices"]

            # Generic M2M publish/subscribe and retained delivery use real MQTT.
            observer.publish("household/weather", b"sunny", qos=1, retain=True).wait_for_publish(timeout=3)
            observer.subscribe("household/+", qos=0)
            wait(lambda: ("household/weather", b"sunny") in messages)
            rejected = threading.Event()
            bad = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id="smoke-bad")
            bad.username_pw_set("household", "wrong-token")
            bad.on_connect = lambda c,u,f,code,p: rejected.set() if code.is_failure else None
            bad.connect("127.0.0.1", mqtt_port)
            bad.loop_start()
            clients.append(bad)
            assert rejected.wait(3)

            # Prepare a real board-sized raw image and a surviving notification.
            rgb = b"\xf8\x00" * (172 * 320)
            assert http("/api/blobs/photos/red.rgb565", "PUT", raw=rgb, mime="application/x-rgb565")[0] == 201
            payload = notice("image-third", "image-3")
            payload["image"] = {"bucket": "photos", "key": "red.rgb565"}
            assert http("/api/plugins/screen/invoke", "POST", value={"topic":"minicloud/image/show", "payload":payload})[0] == 202
            wait(lambda: len(http("/api/screen", auth=False)[2]["notices"]) == 1)
            assert http("/api/blobs/photos/red.rgb565", "DELETE")[0] == 409
            for client in clients:
                client.disconnect()
                client.loop_stop()
            clients.clear()
            process.terminate()
            process.wait(timeout=5)
            start()
            screen = http("/api/screen", auth=False)[2]
            assert len(screen["notices"]) == 1 and screen["notices"][0]["id"] == "image-3"
            assert http("/blobs/photos/red.rgb565", auth=False)[2] == rgb
            assert http("/blobs/photos/test.bin", auth=False)[2] == file_bytes
            http("/api/screen/notify", "POST", value=notice("late-repeat", "mail-1"), auth=False)
            wait(lambda: any(r["id"] == "screen:late-repeat" for r in http("/api/jobs")[2]["receipts"]))
            assert len(http("/api/screen", auth=False)[2]["notices"]) == 1
            print("PASS: HTTP streaming/MIME/ETag/auth, MQTT QoS1/pub-sub/retained/auth, compiled plugin work, public dismissal, read ordering, image references, restart recovery")
        finally:
            for client in clients:
                client.disconnect()
                client.loop_stop()
            if process:
                process.terminate()
                process.wait(timeout=5)
            log.close()


if __name__ == "__main__":
    main()
