"""Five-process, three-vehicle end-to-end tests. Uses isolated data within project run/tests."""
import json
import os
from pathlib import Path
import socket
import subprocess
import time
import urllib.error
import urllib.request

ROOT=Path(__file__).resolve().parents[1]
BINARY=ROOT/'target'/'debug'/'rust-rcs-lab.exe'
DATA=ROOT/'run'/'tests'/('fleet-test-'+str(time.time_ns()))
CARS={'CT-SIM-01':8212,'CT-SIM-02':8213,'CT-SIM-03':8214}
processes={};handles=[]

def call(port,path,body=None,expected=200):
    payload=None if body is None else json.dumps(body).encode()
    req=urllib.request.Request(f'http://127.0.0.1:{port}{path}',data=payload,headers={'Content-Type':'application/json'})
    try:
        with urllib.request.urlopen(req,timeout=8) as r: code,value=r.status,json.load(r)
    except urllib.error.HTTPError as e: code,value=e.code,json.load(e)
    assert (code in (200,201) if port==8210 and path=='/api/tasks' and expected==200 else code==expected),(port,path,code,value)
    return value

def wait_until(predicate,seconds=60):
    deadline=time.monotonic()+seconds
    while time.monotonic()<deadline:
        if predicate():return
        time.sleep(.15)
    raise AssertionError('Timed out waiting for expected fleet state')

def start(role,port,vehicle_id=None):
    key=vehicle_id or role
    handle=open(DATA/f'{key}-{time.time_ns()}.log','w',encoding='utf-8');handles.append(handle)
    args=[str(BINARY),'--role',role]
    if vehicle_id:args+=['--vehicle-id',vehicle_id]
    processes[key]=subprocess.Popen(args,cwd=ROOT,env=dict(os.environ,RCS_DATA_DIR=str(DATA)),stdout=handle,stderr=handle,creationflags=subprocess.CREATE_NO_WINDOW)
    for _ in range(40):
        if processes[key].poll() is not None:raise AssertionError(f'{key} exited; see {handle.name}')
        try:
            h=call(port,'/health')
            if h['role']==role and (not vehicle_id or h['vehicle_id']==vehicle_id):return
        except (OSError,urllib.error.URLError):pass
        time.sleep(.1)
    raise AssertionError(f'{key} failed to start')

def stop(key):
    processes[key].terminate();processes[key].wait(timeout=5)

def tasks():return call(8211,'/api/state')['tasks']
def task(task_id):return next(t for t in tasks() if t['id']==task_id)
def create(key,source='A',destination='B'):
    return call(8210,'/api/tasks',{'request_id':key,'from':source,'to':destination})['task']['id']
def pause_all(paused):
    for port in CARS.values():call(port,'/api/control',{'paused':paused})

for port in [8210,8211,*CARS.values()]:
    with socket.socket() as sock:sock.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211);call(8211,'/api/dispatch',{'enabled':False})
    for name,port in CARS.items():start('agv',port,name)
    start('central',8210)
    assert len(call(8211,'/api/vehicles')['vehicles'])==3
    call(8213,'/api/missions',{'task_id':'wrong-car','vehicle_id':'CT-SIM-01','from':'A','to':'B'},expected=409)
    assert call(8213,'/api/state')['active'] is None
    call(8211,'/api/tasks',{'request_id':'invalid','from':'A','to':'A'},expected=400)
    first=create('duplicate-check')
    assert create('duplicate-check')==first and len(tasks())==1
    call(8211,'/api/tasks',{'request_id':'duplicate-check','from':'A','to':'C'},expected=409)
    second=create('fleet-second','B','WIP');third=create('fleet-third','C','A');fourth=create('fleet-fourth','HOME','C')
    queued_cancel=create('queued-cancel','C','B');call(8210,f'/api/cancel/{queued_cancel}',{})
    assert task(queued_cancel)['status']=='CANCELED'
    print('PASS: three independent vehicles, vehicle identity, duplicate task protection, queued cancellation',flush=True)

    call(8211,'/api/dispatch',{'enabled':True})
    wait_until(lambda:all(task(i)['status']=='RUNNING' for i in [first,second,third]))
    assigned={task(i)['assigned_vehicle'] for i in [first,second,third]}
    assert assigned==set(CARS)
    assert [task(i)['assigned_vehicle'] for i in [first,second,third]]==['CT-SIM-01','CT-SIM-03','CT-SIM-02']
    assert task(fourth)['status']=='QUEUED'
    pause_all(True)
    previous={name:call(port,'/api/state')['progress'] for name,port in CARS.items()}
    time.sleep(1.1)
    assert all(call(port,'/api/state')['progress']==previous[name] for name,port in CARS.items())
    assert task(fourth)['status']=='QUEUED'
    print('PASS: three simultaneous tasks, nearest idle selection, fourth task waits, per-car pause',flush=True)

    wrong_vehicle='CT-SIM-03'
    call(8211,'/api/events',{'task_id':first,'vehicle_id':wrong_vehicle,'seq':999,'status':'COMPLETED','progress':100,'phase':'wrong vehicle'},expected=409)
    assert task(first)['status']=='RUNNING'
    call(8213,f'/api/cancel/{first}',{},expected=409)
    call(8210,f'/api/cancel/{first}',{})
    wait_until(lambda:task(first)['status']=='CANCELED')
    assert all(task(i)['status']=='RUNNING' for i in [second,third])
    assert call(8212,'/api/state')['active'] is None
    wait_until(lambda:not call(8212,'/api/state')['pending_callbacks'])
    if any(l['vehicle_id']=='CT-SIM-01' for l in call(8211,'/api/state')['roads']['reservations']):call(8211,'/api/roads/recover',{'vehicle_id':'CT-SIM-01','node':'P1'})
    assert task(fourth)['status']=='QUEUED'
    call(8212,'/api/control',{'paused':False})
    wait_until(lambda:task(fourth)['status']=='RUNNING')
    assert task(fourth)['assigned_vehicle']=='CT-SIM-01'
    pause_all(False)
    wait_until(lambda:all(task(i)['status']=='COMPLETED' for i in [second,third,fourth]))
    print('PASS: wrong-car callbacks rejected; cancel only stops assigned vehicle; freed vehicle takes waiting task',flush=True)

    traffic=call(8210,'/api/snapshot')['traffic']
    for task_id in [second,third,fourth]:
        actual=task(task_id);car=actual['assigned_vehicle'];timing=actual['timing']
        assert actual['created_at']<=timing['dispatched_at']<=timing['accepted_at']<=timing['first_progress_at']<=timing['completed_at']
        trace=next(t for t in traffic if t['path']=='/api/missions' and t['task_id']==task_id)
        assert trace['request']['vehicle_id']==car and trace['port']==CARS[car]
        assert trace['response']['vehicle_id']==car and trace['vehicle_id']==car
        assert trace['received_at']<=trace['response_ready_at']<=trace['responded_at']
        complete=next(t for t in traffic if t['source']==car and t['task_id']==task_id and t['request'].get('status')=='COMPLETED')
        assert complete['request']['vehicle_id']==car and complete['response']['ok']
    car=task(third)['assigned_vehicle']
    call(CARS[car],'/api/missions',{'task_id':third,'vehicle_id':car,'from':'C','to':'A'})
    assert call(CARS[car],'/api/state')['active'] is None
    call(8211,'/api/events',{'task_id':third,'vehicle_id':car,'seq':999,'status':'RUNNING','progress':0,'phase':'stale'})
    assert task(third)['status']=='COMPLETED'
    catalog=call(8210,'/api/catalog');assert catalog['entry_count']==69 and catalog['service_route_count']==99
    for entry in catalog['entries']:
        if entry['method']!='GET' or entry['role'] in ['wcs','ecs'] or entry['path'] in ['/api/jobs','/api/workflow']:continue
        ports=[8210,8211,8212,8213,8214] if entry['role']=='all' else list(CARS.values()) if entry['role']=='agv' else [8210 if entry['role']=='central' else 8211]
        for port in ports:call(port,entry['path'])
    print('PASS: three-car API traces, timestamps, no completed-task replay, complete API catalog',flush=True)

    stop('CT-SIM-03')
    offline=call(8211,'/api/vehicles')['vehicles']
    assert next(v for v in offline if v['vehicle_id']=='CT-SIM-03')['online'] is False
    restart_ids=[create('offline-skip-1','B','C'),create('offline-skip-2','C','WIP')]
    wait_until(lambda:all(task(i)['status']=='RUNNING' for i in restart_ids))
    assert {task(i)['assigned_vehicle'] for i in restart_ids}=={'CT-SIM-01','CT-SIM-02'}
    for port in [8212,8213]:call(port,'/api/control',{'paused':True})
    start('agv',8214,'CT-SIM-03')
    restart_ids.append(create('vehicle-returned','A','B'))
    wait_until(lambda:task(restart_ids[-1])['status']=='RUNNING')
    assert task(restart_ids[-1])['assigned_vehicle']=='CT-SIM-03'
    call(8214,'/api/control',{'paused':True})
    stop('rcs');start('rcs',8211)
    assert all(task(i)['status']=='INTERRUPTED' for i in restart_ids)
    assert all(task(i)['assigned_vehicle'] in CARS for i in restart_ids)
    assert task(third)['status']=='COMPLETED'
    assert len(json.loads((DATA/'rcs.json').read_text(encoding='utf-8'))['tasks'])==8
    print('PASS: offline vehicle skipped, reconnect accepted, three active tasks preserved as interrupted after RCS restart',flush=True)
    print('ALL THREE-VEHICLE INTEGRATION CHECKS PASSED',flush=True)
finally:
    for process in processes.values():
        if process.poll() is None:process.terminate()
        process.wait(timeout=5)
    for handle in handles:handle.close()
