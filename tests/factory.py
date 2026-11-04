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
def car(id):return call(CARS[id],'/api/state')
def send(key,id,source,dest):return call(8210,'/api/tasks',{'request_id':key,'requested_vehicle':id,'from':source,'to':dest})['task']['id']
for port in [8210,8211,*CARS.values()]:
    with socket.socket() as sock:sock.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211)
    for id,port in CARS.items():start('agv',port,id)
    start('central',8210)
    graph=call(8211,'/api/map');assert len(graph['nodes'])==36 and len(graph['edges'])==41
    stations={n['id']:n for n in graph['nodes'] if n['kind']=='station'}
    assert len(stations)==18 and stations['CUT']['category']=='machine' and stations['CHG1']['category']=='charging'
    assert stations['S1']['category']=='standby' and stations['I1']['category']=='idle'
    legal={(e['from'],e['to']) for e in graph['edges']}|{(e['to'],e['from']) for e in graph['edges'] if e['bidirectional']}
    plans={'CT-SIM-01':['P1','CUT','CNC','OVEN','PACK'],'CT-SIM-02':['P2','CHG1','CHG2','S1'],'CT-SIM-03':['P3','S2','I1','I2']}
    pending={};completed=[]
    for id,path in plans.items():pending[id]=send('factory-'+id+'-1',id,path[0],path[1])
    deadline=time.monotonic()+180
    while pending and time.monotonic()<deadline:
        snapshot=state();tasks_by_id={t['id']:t for t in snapshot['tasks']}
        leases=snapshot['roads']['reservations']
        for i,a in enumerate(leases):
            for b in leases[i+1:]:
                if a['vehicle_id']!=b['vehicle_id']:assert not set(a['path'])&set(b['path'])
        for id,tid in list(pending.items()):
            t=tasks_by_id[tid]
            for r in t['routes']:assert all(p in legal for p in zip(r['path'],r['path'][1:]))
            if t['status']=='COMPLETED':
                assert car(id)['station']==plans[id][1]
                completed.append(plans[id][1]);plans[id].pop(0)
                if len(plans[id])>1:pending[id]=send('factory-'+id+'-'+str(time.time_ns()),id,plans[id][0],plans[id][1])
                else:del pending[id]
        time.sleep(.12)
    assert not pending,pending
    assert set(completed)=={'CUT','CNC','OVEN','PACK','CHG1','CHG2','S1','S2','I1','I2'}
    print('PASS: three vehicles physically reach every new machine, both charging docks, standby seats and idle spaces via legal intermediate nodes',flush=True)
    wait_until(lambda:not state()['roads']['reservations'] and not any(car(id)['pending_callbacks'] for id in CARS))
    t=send('idle-yield','CT-SIM-01','PACK','I2')
    wait_until(lambda:any(task.get('parent_task')==t for task in tasks()))
    child=next(task for task in tasks() if task.get('parent_task')==t)
    assert child['to']=='I1' and child['requested_vehicle']=='CT-SIM-03'
    wait_until(lambda:task(t)['status']=='COMPLETED',90)
    assert car('CT-SIM-01')['station']=='I2' and car('CT-SIM-03')['station']=='I1'
    assert car('CT-SIM-02')['battery']==85
    print('PASS: new idle spaces participate in automatic yielding; charging docks are navigation targets without fabricating battery charging',flush=True)
    print('ALL EXPANDED FACTORY INTEGRATION CHECKS PASSED',flush=True)
finally:
    for process in processes.values():
        if process.poll() is None:process.terminate()
        process.wait(timeout=5)
    for handle in handles:handle.close()
