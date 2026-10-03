"""Run a small live C6 acceptance test, using only this run's own resources.

Requires paho-mqtt 2.1.0. Run against the selected board after USB deployment.
Uploads/removes one RGB565 fixture and removes every notification it creates.
Does not erase storage or dismiss messages belonging to other producers.
"""
import argparse
import importlib.util
import json
from pathlib import Path
import secrets
import socket
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import paho.mqtt.client as mqtt

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('private_config', ROOT/'scripts/with-config.py')
config = importlib.util.module_from_spec(spec)
spec.loader.exec_module(config)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--url', default='http://minicloud.local')
    args = parser.parse_args()
    base = args.url.rstrip('/')
    token = config.settings()['MINICLOUD_ADMIN_TOKEN']
    hostname = base.removeprefix('http://').split(':', 1)[0]
    board_ip = socket.gethostbyname(hostname)
    print('Board address:', hostname, board_ip)
    # Verify mDNS once, then avoid Windows resolving .local again for every
    # short-lived urllib connection. Browser DNS caches already do this.
    base = 'http://'+board_ip
    prefix = 'probe-'+secrets.token_hex(4)
    identities = []

    def http(path, method='GET', value=None, raw=None, mime='application/json', auth=False, extra=None):
        headers = {'Content-Type': mime}
        if auth:
            headers['Authorization'] = 'Bearer '+token
        headers.update(extra or {})
        data = json.dumps(value).encode() if value is not None else raw
        try:
            response = urlopen(Request(base+path, data=data, headers=headers, method=method), timeout=15)
        except HTTPError as error:
            response = error
        with response:
            data = response.read()
            return response.status, response.headers, json.loads(data) if data and response.headers.get_content_type() == 'application/json' else data

    def wait(check, seconds=30):
        end = time.monotonic()+seconds
        while time.monotonic() < end:
            try:
                result = check()
                if result:
                    return result
            except OSError:
                pass
            time.sleep(.25)
        raise AssertionError('Live board did not reach the expected state')

    def notices():
        status, _, data = http('/api/screen')
        assert status == 200, data
        return data['notices']

    def notify(identity, text, size='medium', **kwargs):
        identities.append(identity)
        payload = dict(event_id=identity+'-notify', source='deployment', id=identity, recipient='Kitchen', text=text, size=size, **kwargs)
        def accepted():
            status, _, result = http('/api/screen/notify','POST',value=payload)
            if status == 503:
                return False  # First boot may still be waiting for SNTP.
            assert status == 202, result
            return True
        wait(accepted, seconds=180)
        return wait(lambda: next((n for n in notices() if n['source'] == 'deployment' and n['id'] == identity), None))

    def read(identity):
        status, _, result = http('/api/screen/read','POST',value=dict(event_id=identity+'-read',source='deployment',id=identity))
        assert status == 202, result
        wait(lambda: not any(n['source'] == 'deployment' and n['id'] == identity for n in notices()))

    client = None
    blob = '/api/blobs/deployment/'+prefix+'.rgb565'
    uploaded = False
    started = time.time()
    try:
        status, _, initial = http('/api/status')
        assert status == 200 and initial['service'] == 'minicloud', initial
        # These dimensions come from the linked C driver, not the Rust preview.
        # A cached portrait object previously passed every HTTP-only check.
        assert initial['display'] == dict(width=320, height=172, driver='st7789-landscape-v1'), initial
        status, _, screen = http('/api/screen')
        assert status == 200 and (screen['width'], screen['height']) == (320, 172), screen
        assert initial['memory']['free'] > 0 and initial['memory']['largest_block'] > 0
        assert http('/api/blobs')[0] == 401
        for page in ['/', '/admin']:
            status, headers, body = http(page, extra={'Accept-Encoding':'gzip'})
            assert status == 200 and headers['Content-Encoding'] == 'gzip' and len(body) > 0
        for size in ['small','medium','large']:
            identity = prefix+'-'+size
            before = http('/api/status')[2]['clock']['unix_seconds']
            notice = notify(identity, 'Kitchen screen '+size, size)
            after = http('/api/status')[2]['clock']['unix_seconds']
            assert before <= notice['expires_at']-86400 <= after, notice
            assert notice['size'] == size
            read(identity)
        expiry_id = prefix+'-expiry'
        board_now = http('/api/status')[2]['clock']['unix_seconds']
        notify(expiry_id, 'This notice expires automatically', expires_at=board_now+15)
        wait(lambda: not any(n['id'] == expiry_id for n in notices()), seconds=30)

        client = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2,client_id=prefix,protocol=mqtt.MQTTv311)
        client.username_pw_set('deployment', token)
        connected = []
        client.on_connect = lambda c,u,f,code,p: connected.append(not code.is_failure)
        client.connect(board_ip,1883,keepalive=15)
        client.loop_start()
        wait(lambda: connected)
        assert connected[-1]
        mqtt_id = prefix+'-mqtt'; identities.append(mqtt_id)
        payload = dict(event_id=mqtt_id+'-notify',source='deployment',id=mqtt_id,recipient='Kitchen',text='MQTT delivery works',size='medium')
        publication = client.publish('minicloud/screen/notify',json.dumps(payload),qos=1)
        publication.wait_for_publish(timeout=15)
        assert publication.is_published()
        wait(lambda: any(n['id'] == mqtt_id for n in notices()))
        read(mqtt_id)

        # Deterministic raw RGB565 test fixture, not a decoded image/framebuffer.
        colors = [0xf800,0x07e0,0x001f,0xffff,0x0000]
        raw = b''.join(colors[(y//35)%5].to_bytes(2,'big')*320 for y in range(172))
        status, _, result = http(blob,'PUT',raw=raw,mime='application/x-rgb565',auth=True)
        assert status == 201, result
        uploaded = True
        status, headers, received = http(result['url'])
        assert status == 200 and headers.get_content_type() == 'application/x-rgb565' and received == raw
        assert http(result['url'],extra={'If-None-Match':headers['ETag']})[0] == 304
        image_id = prefix+'-image'
        notify(image_id,'RGB565 streaming works',image=dict(bucket='deployment',key=prefix+'.rgb565'))
        time.sleep(10)  # Allow at least one LCD rotation without pressing BOOT.
        read(image_id)
        assert http(blob,'DELETE',auth=True)[0] == 200
        uploaded = False
        status, _, report = http('/api/status')
        assert status == 200
        print(json.dumps(dict(initial_memory=initial['memory'], final_memory=report['memory'], storage_bytes=report['storage_bytes'], elapsed_seconds=round(time.time()-started,1)),indent=2))
        print('PASS: public HTTP, bundled Angular, text sizes, 24-hour deadline, actual expiry, read dismissal, MQTT QoS1, authenticated blob upload, public MIME/ETag streaming, RGB565 image command')
        print('LCD appearance needs human observation; a successful API/driver call does not prove visible pixels.')
    finally:
        if client:
            client.disconnect(); client.loop_stop()
        for identity in identities:
            try:
                read(identity)
            except (OSError, AssertionError):
                pass
        if uploaded:
            try:
                http(blob,'DELETE',auth=True)
            except OSError:
                pass


if __name__ == '__main__':
    main()
