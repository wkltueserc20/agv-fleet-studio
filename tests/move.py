"""Point-to-point MOVE integration using isolated data; no demo history changes."""
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'run/tests' / f'move-{time.time_ns()}'
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



def wait(predicate, seconds=70):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        if predicate(): return
        time.sleep(.15)
    raise AssertionError('MOVE operation timed out')

def state(): return call(8211,'/api/state')
def task(id): return next(t for t in state()['tasks'] if t['id']==id)
def car(port=8212): return call(port,'/api/state')
def create(request, source, destination):
    return call(8210,'/api/tasks',{'request_id':request,'kind':'MOVE','from':source,'to':destination,'requested_vehicle':'CT-SIM-01'},201)['task']['id']

for port in range(8210,8215):
    with socket.socket() as check: check.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211);call(8211,'/api/dispatch',{'enabled':False})
    for index in range(1,4): start('agv',8211+index,f'CT-SIM-0{index}')
    start('central',8210)
    first=create('move-approach','A','B')
    payload={'request_id':'move-approach','kind':'MOVE','from':'A','to':'B','requested_vehicle':'CT-SIM-01'}
    assert call(8210,'/api/tasks',payload)['duplicate']
    call(8210,'/api/tasks',dict(payload,kind='TRANSPORT'),409)
    call(8210,'/api/tasks',dict(payload,request_id='invalid-kind',kind='YIELD'),400)
    call(8210,'/api/tasks',dict(payload,request_id='same-station',to='A'),400)
    call(8211,'/api/sim/park',{'vehicle_id':'CT-SIM-02','node':'B'})
    call(8211,'/api/dispatch',{'enabled':True})
    wait(lambda:task(first)['status']=='COMPLETED')
    wait(lambda:car()['pending_callbacks']==0 and not any(l['vehicle_id']=='CT-SIM-01' for l in state()['roads']['reservations']))
    result=task(first)
    assert result['kind']=='MOVE' and result['assigned_vehicle']=='CT-SIM-01'
    assert car()['current_node']=='B'
    assert [r['stage'] for r in result['routes']]==['pickup','destination']
    assert result['routes'][0]['path'][0]=='P1' and result['routes'][0]['path'][-1]=='A'
    assert result['routes'][1]['path'][0]=='A' and result['routes'][1]['path'][-1]=='B'
    assert all(r['released_at'] for r in result['routes'])
    assert any(t['kind']=='YIELD' and t['parent_task']==first and t['status']=='COMPLETED' for t in state()['tasks'])
    traces=call(8210,'/api/snapshot')['traffic']
    callbacks=[t for t in traces if t['path']=='/api/events' and t['task_id']==first]
    assert callbacks and any(t['request']['status']=='COMPLETED' for t in callbacks)
    assert not any('取料' in t['request']['phase'] or '放料' in t['request']['phase'] for t in callbacks)
    mission=next(t for t in traces if t['path']=='/api/missions' and t['task_id']==first)
    assert mission['request']['kind']=='MOVE'
    print('PASS: MOVE approaches A, travels to occupied B after automatic yield, no loading/unloading, routes released',flush=True)

    second=create('move-at-start','B','C')
    wait(lambda:car()['road_edge'] is not None)
    call(8210,f'/api/cancel/{second}',{})
    wait(lambda:task(second)['status']=='CANCELED' and car()['pending_callbacks']==0)
    assert car()['active'] is None
    assert any(l['task_id']==second for l in state()['roads']['reservations'])
    print('PASS: MOVE starting at B supports cancellation and retains uncertain road reservation',flush=True)

    call(8211,'/api/dispatch',{'enabled':False})
    third=create('move-persist','A','C')
    processes[8211].terminate();processes[8211].wait(timeout=5)
    start('rcs',8211)
    assert task(third)['kind']=='MOVE' and task(third)['status']=='QUEUED'
    assert task(first)['status']=='COMPLETED'
    print('PASS: MOVE kind and completed history persist across RCS restart',flush=True)
finally:
    for process in processes.values():
        if process.poll() is None: process.terminate();process.wait(timeout=5)
    for handle in handles: handle.close()
