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
DATA = ROOT / 'run/tests' / f'equipment-timeouts-{time.time_ns()}'
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

def operation(id,action='PICK'):return next(op for op in equipment()['interactions'] if op['job_id']==id and op['action']==action)
def policy(port,**changes):
    p=call(port,'/api/policy')['policy'];return call(port,'/api/policy',dict(p,**changes))['policy']
def review(id,target=None):
    call(8210,f'/api/jobs/{id}/cancel',{'reason':'timeout regression'},202)
    wait(lambda:job(id)['cancel_phase']=='MANUAL_REVIEW')
    call(8215,f'/api/jobs/{id}/recover',dict(request_id='recover-'+id,operator='Test',note='Equipment checked and cargo handled manually',target=target,vehicle_stopped=True,cargo_cleared=True,equipment_reset=True,roads_clear=True,simulation_position=True),202)
    wait(lambda:job(id)['status']=='CANCELED')
for port in range(8210,8217):
    with socket.socket() as check:check.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211)
    for i in range(1,4):start('agv',8211+i,f'CT-SIM-0{i}')
    start('ecs',8216);start('wcs',8215);start('central',8210)
    assert call(8215,'/api/policy')['policy']['source_wait_ms']==300000
    call(8215,'/api/policy',{'source_wait_ms':0},400)
    call(8216,'/api/policy',{'completion_wait_ms':999},400)
    call(8210,'/api/policy',{},404)
    policy(8215,source_wait_ms=1500)
    signals('C',material_ready=False)
    first=create('source-timeout','C','B','CT-SIM-01')
    deadline=job(first)['wait_deadline_at']
    call(8210,f'/api/jobs/{first}/pause',{'paused':True})
    policy(8215,source_wait_ms=300000)
    wait(lambda:job(first)['status']=='INTERRUPTED',seconds=5)
    assert job(first)['alarm']['code']=='SOURCE_WAIT_TIMEOUT' and job(first)['alarm']['owner']=='WCS'
    assert job(first)['wait_deadline_at']==deadline and not job(first)['vehicle_id']
    signals('C',material_ready=True);time.sleep(.7)
    assert job(first)['status']=='INTERRUPTED' and not job(first)['task_ids']
    assert len([e for e in job(first)['events'] if e.get('kind')=='ALARM'])==1
    review(first)
    print('PASS: WCS wait budget persists, includes pause, uses per-job snapshot, latches once and never auto-resumes',flush=True)

    signals('CUT',fault=True)
    fault=create('source-fault','CUT','CNC','CT-SIM-01')
    wait(lambda:job(fault)['status']=='INTERRUPTED')
    assert job(fault)['alarm']['code']=='EQUIPMENT_FAULT' and job(fault)['vehicle_id'] is None
    signals('CUT',fault=False)
    assert job(fault)['status']=='INTERRUPTED'
    review(fault)
    print('PASS: device fault stops WCS before reserving a vehicle; clearing signal does not replay old job',flush=True)

    policy(8216,allow_wait_ms=1000)
    signals('CUT',material_ready=False)
    direct=dict(command_id='direct-allow',job_id='direct-allow-job',vehicle_id='CT-SIM-01',station='CUT',action='PICK')
    call(8216,'/api/interactions',direct)
    wait(lambda:operation('direct-allow-job')['status']=='TIMED_OUT',seconds=5)
    op=operation('direct-allow-job');assert op['alarm']['code']=='ECS_ALLOW_TIMEOUT' and not op['allowed_at'] and not op['completed_at']
    signals('CUT',material_ready=True);time.sleep(1.4)
    assert operation('direct-allow-job')['status']=='TIMED_OUT'
    assert call(8216,'/api/interactions',direct)['duplicate']
    call(8216,'/api/interactions',dict(direct,command_id='late-other',job_id='other'),409)
    print('PASS: ECS allow timeout preserves original result, duplicate deadline, and blocks unresolved station',flush=True)

    policy(8216,allow_wait_ms=30000,completion_wait_ms=1000)
    signals('A',material_ready=True,auto_complete=False)
    pick=create('pick-completion-timeout','A','B','CT-SIM-01')
    wait(lambda:any(op['job_id']==pick and op['status']=='ALLOWED' for op in equipment()['interactions']))
    call(8210,f'/api/jobs/{pick}/pause',{'paused':True})
    wait(lambda:job(pick)['status']=='INTERRUPTED',seconds=5)
    assert job(pick)['alarm']['code']=='ECS_COMPLETION_TIMEOUT' and job(pick)['alarm']['owner']=='ECS'
    assert operation(pick)['status']=='TIMED_OUT' and any(l['job_id']==pick for l in state()['job_locks'])
    signals('A',auto_complete=True);time.sleep(1.4)
    assert not operation(pick)['completed_at']
    review(pick,'I1')
    assert operation(pick)['status']=='TIMED_OUT'
    print('PASS: ECS completion timeout reaches paused WCS, retains vehicle, and manual reconciliation preserves uncertainty',flush=True)

    policy(8216,completion_wait_ms=60000)
    policy(8215,destination_wait_ms=1000)
    signals('B',can_receive=False)
    dest=create('loaded-destination-timeout','A','B','CT-SIM-02')
    wait(lambda:job(dest)['status']=='INTERRUPTED')
    assert job(dest)['alarm']['code']=='DESTINATION_WAIT_TIMEOUT' and job(dest)['loaded']
    assert operation(dest)['status']=='COMPLETED' and any(l['job_id']==dest for l in state()['job_locks'])
    review(dest,'I2')
    assert operation(dest)['status']=='COMPLETED'
    print('PASS: reconciled station accepts new work; loaded destination timeout keeps completed pickup and vehicle lock',flush=True)

    signals('C',auto_complete=False,material_ready=True)
    direct_fault=dict(command_id='direct-fault',job_id='direct-fault-job',vehicle_id='CT-SIM-03',station='C',action='PICK')
    call(8216,'/api/interactions',direct_fault)
    wait(lambda:operation('direct-fault-job')['status']=='ALLOWED')
    signals('C',fault=True)
    wait(lambda:operation('direct-fault-job')['status']=='FAILED')
    assert operation('direct-fault-job')['alarm']['code']=='EQUIPMENT_FAULT'
    signals('C',fault=False,auto_complete=True);time.sleep(1.4)
    assert operation('direct-fault-job')['status']=='FAILED'
    print('PASS: equipment fault during allowed handshake is latched, never completed after signal recovers',flush=True)

    policy(8215,equipment_response_ms=1500)
    signals('OVEN',material_ready=True,auto_complete=False)
    offline=create('ecs-response-timeout','OVEN','PACK','CT-SIM-03')
    wait(lambda:any(op['job_id']==offline and op['status']=='ALLOWED' for op in equipment()['interactions']))
    processes[8216].terminate();processes[8216].wait(timeout=5)
    wait(lambda:job(offline)['status']=='INTERRUPTED',seconds=8)
    assert job(offline)['alarm']['code']=='ECS_RESPONSE_TIMEOUT' and any(l['job_id']==offline for l in state()['job_locks'])
    start('ecs',8216)
    assert operation(offline)['status']=='INTERRUPTED' and operation(offline)['alarm']['code']=='ECS_RESTART'
    assert call(8216,'/api/policy')['policy']['completion_wait_ms']==60000
    processes[8215].terminate();processes[8215].wait(timeout=5);start('wcs',8215)
    assert job(offline)['status']=='INTERRUPTED' and job(offline)['alarm']['code']=='ECS_RESPONSE_TIMEOUT'
    assert call(8215,'/api/policy')['policy']['equipment_response_ms']==1500
    review(offline,'P3')
    print('PASS: communication timeout is owned by WCS; ECS restart/policies and all latched faults persist without replay',flush=True)
finally:
    for p in processes.values():
        if p.poll() is None:p.terminate();p.wait(timeout=5)
    for h in handles:h.close()
