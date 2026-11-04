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
DATA = ROOT / 'run/tests' / f'workflow-{time.time_ns()}'
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
for port in range(8210,8217):
    with socket.socket() as check: check.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211)
    for i in range(1,4): start('agv',8211+i,f'CT-SIM-0{i}')
    start('ecs',8216);start('wcs',8215);start('central',8210)
    assert len(equipment()['devices'])==8
    call(8210,'/api/jobs',{'request_id':'not-equipment','from':'P1','to':'B'},400)
    signals('A',material_ready=False,auto_complete=False)
    signals('B',can_receive=False)
    first=create('workflow-1','A','B','CT-SIM-01')
    wait(lambda:job(first)['status']=='WAITING')
    assert job(first)['step']=='WAIT_SOURCE' and job(first)['vehicle_id'] is None
    assert not state()['tasks']
    assert call(8210,'/api/jobs',{'request_id':'workflow-1','from':'A','to':'B','requested_vehicle':'CT-SIM-01'})['duplicate']
    call(8210,'/api/jobs',{'request_id':'workflow-1','from':'A','to':'C','requested_vehicle':'CT-SIM-01'},409)
    signals('A',material_ready=True)
    wait(lambda:any(op['job_id']==first and op['status']=='ALLOWED' for op in equipment()['interactions']))
    assert job(first)['step']=='PICK' and not job(first)['loaded']
    assert any(l['job_id']==first and l['vehicle_id']=='CT-SIM-01' for l in state()['job_locks'])
    call(8211,'/api/sim/park',{'vehicle_id':'CT-SIM-01','node':'P1'},409)
    print('PASS: source-ready gating; arrival does not complete picking; equipment permission and persistent vehicle lock',flush=True)
    signals('A',auto_complete=True)
    wait(lambda:job(first)['step']=='WAIT_DEST' and job(first)['loaded'])
    processes[8211].terminate();processes[8211].wait(timeout=5);start('rcs',8211)
    assert any(l['job_id']==first and l['vehicle_id']=='CT-SIM-01' for l in state()['job_locks'])
    competing=call(8210,'/api/tasks',{'request_id':'other-job','kind':'MOVE','from':'A','to':'CHG1','requested_vehicle':'CT-SIM-01'},201)['task']['id']
    call(8210,f'/api/jobs/{first}/pause',{'paused':True})
    signals('B',can_receive=True)
    time.sleep(1.2)
    assert job(first)['paused'] and job(first)['step']=='WAIT_DEST'
    assert next(t for t in state()['tasks'] if t['id']==competing)['status']=='QUEUED'
    call(8210,f'/api/jobs/{first}/pause',{'paused':False})
    wait(lambda:job(first)['status']=='COMPLETED')
    result=job(first)
    assert result['vehicle_id']=='CT-SIM-01' and not result['loaded']
    assert not any(l['job_id']==first for l in state()['job_locks'])
    child=[t for t in state()['tasks'] if t['job_id']==first]
    assert len(child)==2 and all(t['kind']=='MOVE' and t['assigned_vehicle']=='CT-SIM-01' for t in child)
    ops=[op for op in equipment()['interactions'] if op['job_id']==first]
    assert len(ops)==2 and {op['action'] for op in ops}=={'PICK','DROP'}
    assert all(op['requested_at']<=op['allowed_at']<=op['completed_at'] for op in ops)
    op=ops[0];payload={key:op[key] for key in ['command_id','job_id','vehicle_id','station','action']}
    assert call(8216,'/api/interactions',payload)['duplicate']
    call(8216,'/api/interactions',dict(payload,station='C'),409)
    wait(lambda:next(t for t in state()['tasks'] if t['id']==competing)['status']=='COMPLETED')
    print('PASS: destination gating, pause/resume, same-car transport, no competing dispatch, equipment completion and idempotency',flush=True)

    signals('C',can_receive=False)
    second=create('restart-wcs','B','C','CT-SIM-02')
    wait(lambda:job(second)['step']=='WAIT_DEST' and job(second)['loaded'])
    count=len(state()['tasks'])
    processes[8215].terminate();processes[8215].wait(timeout=5);start('wcs',8215)
    assert job(second)['status']=='INTERRUPTED' and job(second)['loaded']
    assert any(l['job_id']==second for l in state()['job_locks'])
    call(8210,f'/api/jobs/{second}/pause',{'paused':False},409)
    signals('C',can_receive=True);time.sleep(.8)
    assert len(state()['tasks'])==count
    print('PASS: WCS restart interrupts loaded job and retains vehicle, no replay or automatic resume',flush=True)

    signals('CUT',auto_complete=False)
    third=create('restart-ecs','CUT','PACK','CT-SIM-03')
    wait(lambda:any(op['job_id']==third and op['status']=='ALLOWED' for op in equipment()['interactions']))
    processes[8216].terminate();processes[8216].wait(timeout=5);start('ecs',8216)
    wait(lambda:job(third)['status']=='INTERRUPTED')
    assert next(op for op in equipment()['interactions'] if op['job_id']==third)['status']=='INTERRUPTED'
    assert any(l['job_id']==third for l in state()['job_locks'])
    assert len(jobs())==3 and job(first)['status']=='COMPLETED'
    snapshot=call(8210,'/api/workflow')
    assert len(snapshot['jobs'])==3 and snapshot['equipment'] is not None
    assert any(t['source']=='wcs' and t['target']=='ecs' for t in snapshot['traffic'])
    print('PASS: ECS restart preserves uncertain handshake and blocks replay; workflow API traces available',flush=True)
finally:
    for process in processes.values():
        if process.poll() is None: process.terminate();process.wait(timeout=5)
    for handle in handles: handle.close()
