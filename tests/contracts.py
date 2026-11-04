"""Service contract regression using isolated data; no demo history changes."""
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'run/tests' / f'contracts-{time.time_ns()}'
processes = {}
handles = []


def call(port, path, body=None, expected=200):
    request = urllib.request.Request(f'http://127.0.0.1:{port}{path}',
        data=None if body is None else json.dumps(body).encode(),
        headers={'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(request, timeout=10) as response:
            code, result = response.status, json.load(response)
    except urllib.error.HTTPError as error:
        code, result = error.code, json.load(error)
    assert code == expected, (port, path, code, result)
    return result


def start(role, port, vehicle=None):
    handle = (DATA / f'{port}.log').open('w', encoding='utf-8')
    handles.append(handle)
    args = [str(ROOT / 'target/debug/rust-rcs-lab.exe'), '--role', role]
    if vehicle:
        args += ['--vehicle-id', vehicle]
    processes[port] = subprocess.Popen(args, cwd=ROOT,
        env=dict(os.environ, RCS_DATA_DIR=str(DATA)), stdout=handle, stderr=handle,
        creationflags=subprocess.CREATE_NO_WINDOW)
    for _ in range(50):
        try:
            if call(port, '/health')['role'] == role:
                return
        except OSError:
            time.sleep(.1)
    raise AssertionError(f'Service failed: {port}')


for port in range(8210, 8215):
    with socket.socket() as check:
        check.bind(('127.0.0.1', port))
DATA.mkdir(parents=True)
try:
    start('rcs', 8211)
    call(8211, '/api/dispatch', {'enabled': False})
    for index in range(1, 4):
        start('agv', 8211 + index, f'CT-SIM-0{index}')
    start('central', 8210)
    body = {'request_id': 'contract-create', 'from': 'A', 'to': 'B'}
    task = call(8210, '/api/tasks', body, 201)['task']
    assert call(8210, '/api/tasks', body)['duplicate']
    call(8210, '/api/tasks', dict(body, to='C'), 409)
    call(8210, '/api/tasks', dict(body, request_id='invalid', to='A'), 400)
    for path in ['/api/sim/park', '/api/sim/blocking-demo', '/api/roads/recover']:
        call(8210, path, {}, 404)
    catalog = call(8210, '/api/catalog')
    assert (catalog['entry_count'], catalog['service_route_count']) == (69, 99)
    external = [e for e in catalog['entries'] if e['contract'] == 'external']
    assert len(external) == 8 and all(e['role'] == 'central' for e in external)
    assert all('contract_version' in e for e in catalog['entries'])
    print('PASS: central status codes, idempotency, removed simulation routes and contract inventory', flush=True)

    # Simulate central failure. RCS snapshot must not attempt a reverse call.
    processes[8210].terminate()
    processes[8210].wait(timeout=5)
    snapshot = call(8211, '/api/snapshot')
    assert any(t['id'] == task['id'] for t in snapshot['rcs']['tasks'])
    assert len(snapshot['vehicles']) == 3
    assert not any(t['source'] == 'rcs' and t['target'] == 'rcs' for t in snapshot['traffic'])
    print('PASS: RCS operates without central; snapshot has no self HTTP calls', flush=True)

    # Simulate RCS failure. Device pages remain local and other AGVs absent.
    processes[8211].terminate()
    processes[8211].wait(timeout=5)
    for index in range(1, 4):
        snapshot = call(8211 + index, '/api/snapshot')
        assert snapshot['rcs'] is None
        assert len(snapshot['vehicles']) == 1
        assert snapshot['vehicle']['vehicle_id'] == f'CT-SIM-0{index}'
    start('central', 8210)
    call(8210, '/api/tasks', dict(body, request_id='rcs-unavailable'), 502)
    print('PASS: all device pages work independently; central never bypasses offline RCS', flush=True)
finally:
    for process in processes.values():
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=5)
    for handle in handles:
        handle.close()
