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



def state():return call(8211,'/api/state')
def car(name):return call(CARS[name],'/api/state')
def park(name,node,expected=200):return call(8211,'/api/sim/park',{'vehicle_id':name,'node':node},expected)
def stations():return {s['node']:s for s in state()['stations']['stations']}
def children(parent):return [t for t in tasks() if t.get('parent_task')==parent]
def assigned(key,name,source,destination):return call(8210,'/api/tasks',{'request_id':key,'from':source,'to':destination,'requested_vehicle':name})['task']['id']
def settled():return all(not car(name)['pending_callbacks'] for name in CARS) and not any(s['reservations'] for s in stations().values()) and not state()['roads']['reservations']
def setup():
    wait_until(settled)
    for name,target in [('CT-SIM-01','P1'),('CT-SIM-02','B'),('CT-SIM-03','P3')]:park(name,target)

for port in [8210,8211,*CARS.values()]:
    with socket.socket() as sock:sock.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211)
    for name,port in CARS.items():start('agv',port,name)
    start('central',8210)
    setup()
    call(8213,'/api/control',{'paused':True})
    t=assigned('ordinary-order','CT-SIM-01','A','B')
    wait_until(lambda:bool(task(t)['wait_reason']))
    assert not children(t) and car('CT-SIM-01')['station']=='A' and car('CT-SIM-01')['progress']==37
    print('PASS: ordinary transport order waits for occupied B; paused occupant is not forced to move',flush=True)
    call(8213,'/api/control',{'paused':False})
    wait_until(lambda:bool(children(t)))
    child=children(t)[0];cid=child['id']
    wait_until(lambda:task(cid)['status']=='RUNNING')
    call(8213,'/api/control',{'paused':True})
    assert len(children(t))==1 and task(cid)['kind']=='YIELD' and task(cid)['from']=='B' and task(cid)['to']=='P2'
    assert stations()['P2']['reservations'][0]['task_id']==cid
    park('CT-SIM-03','P2',409)
    before=car('CT-SIM-02');time.sleep(1.2)
    after=car('CT-SIM-02');assert before['progress']==after['progress']
    assert task(t)['progress']==37 and task(t)['status']=='RUNNING'
    call(8213,'/api/control',{'paused':False})
    wait_until(lambda:car('CT-SIM-02')['progress']>before['progress'])
    moved=car('CT-SIM-02');assert moved['active']['kind']=='YIELD' and moved['active']['parent_task']==t
    assert (moved['x'],moved['y'])!=(80,25) and task(t)['progress']==37
    wait_until(lambda:task(cid)['status']=='COMPLETED')
    assert car('CT-SIM-02')['station']=='P2'
    wait_until(lambda:task(t)['status']=='COMPLETED')
    assert len(children(t))==1 and car('CT-SIM-01')['station']=='B'
    traces=call(8211,'/api/snapshot')['traffic']
    mission=next(x for x in traces if x['path']=='/api/missions' and x['task_id']==cid)
    assert mission['request']['kind']=='YIELD' and mission['request']['parent_task']==t and mission['port']==8213
    assert any(x['path']=='/api/entry' and x['request'].get('stage')=='move' and x['response'].get('granted') for x in traces)
    assert task(cid)['timing']['completed_at']<=task(t)['timing']['completed_at']
    print('PASS: automatic child task reserves P2, moves through intermediate coordinates over HTTP, and completes before parent proceeds; no duplicate yield',flush=True)

    setup();call(8212,'/api/control',{'paused':True})
    t=assigned('no-space','CT-SIM-01','A','B')
    call(8212,'/api/control',{'paused':False})
    wait_until(lambda:car('CT-SIM-01')['progress']>=3)
    call(8212,'/api/control',{'paused':True})
    call(8214,'/api/control',{'paused':True})
    queued=assigned('reserve-parking-route','CT-SIM-03','P1','P2')
    extra=assigned('reserve-idle-route','CT-SIM-03','I1','I2')
    call(8212,'/api/control',{'paused':False})
    wait_until(lambda:'沒有可用等待點' in (task(t)['wait_reason'] or ''))
    assert not children(t) and task(t)['progress']==37
    call(8210,f'/api/cancel/{queued}',{})
    call(8210,f'/api/cancel/{extra}',{})
    wait_until(lambda:bool(children(t)))
    cid=children(t)[0]['id']
    wait_until(lambda:task(cid)['status']=='RUNNING')
    # Cancel the parent while its accepted relocation is already moving.
    call(8210,f'/api/cancel/{t}',{})
    wait_until(lambda:task(t)['status']=='CANCELED')
    wait_until(lambda:task(cid)['status']=='COMPLETED')
    assert task(t)['status']=='CANCELED'
    call(8214,'/api/control',{'paused':False})
    print('PASS: unavailable waiting points hold parent; clearing a queued route enables yield; cancelling parent does not abandon an accepted relocation mid-move',flush=True)

    setup();t=assigned('cancel-yield','CT-SIM-01','A','B')
    wait_until(lambda:bool(children(t)));cid=children(t)[0]['id']
    wait_until(lambda:task(cid)['status']=='RUNNING')
    call(8213,'/api/control',{'paused':True})
    call(8210,f'/api/cancel/{cid}',{})
    wait_until(lambda:task(cid)['status']=='CANCELED')
    time.sleep(1.1)
    assert task(t)['progress']==37 and len(children(t))==1 and task(t)['wait_reason']
    call(8210,f'/api/cancel/{t}',{});wait_until(lambda:task(t)['status']=='CANCELED')
    wait_until(lambda:not car('CT-SIM-02')['pending_callbacks'])
    if any(l['vehicle_id']=='CT-SIM-02' for l in state()['roads']['reservations']):call(8211,'/api/roads/recover',{'vehicle_id':'CT-SIM-02','node':'P2'})
    call(8213,'/api/control',{'paused':False})
    print('PASS: cancelled relocation keeps parent waiting and is not silently recreated',flush=True)

    setup();t=assigned('restart-yield','CT-SIM-01','A','B')
    wait_until(lambda:bool(children(t)));cid=children(t)[0]['id']
    wait_until(lambda:task(cid)['status']=='RUNNING')
    pause_all(True)
    stop('rcs');start('rcs',8211)
    assert task(t)['status']=='INTERRUPTED' and task(cid)['status']=='INTERRUPTED'
    assert task(cid)['parent_task']==t
    assert stations()['P2']['reservations'][0]['task_id']==cid
    print('PASS: restart preserves parent/child links and unconfirmed waiting-point reservation',flush=True)
    print('ALL AUTOMATIC YIELD INTEGRATION CHECKS PASSED',flush=True)
finally:
    for process in processes.values():
        if process.poll() is None:process.terminate()
        process.wait(timeout=5)
    for handle in handles:handle.close()
