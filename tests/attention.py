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
DATA = ROOT / 'run/tests' / f'attention-{time.time_ns()}'
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

def attention():return call(8215,'/api/attention')
for port in range(8210,8217):
    with socket.socket() as check:check.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
stamp=int(time.time()*1000)
def seed_job(id,age,step='WAIT_SOURCE',vehicle=None,status='INTERRUPTED'):
    return dict(id=id,request_id='seed-'+id,**{'from':'A','to':'B'},requested_vehicle=vehicle,vehicle_id=vehicle,status=status,step=step,paused=False,loaded=step=='WAIT_DEST',created_at=stamp-age-10000,updated_at=stamp-age,wait_reason='測試異常：等待人工確認',task_ids=['OWN-TASK'] if id=='JOB-CRIT' else [],events=[{'at':stamp-age,'step':step,'message':'等待人工確認'}])
seed=[seed_job('JOB-WAIT',30000),seed_job('JOB-WARN',360000,'WAIT_DEST','CT-SIM-01'),seed_job('JOB-CRIT',960000,'TO_SOURCE','CT-SIM-02'),seed_job('JOB-DONE',999999,status='COMPLETED')]
(DATA/'wcs.json').write_text(json.dumps({'jobs':seed},ensure_ascii=False),encoding='utf-8')
route=dict(token='route-owned',task_id='OWN-TASK',vehicle_id='CT-SIM-02',stage='pickup',path=['P2','J5','A'],granted_at=stamp-1000000,released_at=None)
task=dict(id='OWN-TASK',kind='MOVE',job_id='JOB-CRIT',request_id='critical-move',**{'from':'P2','to':'A'},status='INTERRUPTED',progress=20,phase='等待確認',created_at=stamp-1100000,updated_at=stamp-960000,note='',last_seq=1,assigned_vehicle='CT-SIM-02',routes=[route])
rcs=dict(tasks=[task],events=[],auto_dispatch=False,job_locks=[dict(job_id='JOB-WARN',vehicle_id='CT-SIM-01',at=stamp-400000),dict(job_id='JOB-CRIT',vehicle_id='CT-SIM-02',at=stamp-1000000)],roads={'leases':[route]},traffic={'leases':[dict(node='A',vehicle_id='CT-SIM-02',task_id='OWN-TASK',stage='pickup',at=stamp-1000000)],'observations':[]})
(DATA/'rcs.json').write_text(json.dumps(rcs,ensure_ascii=False),encoding='utf-8')
try:
    start('rcs',8211)
    for i in range(1,4):start('agv',8211+i,f'CT-SIM-0{i}')
    start('ecs',8216);start('wcs',8215);start('central',8210)
    a=attention();items={i['job_id']:i for i in a['items']}
    assert a['summary']==dict(total=3,warning=1,critical=1)
    assert [i['job_id'] for i in a['items']]==['JOB-CRIT','JOB-WARN','JOB-WAIT']
    assert items['JOB-WAIT']['severity']=='WAITING' and not items['JOB-WAIT']['resources']['vehicle_locks']
    assert items['JOB-WARN']['resources']['vehicle_locks'][0]['vehicle_id']=='CT-SIM-01'
    assert len(items['JOB-CRIT']['resources']['road_reservations'])==1 and len(items['JOB-CRIT']['resources']['station_reservations'])==1
    assert not items['JOB-CRIT']['can_continue'] and items['JOB-WARN']['can_continue']
    before=state();since={i['job_id']:i['attention_since'] for i in a['items']}
    call(8215,'/api/attention/policy',dict(warning_ms=1000,critical_ms=1000),400)
    call(8215,'/api/attention/policy',dict(warning_ms=0,critical_ms=2000),400)
    call(8210,'/api/attention',expected=404)
    call(8215,'/api/attention/policy',dict(warning_ms=600000,critical_ms=1200000))
    a=attention();assert a['summary']==dict(total=3,warning=1,critical=0)
    assert {i['job_id']:i['attention_since'] for i in a['items']}==since
    after=state();assert before['job_locks']==after['job_locks'] and before['roads']==after['roads']
    assert [(j['id'],j['status']) for j in jobs()]==[(j['id'],j['status']) for j in seed]
    print('PASS: queue sorts by age, dynamic thresholds and exact owned resources; policy/read APIs never release or execute work',flush=True)

    processes[8215].terminate();processes[8215].wait(timeout=5);start('wcs',8215)
    a=attention();assert a['policy']==dict(warning_ms=600000,critical_ms=1200000)
    assert {i['job_id']:i['attention_since'] for i in a['items']}==since
    processes[8211].terminate();processes[8211].wait(timeout=5)
    a=attention();assert a['summary']['total']==3 and a['rcs_error'] and all(not i['resources']['known'] for i in a['items'])
    start('rcs',8211);assert all(i['resources']['known'] for i in attention()['items'])
    print('PASS: waiting origin/policy survive restart, offline RCS reports unknown resources instead of false release',flush=True)

    # New actual WCS timeout gets a stable timestamp, stays visible through cancellation, then leaves only after successful closure.
    policy(8215,source_wait_ms=1000);signals('C',material_ready=False)
    fresh=create('new-attention','C','WIP','CT-SIM-03');wait(lambda:job(fresh)['status']=='INTERRUPTED')
    original=job(fresh)['attention_since'];assert original>=stamp
    call(8215,f'/api/jobs/{fresh}/cancel',{},202)
    wait(lambda:job(fresh)['cancel_phase']=='MANUAL_REVIEW')
    assert next(i for i in attention()['items'] if i['job_id']==fresh)['attention_since']==original
    c=dict(request_id='close-attention',operator='Test',note='Equipment checked, no vehicle or cargo assigned',target=None,vehicle_stopped=True,cargo_cleared=True,equipment_reset=True,roads_clear=True,simulation_position=True)
    call(8215,f'/api/jobs/{fresh}/recover',c,202);wait(lambda:job(fresh)['status']=='CANCELED')
    assert not job(fresh)['attention_since'] and fresh not in [i['job_id'] for i in attention()['items']]
    assert job(fresh)['alarm'] and job(fresh)['events']
    print('PASS: real timeout timestamp persists through processing and disappears only on verified closure; history preserved',flush=True)
    resumed=create('resumed-attention','C','WIP','CT-SIM-03');wait(lambda:job(resumed)['status']=='INTERRUPTED')
    signals('C',material_ready=True)
    continue_job(resumed,confirmation('attention-continue'))
    wait(lambda:job(resumed)['continuations'][-1]['state']=='DONE')
    assert job(resumed)['attention_since'] is None and resumed not in [i['job_id'] for i in attention()['items']]
    assert job(resumed)['continuations'][-1]['original_alarm'] and not job(resumed)['alarm']
    print('PASS: verified continuation removes resolved attention while preserving original fault and same job history',flush=True)
finally:
    for p in processes.values():
        if p.poll() is None:p.terminate();p.wait(timeout=5)
    for h in handles:h.close()
