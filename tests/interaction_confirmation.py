"""WCS/ECS seven-service regression using isolated data; no demo history changes."""
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'run/tests' / f'interaction-confirm-{time.time_ns()}'
processes = {}
handles = []


def call(port, path, body=None, expected=200, source=None):
    request = urllib.request.Request(f'http://127.0.0.1:{port}{path}',
        data=None if body is None else json.dumps(body).encode(),
        headers={'Content-Type': 'application/json',**({'X-Lab-Source':source} if source else {})})
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



def wait(predicate, seconds=85):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        if predicate(): return
        time.sleep(.15)
    raise AssertionError('Workflow timed out; see '+str(DATA))
def jobs(): return call(8215,'/api/jobs')['jobs']
def job(id): return next(j for j in jobs() if j['id']==id)
def state(): return call(8211,'/api/state')
def equipment(): return call(8216,'/api/equipment')
def signals(station, **changes):
    d=next(d for d in equipment()['devices'] if d['id']==station)
    call(8216,'/api/equipment/sim/signals',dict(d,**changes))
def create(key, source, destination, vehicle):
    return call(8210,'/api/jobs',{'request_id':key,'from':source,'to':destination,'requested_vehicle':vehicle},201)['job']['id']

def policy(port,**changes):return call(port,'/api/policy',dict(call(port,'/api/policy')['policy'],**changes))['policy']
def operation(id,action='PICK'):return next(op for op in equipment()['interactions'] if op['job_id']==id and op['action']==action)
def confirmation(key,complete=False):return dict(request_id=key,operator='Test operator',note='Original car and goods checked; equipment restored',equipment_ready=True,vehicle_stopped=True,cargo_confirmed=True,interaction_result='COMPLETED' if complete else None)
def continue_job(id,c):return call(8215,f'/api/jobs/{id}/continue',c,202)
def done(id):return job(id)['status']=='COMPLETED'

DATA.mkdir(parents=True)
try:
    start('ecs',8216);signals('A',auto_complete=False)
    op=dict(command_id='old-pick',job_id='old-job',vehicle_id='CT-SIM-01',station='A',action='PICK')
    call(8216,'/api/interactions',op);wait(lambda:operation('old-job')['status']=='ALLOWED')
    c=confirmation('review-old',True);body=dict(op,confirmation=c)
    call(8216,'/api/interactions/confirm-result',body,409,source='wcs')
    signals('A',fault=True);wait(lambda:operation('old-job')['status']=='FAILED')
    signals('A',fault=False)
    call(8216,'/api/interactions/confirm-result',dict(body,vehicle_id='CT-SIM-02'),409,source='wcs')
    call(8216,'/api/interactions/confirm-result',body,source='wcs')
    assert operation('old-job')['status']=='FAILED' and not next(d for d in equipment()['devices'] if d['id']=='A')['material_ready']
    signals('A',auto_complete=True)
    second=dict(command_id='new-drop',job_id='new-job',vehicle_id='CT-SIM-01',station='A',action='DROP')
    call(8216,'/api/interactions',second);wait(lambda:operation('new-job','DROP')['status']=='COMPLETED')
    assert call(8216,'/api/interactions/confirm-result',body,source='wcs')['duplicate']
    assert next(d for d in equipment()['devices'] if d['id']=='A')['material_ready']
    call(8216,'/api/interactions/confirm-result',dict(body,confirmation=dict(c,note='changed')),409,source='wcs')
    third=dict(command_id='latest-pick',job_id='latest-job',vehicle_id='CT-SIM-01',station='A',action='PICK')
    call(8216,'/api/interactions',third);wait(lambda:operation('latest-job')['status']=='COMPLETED')
    call(8216,'/api/interactions/confirm-result',dict(second,confirmation=confirmation('old-drop-review',True)),409,source='wcs')
    assert not next(d for d in equipment()['devices'] if d['id']=='A')['material_ready']
    print('PASS: active/wrong-identity reviews rejected; original fault preserved; duplicate never overwrites later goods; old result cannot overwrite newer operation',flush=True)
finally:
    for p in processes.values():
        if p.poll() is None:p.terminate();p.wait(timeout=5)
    for h in handles:h.close()
