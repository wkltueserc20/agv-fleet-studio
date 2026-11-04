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
DATA = ROOT / 'run/tests' / f'continuation-{time.time_ns()}'
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
for port in range(8210,8217):
    with socket.socket() as check:check.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211)
    for i in range(1,4):start('agv',8211+i,f'CT-SIM-0{i}')
    start('ecs',8216);start('wcs',8215);start('central',8210)
    policy(8215,source_wait_ms=1000)
    signals('A',material_ready=False)
    first=create('continue-source','A','B','CT-SIM-01');wait(lambda:job(first)['status']=='INTERRUPTED')
    c=confirmation('source-review')
    call(8210,f'/api/jobs/{first}/continue',c,403)
    call(8215,f'/api/jobs/{first}/continue',dict(c,cargo_confirmed=False),400)
    call(8211,'/api/job-continue-check',dict(job_id=first,vehicle_id=None,expected_node='A'),403)
    signals('A',material_ready=True)
    continue_job(first,c);wait(lambda:done(first))
    assert not job(first)['alarm'] and job(first)['continuations'][0]['original_alarm']['code']=='SOURCE_WAIT_TIMEOUT'
    assert call(8215,f'/api/jobs/{first}/continue',c)['duplicate']
    call(8215,f'/api/jobs/{first}/continue',dict(c,note='changed'),409)
    call(8215,f'/api/jobs/{first}/continue',confirmation('new-finished'),409)
    assert len([e for e in job(first)['events'] if e.get('kind')=='CONTINUE_DONE'])==1
    print('PASS: source wait resumes same job, retains history, idempotency and management boundary',flush=True)

    policy(8215,source_wait_ms=300000,destination_wait_ms=1000)
    signals('B',can_receive=False)
    dest=create('continue-destination','C','B','CT-SIM-02');wait(lambda:job(dest)['status']=='INTERRUPTED')
    assert job(dest)['loaded'] and job(dest)['step']=='WAIT_DEST'
    original_car=job(dest)['vehicle_id'];child=list(job(dest)['task_ids'])
    call(8211,'/api/job-continue-check',dict(job_id=dest,vehicle_id=original_car,expected_node='A'),409,source='wcs')
    call(8213,'/api/control',{'paused':True})
    call(8211,'/api/job-continue-check',dict(job_id=dest,vehicle_id=original_car,expected_node='C'),409,source='wcs')
    call(8213,'/api/control',{'paused':False})
    signals('B',can_receive=True)
    continue_job(dest,confirmation('dest-review'));wait(lambda:done(dest))
    assert job(dest)['vehicle_id']==original_car and job(dest)['task_ids'][:len(child)]==child
    assert len([o for o in equipment()['interactions'] if o['job_id']==dest and o['action']=='PICK'])==1
    print('PASS: loaded destination resumes on original car; wrong position/paused car rejected; pickup never repeated',flush=True)

    policy(8215,destination_wait_ms=300000)
    policy(8216,completion_wait_ms=1000)
    signals('A',material_ready=True,auto_complete=False)
    pick=create('continue-manual-pick','A','B','CT-SIM-01');wait(lambda:job(pick)['status']=='INTERRUPTED')
    assert operation(pick)['status']=='TIMED_OUT'
    c=confirmation('pick-review',True)
    call(8215,f'/api/jobs/{pick}/continue',confirmation('missing-result'),400)
    policy(8216,completion_wait_ms=60000);signals('B',can_receive=False)
    continue_job(pick,c);wait(lambda:job(pick)['step']=='WAIT_DEST')
    assert job(pick)['loaded'] and operation(pick)['status']=='TIMED_OUT'
    assert len([r for r in equipment()['confirmed_results'] if r['job_id']==pick])==1
    # Lost acknowledgment after ECS confirmation: WCS repeats verification, never the equipment command.
    processes[8215].terminate();processes[8215].wait(timeout=5)
    db=DATA/'wcs.json';saved=json.loads(db.read_text(encoding='utf-8'));j=next(j for j in saved['jobs'] if j['id']==pick)
    j.update(status='RESUMING',step='PICK',loaded=False,alarm=j['continuations'][-1]['original_alarm'])
    j['continuations'][-1].update(state='VERIFYING',resumed_at=None,deadline_at=int(time.time()*1000)+60000)
    db.write_text(json.dumps(saved,ensure_ascii=False),encoding='utf-8')
    start('wcs',8215);wait(lambda:job(pick)['step']=='WAIT_DEST')
    assert job(pick)['loaded'] and operation(pick)['status']=='TIMED_OUT'
    assert len([r for r in equipment()['confirmed_results'] if r['job_id']==pick])==1
    signals('B',can_receive=True);wait(lambda:done(pick))
    print('PASS: manual completed pickup advances once; original timeout preserved; restart/lost ACK never replays pickup',flush=True)

    signals('OVEN',material_ready=True,auto_complete=True);signals('PACK',can_receive=False,auto_complete=False)
    drop=create('continue-manual-drop','OVEN','PACK','CT-SIM-03')
    wait(lambda:job(drop)['step']=='WAIT_DEST')
    policy(8216,completion_wait_ms=1000);signals('PACK',can_receive=True)
    wait(lambda:job(drop)['status']=='INTERRUPTED')
    assert job(drop)['step']=='DROP' and job(drop)['loaded'] and operation(drop,'DROP')['status']=='TIMED_OUT'
    continue_job(drop,confirmation('drop-review',True));wait(lambda:done(drop))
    assert not job(drop)['loaded'] and operation(drop,'DROP')['status']=='TIMED_OUT'
    assert next(d for d in equipment()['devices'] if d['id']=='PACK')['material_ready']
    assert not any(l['job_id']==drop for l in state()['job_locks'])
    print('PASS: confirmed completed drop proceeds to release without repeating drop or canceling job',flush=True)

    # Force expiration of a saved verification and retry the same operator confirmation.
    policy(8215,source_wait_ms=1000);signals('CUT',material_ready=False)
    blocked=create('continue-blocked','CUT','CNC','CT-SIM-01');wait(lambda:job(blocked)['status']=='INTERRUPTED')
    c=confirmation('blocked-review');continue_job(blocked,c)
    wait(lambda:job(blocked)['continuations'][-1]['state']=='VERIFYING')
    processes[8215].terminate();processes[8215].wait(timeout=5)
    saved=json.loads(db.read_text(encoding='utf-8'));j=next(j for j in saved['jobs'] if j['id']==blocked);j['continuations'][-1]['deadline_at']=1
    db.write_text(json.dumps(saved,ensure_ascii=False),encoding='utf-8');start('wcs',8215)
    wait(lambda:job(blocked)['status']=='INTERRUPTED')
    assert job(blocked)['continuations'][-1]['state']=='BLOCKED' and not job(blocked)['task_ids']
    signals('CUT',material_ready=True,auto_complete=True);signals('CNC',can_receive=True,auto_complete=True);policy(8216,completion_wait_ms=60000)
    continue_job(blocked,c);wait(lambda:done(blocked))
    print('PASS: unmet readiness keeps job stopped, persisted verification expires, original confirmation can retry safely',flush=True)

    # Cancellation supersedes a pending continue; no late response may restart it.
    signals('C',material_ready=False)
    cancel_pending=create('continue-cancel-race','C','B','CT-SIM-02');wait(lambda:job(cancel_pending)['status']=='INTERRUPTED')
    continue_job(cancel_pending,confirmation('cancel-race-review'))
    call(8215,f'/api/jobs/{cancel_pending}/cancel',{},202)
    wait(lambda:job(cancel_pending)['status'] in ['CANCELED','INTERRUPTED'])
    signals('C',material_ready=True);time.sleep(1)
    assert not job(cancel_pending)['task_ids'] and job(cancel_pending)['cancel_requested_at']
    call(8215,f'/api/jobs/{cancel_pending}/continue',confirmation('after-cancel'),409)
    assert not any(t['path']=='/api/sim/position' for t in call(8212,'/api/traces')['traces'])
    print('PASS: cancel supersedes continue, canceled jobs cannot resume and no continuation changes vehicle position',flush=True)
finally:
    for p in processes.values():
        if p.poll() is None:p.terminate();p.wait(timeout=5)
    for h in handles:h.close()
