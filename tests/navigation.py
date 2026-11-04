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
def park(id,node):return call(8211,'/api/sim/park',{'vehicle_id':id,'node':node})
def create_for(key,id,source,dest):return call(8210,'/api/tasks',{'request_id':key,'requested_vehicle':id,'from':source,'to':dest})['task']['id']
def setup():
    wait_until(lambda:not state()['roads']['reservations'] and not any(n['reservations'] for n in state()['stations']['stations']) and not any(car(id)['pending_callbacks'] for id in CARS))
    park('CT-SIM-01','P1');park('CT-SIM-02','HOME');park('CT-SIM-03','P2')
def check_leases():
    leases=state()['roads']['reservations']
    for i,a in enumerate(leases):
        for b in leases[i+1:]:
            if a['vehicle_id']!=b['vehicle_id']:assert not set(a['path'])&set(b['path']),(a,b)

for port in [8210,8211,*CARS.values()]:
    with socket.socket() as sock:sock.bind(('127.0.0.1',port))
DATA.mkdir(parents=True)
try:
    start('rcs',8211)
    for id,port in CARS.items():start('agv',port,id)
    start('central',8210)
    graph=call(8211,'/api/map');assert len(graph['nodes'])==36 and len(graph['edges'])==41
    assert next(e for e in graph['edges'] if e['from']=='J2' and e['to']=='J5')['bidirectional'] is False
    legal={(e['from'],e['to']) for e in graph['edges']}|{(e['to'],e['from']) for e in graph['edges'] if e['bidirectional']}
    setup();a=create_for('junction-a','CT-SIM-01','P1','P3');b=create_for('junction-b','CT-SIM-02','HOME','A')
    assert create_for('junction-a','CT-SIM-01','P1','P3')==a
    call(8213,'/api/missions',{'task_id':'wrong','vehicle_id':'CT-SIM-01','from':'A','to':'B'},409)
    wait_until(lambda:any(l['stage']=='destination' for l in state()['roads']['reservations']))
    winner=next(l for l in state()['roads']['reservations'] if l['stage']=='destination');wid=winner['vehicle_id'];loser=b if wid=='CT-SIM-01' else a
    call(CARS[wid],'/api/control',{'paused':True})
    wait_until(lambda:'路段' in (task(loser)['wait_reason'] or ''))
    call(8211,'/api/sim/park',{'vehicle_id':'CT-SIM-03','node':winner['path'][0]},409)
    check_leases();before=car(wid);time.sleep(1.1);after=car(wid);assert (before['x'],before['y'])==(after['x'],after['y'])
    assert any(l['token']==winner['token'] for l in state()['roads']['reservations'])
    for t in tasks():
        for route in t['routes']:assert all(pair in legal for pair in zip(route['path'],route['path'][1:]))
    print('PASS: directed shortest paths, shared-junction exclusion, visible blocking vehicle, pause retains full route; duplicate task and vehicle identity protected',flush=True)
    call(CARS[wid],'/api/control',{'paused':False})
    wait_until(lambda:all(task(t)['status']=='COMPLETED' for t in [a,b]))
    wait_until(lambda:not state()['roads']['reservations'])
    assert car('CT-SIM-01')['station']=='P3' and car('CT-SIM-02')['station']=='A'
    assert all(r['released_at'] for t in tasks() for r in t['routes'])
    traces=call(8211,'/api/traces')['traces'];assert any(t['path']=='/api/entry' and t['response'].get('path') and t['response'].get('route_token') for t in traces)
    print('PASS: cars traverse connected edges, arrive exactly, release after telemetry, and the waiting car proceeds',flush=True)

    setup();t=create_for('cancel-midroad','CT-SIM-01','P1','P3')
    wait_until(lambda:car('CT-SIM-01')['road_edge'] is not None)
    call(8212,'/api/control',{'paused':True});token=car('CT-SIM-01')['route_token']
    call(8210,f'/api/cancel/{t}',{});wait_until(lambda:task(t)['status']=='CANCELED' and not car('CT-SIM-01')['pending_callbacks'])
    time.sleep(.7);assert any(l['token']==token for l in state()['roads']['reservations'])
    call(8212,'/api/control',{'paused':False})
    queued=create_for('after-cancel','CT-SIM-01','P1','A');time.sleep(1);assert task(queued)['status']=='QUEUED'
    call(8211,'/api/roads/recover',{'vehicle_id':'CT-SIM-01','node':'P1'})
    wait_until(lambda:task(queued)['status']=='COMPLETED');wait_until(lambda:not state()['roads']['reservations'])
    print('PASS: cancel mid-road does not free occupied roads or reassign car; explicit simulation recovery restores service',flush=True)

    setup();t=create_for('offline-road','CT-SIM-01','P1','P3');wait_until(lambda:car('CT-SIM-01')['road_edge'] is not None)
    call(8212,'/api/control',{'paused':True});token=car('CT-SIM-01')['route_token'];stop('CT-SIM-01')
    time.sleep(1);assert any(l['token']==token for l in state()['roads']['reservations'])
    call(8211,'/api/roads/recover',{'vehicle_id':'CT-SIM-01','node':'P1'},409)
    stop('rcs');start('rcs',8211)
    assert task(t)['status']=='INTERRUPTED' and any(l['token']==token for l in state()['roads']['reservations'])
    assert json.loads((DATA/'rcs.json').read_text(encoding='utf-8'))['roads']['leases']
    print('PASS: offline vehicle and RCS restart retain route reservation without replay or unsafe release',flush=True)
    print('ALL ROAD NETWORK INTEGRATION CHECKS PASSED',flush=True)
finally:
    for process in processes.values():
        if process.poll() is None:process.terminate()
        process.wait(timeout=5)
    for handle in handles:handle.close()
