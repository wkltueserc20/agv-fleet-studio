"""Whole-job cancellation regression using isolated data; no demo history changes."""
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / 'run/tests' / f'job-cancel-{time.time_ns()}'
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
    assert (code in expected if isinstance(expected,tuple) else code==expected), (port, path, code, result)
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




os.environ['WCS_CANCEL_TIMEOUT_MS']='10000'
def wait(predicate,seconds=85):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        if predicate():return
        time.sleep(.1)
    raise AssertionError('Timed out; see '+str(DATA))
def jobs():return call(8215,'/api/jobs')['jobs']
def job(id):return next(j for j in jobs() if j['id']==id)
def state():return call(8211,'/api/state')
def eq():return call(8216,'/api/equipment')
def car(port=8212):return call(port,'/api/state')
def signals(station,**changes):
    device=next(d for d in eq()['devices'] if d['id']==station)
    call(8216,'/api/equipment/sim/signals',dict(device,**changes))
def create(key,source='A',destination='B',vehicle='CT-SIM-01'):
    return call(8210,'/api/jobs',{'request_id':key,'from':source,'to':destination,'requested_vehicle':vehicle},201)['job']['id']
def cancel(id):return call(8210,f'/api/jobs/{id}/cancel',{'reason':'integration cancellation test'},202)
def stopped(id):return job(id)['status'] in ['CANCELED','INTERRUPTED','COMPLETED']
def no_lock(id):return not any(l['job_id']==id for l in state()['job_locks'])
def operation(id,action):return next(op for op in eq()['interactions'] if op['job_id']==id and op['action']==action)
for port in range(8210,8217):
    with socket.socket() as check:check.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211)
    for i in range(1,4):start('agv',8211+i,f'CT-SIM-0{i}')
    start('ecs',8216);start('wcs',8215);start('central',8210)
    signals('C',material_ready=False)
    first=create('waiting-source','C','B')
    wait(lambda:job(first)['status']=='WAITING')
    call(8210,f'/api/jobs/{first}/pause',{'paused':True})
    assert cancel(first)['job']['status']=='CANCELING'
    duplicate=call(8210,f'/api/jobs/{first}/cancel',{},(200,202));assert duplicate['duplicate']
    wait(lambda:job(first)['status']=='CANCELED')
    assert no_lock(first) and not job(first)['task_ids'] and not job(first)['paused']
    assert job(first)['cancel_requested_at']<=job(first)['canceled_at']
    assert len([e for e in job(first)['events'] if e.get('kind')=='CANCEL_REQUESTED'])==1
    assert call(8210,f'/api/jobs/{first}/cancel',{})['job']['status']=='CANCELED'
    call(8210,f'/api/jobs/{first}/pause',{'paused':False},409)
    call(8210,'/api/jobs/unknown/cancel',{},404)
    print('PASS: waiting and paused jobs cancel durably, duplicate requests do not create extra actions',flush=True)

    call(8211,'/api/dispatch',{'enabled':False})
    queued=create('queued-child')
    wait(lambda:bool(job(queued)['task_ids']))
    cancel(queued);wait(lambda:job(queued)['status']=='CANCELED')
    assert no_lock(queued)
    assert all(t['status']=='CANCELED' for t in state()['tasks'] if t['job_id']==queued)
    assert any(f['job_id']==queued and f['released_at'] for f in state()['job_cancellations'])
    call(8211,'/api/tasks',{'request_id':'late-child','kind':'MOVE','job_id':queued,'from':'A','to':'B','requested_vehicle':'CT-SIM-01'},409,source='wcs')
    call(8211,'/api/job-lock',{'job_id':queued,'from':'A'},409,source='wcs')
    call(8216,'/api/interactions',{'command_id':queued+'-PICK','job_id':queued,'vehicle_id':'CT-SIM-01','station':'A','action':'PICK'},409)
    call(8211,'/api/dispatch',{'enabled':True})
    print('PASS: queued child canceled and vehicle released; late tasks, locks and equipment commands rejected',flush=True)

    signals('A',material_ready=True,auto_complete=False)
    pending=create('pending-pick')
    wait(lambda:any(op['job_id']==pending and op['status']=='ALLOWED' for op in eq()['interactions']))
    cancel(pending);wait(lambda:stopped(pending))
    assert job(pending)['status']=='CANCELED',job(pending)
    assert operation(pending,'PICK')['status']=='CANCELED' and no_lock(pending)
    assert not job(pending)['loaded']
    signals('A',auto_complete=True);time.sleep(1.5)
    assert operation(pending,'PICK')['status']=='CANCELED'
    print('PASS: unfinished equipment handshake stops, signal re-enable cannot resurrect canceled picking',flush=True)

    signals('A',material_ready=True);signals('B',can_receive=False)
    loaded=create('loaded-cancel',vehicle='CT-SIM-02')
    wait(lambda:job(loaded)['step']=='WAIT_DEST' and job(loaded)['loaded'])
    cancel(loaded);wait(lambda:stopped(loaded))
    assert job(loaded)['status']=='INTERRUPTED' and job(loaded)['cancel_phase']=='MANUAL_REVIEW'
    assert job(loaded)['loaded'] and job(loaded)['canceled_at'] is None and not no_lock(loaded)
    assert operation(loaded,'PICK')['status']=='COMPLETED'
    assert not any(op['job_id']==loaded and op['action']=='DROP' for op in eq()['interactions'])
    call(8210,f'/api/jobs/{loaded}/pause',{'paused':False},409)
    print('PASS: loaded vehicle is retained for manual review; completed pickup is never rewritten as canceled',flush=True)

    signals('OVEN',material_ready=False)
    resume=create('cancel-restart','OVEN','PACK')
    wait(lambda:job(resume)['status']=='WAITING')
    processes[8216].terminate();processes[8216].wait(timeout=5)
    cancel(resume)
    wait(lambda:any(f['job_id']==resume for f in state()['job_cancellations']))
    original=job(resume)['cancel_requested_at']
    processes[8215].terminate();processes[8215].wait(timeout=5);start('wcs',8215);start('ecs',8216)
    wait(lambda:stopped(resume))
    assert job(resume)['status']=='CANCELED' and job(resume)['cancel_requested_at']==original
    assert not job(resume)['task_ids']
    print('PASS: cancellation survives WCS/ECS disconnect and restart without resuming normal work',flush=True)

    # Simulate a crash after RCS durably released a cancel, before WCS saved it.
    processes[8215].terminate();processes[8215].wait(timeout=5)
    db=DATA/'wcs.json';saved=json.loads(db.read_text(encoding='utf-8'))
    stale=next(j for j in saved['jobs'] if j['id']==queued)
    stale.update(status='CANCELING',cancel_phase='VERIFYING',canceled_at=None,cancel_verify_since=None,cancel_deadline_at=int(time.time()*1000)+30000)
    stale['events']=[e for e in stale['events'] if not e.get('message','').startswith('整張作業已取消：')]
    db.write_text(json.dumps(saved,ensure_ascii=False),encoding='utf-8')
    other=call(8210,'/api/tasks',{'request_id':'unrelated-after-release','kind':'MOVE','from':'P1','to':'C','requested_vehicle':'CT-SIM-01'},201)['task']['id']
    wait(lambda:car()['active'] is not None)
    start('wcs',8215);wait(lambda:job(queued)['status']=='CANCELED',seconds=5)
    wait(lambda:next(t for t in state()['tasks'] if t['id']==other)['status']=='COMPLETED')
    assert next(t for t in state()['tasks'] if t['id']==other)['status']=='COMPLETED'
    print('PASS: durable release proof resolves lost ACK without stopping the next unrelated task',flush=True)

    signals('PACK',material_ready=True);signals('WIP',auto_complete=False,can_receive=True)
    late=create('drop-wins-race','PACK','WIP')
    wait(lambda:any(op['job_id']==late and op['action']=='DROP' and op['status']=='ALLOWED' for op in eq()['interactions']))
    call(8210,f'/api/jobs/{late}/pause',{'paused':True})
    signals('WIP',auto_complete=True)
    wait(lambda:operation(late,'DROP')['status']=='COMPLETED')
    cancel(late);wait(lambda:stopped(late))
    assert job(late)['status']=='COMPLETED' and job(late)['cancel_phase']=='TOO_LATE' and no_lock(late)
    call(8210,f'/api/jobs/{late}/cancel',{},409)
    print('PASS: completed dropping wins cancellation race; no false canceled result or rollback of goods',flush=True)

    mid=create('mid-road','CUT','CNC','CT-SIM-03')
    wait(lambda:car(8214)['road_edge'] is not None)
    cancel(mid);wait(lambda:stopped(mid))
    assert job(mid)['status']=='INTERRUPTED' and not no_lock(mid)
    assert car(8214)['active'] is None
    assert any(l['vehicle_id']=='CT-SIM-03' for l in state()['roads']['reservations'])
    print('PASS: mid-road cancellation stops vehicle and retains road reservation and vehicle lock',flush=True)

    timeout=create('offline-timeout','OVEN','PACK')
    wait(lambda:job(timeout)['status']=='WAITING')
    processes[8216].terminate();processes[8216].wait(timeout=5)
    cancel(timeout);wait(lambda:job(timeout)['status']=='INTERRUPTED',seconds=20)
    assert job(timeout)['canceled_at'] is None and job(timeout)['cancel_phase']=='MANUAL_REVIEW'
    assert '逾時' in job(timeout)['wait_reason']
    start('ecs',8216)
    assert operation(pending,'PICK')['status']=='CANCELED'
    processes[8211].terminate();processes[8211].wait(timeout=5);start('rcs',8211)
    assert any(f['job_id']==queued and f['released_at'] for f in state()['job_cancellations'])
    assert not no_lock(loaded) and not no_lock(mid)
    call(8211,'/api/tasks',{'request_id':'after-restart','kind':'MOVE','job_id':queued,'from':'A','to':'B','requested_vehicle':'CT-SIM-01'},409,source='wcs')
    traces=call(8210,'/api/workflow')['traffic']
    assert any(t['job_id']==timeout and t['path'].endswith('/cancel') for t in traces)
    print('PASS: cancel confirmation timeout remains manual, fences and retained resources persist across restart',flush=True)
    def confirmation(key,target=None):return dict(request_id=key,operator='Test operator',note='Goods unloaded manually; equipment reset and road cleared',target=target,vehicle_stopped=True,cargo_cleared=True,equipment_reset=True,roads_clear=True,simulation_position=True)
    def recover(id,c):return call(8215,f'/api/jobs/{id}/recover',c,202)
    loaded_review=confirmation('review-loaded','I1')
    call(8210,f'/api/jobs/{loaded}/recover',loaded_review,403)
    call(8215,f'/api/jobs/{loaded}/recover',dict(loaded_review,cargo_cleared=False),400)
    call(8215,f'/api/jobs/{first}/recover',confirmation('terminal'),409)
    call(8211,'/api/job-recover',dict(job_id=loaded,vehicle_id='CT-SIM-02',confirmation=loaded_review),403)
    call(8212,'/api/sim/position',{'node':'I1'},source='rcs')
    call(8211,'/api/job-recover',dict(job_id=loaded,vehicle_id='CT-SIM-02',confirmation=loaded_review),409,source='wcs')
    assert not no_lock(loaded) and car(8213)['current_node']=='A'
    call(8212,'/api/sim/position',{'node':'P1'},source='rcs')
    processes[8213].terminate();processes[8213].wait(timeout=5)
    call(8211,'/api/job-recover',dict(job_id=loaded,vehicle_id='CT-SIM-02',confirmation=loaded_review),409,source='wcs')
    assert not no_lock(loaded)
    start('agv',8213,'CT-SIM-02')
    recover(loaded,loaded_review);wait(lambda:job(loaded)['recovery']['state']=='DONE')
    assert job(loaded)['status']=='CANCELED' and no_lock(loaded) and not job(loaded)['loaded']
    assert car(8213)['current_node']=='I1' and operation(loaded,'PICK')['status']=='COMPLETED'
    assert call(8215,f'/api/jobs/{loaded}/recover',loaded_review)['duplicate']
    call(8215,f'/api/jobs/{loaded}/recover',dict(loaded_review,note='different'),409)
    print('PASS: manual cargo reconciliation closes job without rewriting completed pickup; external access and incomplete confirmations rejected',flush=True)

    mid_review=confirmation('review-midroad','I2')
    recover(mid,mid_review);wait(lambda:job(mid)['recovery']['state']=='DONE')
    assert no_lock(mid) and car(8214)['current_node']=='I2' and car(8214)['road_edge'] is None
    assert not any(l['vehicle_id']=='CT-SIM-03' for l in state()['roads']['reservations'])
    assert any(r['job_id']==mid for r in state()['job_recoveries'])
    recover(timeout,confirmation('review-no-car'));wait(lambda:job(timeout)['recovery']['state']=='DONE')
    print('PASS: mid-road reconciliation clears only stopped owned resources; job with no assigned car can also close',flush=True)

    # Recover WCS's lost acknowledgment while a later unrelated task owns the same car.
    processes[8215].terminate();processes[8215].wait(timeout=5)
    saved=json.loads(db.read_text(encoding='utf-8'));stale=next(j for j in saved['jobs'] if j['id']==loaded)
    stale.update(status='RECOVERING',cancel_phase='MANUAL_REVIEW',loaded=True,canceled_at=None)
    stale['recovery'].update(state='VERIFYING',resolved_at=None,deadline_at=int(time.time()*1000)+60000)
    db.write_text(json.dumps(saved,ensure_ascii=False),encoding='utf-8')
    other=call(8210,'/api/tasks',dict(request_id='after-recovery',kind='MOVE',**{'from':'I1','to':'CUT'},requested_vehicle='CT-SIM-02'),201)['task']['id']
    wait(lambda:car(8213)['active'] is not None)
    start('wcs',8215);wait(lambda:job(loaded)['status']=='CANCELED',seconds=5)
    wait(lambda:next(t for t in state()['tasks'] if t['id']==other)['status']=='COMPLETED')
    assert car(8213)['current_node']=='CUT'
    processes[8216].terminate();processes[8216].wait(timeout=5);start('ecs',8216)
    processes[8211].terminate();processes[8211].wait(timeout=5);start('rcs',8211)
    assert len([r for r in state()['job_recoveries'] if r['job_id']==loaded])==1
    assert len([r for r in eq()['reconciliations'] if r['job_id']==loaded])==1
    assert call(8215,f'/api/jobs/{loaded}/recover',loaded_review)['duplicate']
    print('PASS: recovery receipts survive all service restarts and lost ACK never repositions next unrelated task',flush=True)
finally:
    for process in processes.values():
        if process.poll() is None:process.terminate();process.wait(timeout=5)
    for handle in handles:handle.close()
