const role='__ROLE__', ownVehicle='__VEHICLE_ID__';
const $=id=>document.getElementById(id);
const names={"A": "組裝機 A", "B": "檢測機 B", "C": "清洗機 C", "WIP": "立庫出入庫", "HOME": "待命總站", "P1": "避讓點 P1", "P2": "避讓點 P2", "P3": "避讓點 P3", "CUT": "雷射切割機", "CNC": "CNC 加工機", "OVEN": "烘烤爐", "PACK": "包裝機", "CHG1": "充電站 01", "CHG2": "充電站 02", "S1": "待命席 01", "S2": "待命席 02", "I1": "空閒停車 01", "I2": "空閒停車 02"};
const coordinates={"A": [15, 25], "B": [80, 25], "C": [15, 78], "WIP": [80, 78], "HOME": [48, 52], "P1": [30, 55], "P2": [48, 64], "P3": [65, 55], "CUT": [15, 7], "CNC": [36, 7], "OVEN": [58, 7], "PACK": [80, 7], "CHG1": [96, 38], "CHG2": [96, 64], "S1": [15, 94], "S2": [36, 94], "I1": [58, 94], "I2": [80, 94]};
const taskKindLabel=kind=>({TRANSPORT:'搬運',MOVE:'點到點移動',YIELD:'自動讓位'}[kind]||kind);
const labels={QUEUED:'等待派車',DISPATCHING:'交付車端',RUNNING:'執行中',COMPLETED:'已完成',CANCELED:'已取消',FAILED:'失敗',INTERRUPTED:'待確認'};
const fleetConfig=[{id:'CT-SIM-01',port:8212,color:'#266549'},{id:'CT-SIM-02',port:8213,color:'#326b95'},{id:'CT-SIM-03',port:8214,color:'#a97429'}];
let snapshot=null,requestId=null,busy=false,observed=role==='agv'?ownVehicle:'CT-SIM-01';
const labTime=ms=>new Date(Number(ms)+8*3600000).toISOString().replace('T',' ').replace('Z',' +08:00');
const titles={central:['外部介接入口：提出任務需求','同一張佇列，由 RCS 選擇三台車中可執行的車輛。','建立搬運任務'],rcs:['RCS 調度：三台車同時執行','依任務順序，選取料點最近的空閒車；忙碌、暫停或離線的車不派單。','派車控制'],agv:[ownVehicle+'：模擬車上電腦','本頁控制 '+ownVehicle+'；另外兩台車由各自的獨立程序執行。','此頁操作車：'+ownVehicle]};
$('page-title').textContent=role==='agv'?titles[role][0]:role==='central'?'外部介接入口':'RCS 車隊調度';$('page-intro').textContent=role==='agv'?'此頁只讀取與操作本車；車隊管理請開啟 RCS。':role==='central'?'Integration Service · 接收外部需求；整張作業交 WCS，單項移動交 RCS。':'查看 WCS 作業步驟、RCS 派車、ECS 設備交接與 AGV 位置。';$('control-title').textContent=role==='agv'?titles[role][2]:'提出任務需求';
document.querySelectorAll('[data-role]').forEach(e=>e.classList.toggle('current',e.dataset.role===role));
async function api(path,body){const res=await fetch(path,body===undefined?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});const value=await res.json();if(!res.ok)throw Error(value.error||'請求失敗');return value;}
function message(text,error=false){$('message').hidden=false;$('message').className='message'+(error?' error':'');$('message').textContent=text;}
function light(id,online,label){$(id).className=online?'online':'offline';$(id).textContent=label;}
function status(v){if(v?.online&&!v.active&&snapshot?.rcs?.job_locks?.some(l=>l.vehicle_id===v.vehicle_id))return '作業保留中';return !v?.online?'離線':v.paused?'暫停':v.waiting_for?'等待進站':v.active?'執行中':v.pending_callbacks?'等待補送回報':'待命';}
function observe(id){observed=id;$('observe-vehicle').value=id;if(snapshot){renderFleet(snapshot);renderRoads(snapshot);}}
function renderFleet(s){
  const cars=s.vehicles||[],tasks=s.rcs?.tasks||[];
  const count=cars.filter(v=>v.online).length;const running=cars.filter(v=>v.online&&v.active).length;
  $('fleet-summary').textContent=`${count}/${role==='agv'?1:3} 台連線 · ${running} 台執行中`;
  light('agv-light',count===(role==='agv'?1:3),`AGV ${count}/${role==='agv'?1:3} 台連線`);
  $('fleet-cards').replaceChildren();
  fleetConfig.filter(cfg=>role!=='agv'||cfg.id===ownVehicle).forEach((cfg,index)=>{
    const v=cars.find(v=>v.vehicle_id===cfg.id)||{vehicle_id:cfg.id,online:false};
    const card=document.createElement('article');card.className='fleet-card'+(observed===cfg.id?' selected':'');card.dataset.vehicle=cfg.id;
    const head=document.createElement('div');head.className='fleet-card-head';
    const b=document.createElement('button');b.className='fleet-pick';b.type='button';b.setAttribute('aria-pressed',String(observed===cfg.id));
    const dot=document.createElement('span');dot.className='fleet-dot';dot.style.background=cfg.color;const name=document.createElement('span');name.textContent=cfg.id;b.append(dot,name);b.onclick=()=>observe(cfg.id);
    const badge=document.createElement('span');badge.className='badge';badge.textContent=status(v);head.append(b,badge);
    const phase=document.createElement('p');phase.textContent=v.online?(v.paused?'已暫停 · '+v.phase:v.phase):'車端未連線，RCS 不會派單';
    const task=document.createElement('div');task.className='sub';task.textContent=v.active?`${taskKindLabel(v.active.kind)} · ${v.active.task_id} ｜ ${names[v.active.from]} → ${names[v.active.to]}`:'目前沒有執行中的任務';
    const progress=document.createElement('div');progress.className='bar';const fill=document.createElement('div');fill.style.width=(v.progress||0)+'%';fill.style.background=cfg.color;progress.append(fill);
    const link=document.createElement('a');link.href='http://127.0.0.1:'+cfg.port;link.textContent=`開啟此車操作頁 :${cfg.port} ↗`;
    card.append(head,phase,task,progress,link);$('fleet-cards').append(card);
    let marker=$('car-'+cfg.id);if(!marker){marker=document.createElement('div');marker.id='car-'+cfg.id;marker.className='car fleet-car';marker.textContent='CT-0'+(index+1);marker.style.background=cfg.color;$('fleet-markers').append(marker);}
    marker.hidden=!v.online;marker.style.left=`clamp(27px,${v.x||0}%,calc(100% - 27px))`;marker.style.top=`clamp(20px,${v.y||0}%,calc(100% - 20px))`;marker.style.transform='translate(-50%,-50%)';marker.classList.toggle('loaded',v.phase==='搬運至目的地'||v.phase==='放料中（模擬）');marker.classList.toggle('observed',observed===cfg.id);
    let route=$('route-'+cfg.id);if(!route){route=document.createElementNS('http://www.w3.org/2000/svg','path');route.id='route-'+cfg.id;route.classList.add('route','fleet-route');route.style.stroke=cfg.color;$('fleet-svg').append(route);}
    if(v.active){const a=coordinates[v.active.from],b=coordinates[v.active.to];route.setAttribute('d',`M${a[0]} ${a[1]} L${b[0]} ${b[1]}`);}else route.setAttribute('d','');
  });
  const v=cars.find(v=>v.vehicle_id===observed);$('observe-vehicle').value=observed;
  $('vehicle-state').textContent=observed+' · '+status(v);$('vehicle-phase').textContent=v?.online?(v.paused?'車端暫停（保留進度）':v.phase):'等待車端連線';$('progress-bar').style.width=(v?.progress||0)+'%';$('progress-text').textContent=(v?.progress||0)+'%';
  $('observed-detail').textContent=v?.active?`${observed} 正在執行 ${v.active.task_id}：${names[v.active.from]} → ${names[v.active.to]}；進度 ${v.progress}%。`:`${observed}：${status(v)}。`;
  document.querySelectorAll('[data-node]').forEach(n=>n.classList.toggle('target',cars.some(v=>v.active?.to===n.dataset.node)));
  const own=cars.find(v=>v.vehicle_id===ownVehicle);$('pause-toggle').textContent=own?.paused?'恢復本車模擬走行':'暫停本車模擬走行';$('pause-toggle').disabled=!own?.online;$('vehicle-cancel').disabled=!own?.active;
  $('queue-count').textContent=s.rcs?tasks.filter(t=>t.status==='QUEUED').length:'—';$('running-count').textContent=s.rcs?tasks.filter(t=>['RUNNING','DISPATCHING'].includes(t.status)).length:'—';$('complete-count').textContent=s.rcs?tasks.filter(t=>t.status==='COMPLETED').length:'—';
}
function render(s){
  snapshot=s;document.dispatchEvent(new CustomEvent('rcs-snapshot',{detail:s}));const r=s.rcs;
  light('rcs-light',!!r,role==='agv'?'本車獨立操作頁':r?'RCS 已連線':'RCS 未連線');const warnings=[];
  if(s.rcs_error)warnings.push('RCS 未連線：'+s.rcs_error);if(r?.storage_error)warnings.push('任務儲存異常：'+r.storage_error);
  (s.vehicles||[]).forEach(v=>{if(!v.online)warnings.push(v.vehicle_id+' 離線：'+v.error);if(v.callback_error)warnings.push(v.vehicle_id+' 回報待補送：'+v.callback_error);});
  $('global-error').hidden=!warnings.length;$('global-error').textContent=warnings.join('；');renderFleet(s);renderStations(s);renderConsole(s);renderRoads(s);
  if(r){$('dispatch-mode').textContent=r.auto_dispatch?'自動派車已開啟':'自動派車已暫停';$('dispatch-toggle').textContent=r.auto_dispatch?'暫停新任務派發':'開啟自動派車';}
  const tasks=r?.tasks||[];$('tasks').replaceChildren();
  [...tasks].reverse().slice(0,50).forEach(t=>{
    const tr=document.createElement('tr');const id=document.createElement('td');id.textContent=t.id;const when=document.createElement('small');when.textContent=labTime(t.created_at);id.append(when);const kind=document.createElement('small');kind.textContent=t.kind==='YIELD'?'自動讓位 · 原任務 '+t.parent_task:taskKindLabel(t.kind)+'任務';id.append(kind);
    const route=document.createElement('td');route.textContent=(names[t.from]||t.from)+' → '+(names[t.to]||t.to);
    const car=document.createElement('td');car.textContent=t.assigned_vehicle||'尚未指派／舊版未記錄';
    const state=document.createElement('td');const badge=document.createElement('span');badge.className='badge '+t.status;badge.textContent=labels[t.status]||t.status;const phase=document.createElement('small');phase.textContent=(t.wait_reason||t.phase)+' · '+t.progress+'%';state.append(badge,phase);if(t.note){const n=document.createElement('small');n.textContent=t.note;state.append(n);}
    const action=document.createElement('td');if(['QUEUED','RUNNING','DISPATCHING'].includes(t.status)){const b=document.createElement('button');b.className='cancel';b.type='button';b.textContent='取消';b.disabled=t.status==='DISPATCHING';b.onclick=async()=>{b.disabled=true;try{await api('/api/cancel/'+encodeURIComponent(t.id),{});message('已送出取消：'+t.id);}catch(e){message(e.message,true);}await pollOnce();};action.append(b);}else action.textContent='—';
    if(t.wait_reason){const w=document.createElement('small');w.textContent='開始等待：'+labTime(t.wait_since);state.append(w);}tr.append(id,route,car,state,action);$('tasks').append(tr);
  });
  if(!tasks.length){const tr=document.createElement('tr');const td=document.createElement('td');td.colSpan=5;td.className='empty';td.textContent=r?'目前沒有任務。先從介接服務建立搬運單。':'RCS 離線，暫無法讀取任務。';tr.append(td);$('tasks').append(tr);}
  $('events').replaceChildren();(r?.events||[]).slice(0,50).forEach(e=>{const row=document.createElement('div');row.className='event';const text=document.createElement('b');text.textContent=e.message;const meta=document.createElement('small');meta.textContent=labTime(e.at)+' · '+e.task_id+(e.vehicle_id?' · '+e.vehicle_id:'');row.append(text,meta);$('events').append(row);});
  if(!r?.events?.length){const row=document.createElement('div');row.className='empty';row.textContent='等待第一張任務與車端回報。';$('events').append(row);}
}
async function pollOnce(){try{render(await api('/api/snapshot'));}catch(e){$('global-error').hidden=false;$('global-error').textContent='本頁服務連線失敗：'+e.message;}}
async function poll(){await pollOnce();setTimeout(poll,850);}
function busyControls(value){busy=value;$('submit').disabled=value;$('demo-batch').disabled=value;}
$('task-form').onsubmit=async e=>{e.preventDefault();if(busy)return;if($('from').value===$('to').value){message('請選擇不同的起點與終點。',true);return;}busyControls(true);if(!requestId)requestId=crypto.randomUUID();try{const workflow=$('task-kind').value==='WORKFLOW';const v=await api(workflow?'/api/jobs':'/api/tasks',{request_id:requestId,kind:workflow?undefined:'MOVE',from:$('from').value,to:$('to').value,requested_vehicle:$('requested-vehicle').value||null});const item=workflow?v.job:v.task;message((v.duplicate?'找到同一張：':workflow?'WCS 已建立整張作業：':'RCS 已建立移動任務：')+item.id);$('last-created').hidden=false;$('last-created').textContent='最近建立：'+item.id;if(workflow){setComposer(false);trackWorkflow(item.id);}else trackTask(item.id);requestId=null;}catch(e){message(e.message+'；可再次送出，同一請求不會重複建立。',true);}finally{busyControls(false);await pollOnce();}};
for(const id of ['from','to','requested-vehicle'])$(id).onchange=()=>{requestId=null;};
$('demo-batch').onclick=async()=>{if(busy)return;busyControls(true);let count=0;try{for(const [from,to] of [['A','B'],['B','WIP'],['C','A'],['HOME','C']]){await api('/api/tasks',{request_id:crypto.randomUUID(),from,to});count++;}message('已建立 4 張測試任務。最多三車併行；點位被占用或預留時會等待，請查看點位區。');}catch(e){message('已建立 '+count+' 張；後續失敗：'+e.message,true);}finally{busyControls(false);await pollOnce();}};
$('dispatch-toggle').onclick=async()=>{try{await api('/api/dispatch',{enabled:!snapshot?.rcs?.auto_dispatch});message('派車設定已更新。');}catch(e){message(e.message,true);}await pollOnce();};
$('pause-toggle').onclick=async()=>{const own=snapshot?.vehicles?.find(v=>v.vehicle_id===ownVehicle);try{await api('/api/control',{paused:!own?.paused});message(ownVehicle+' 的暫停設定已更新。');}catch(e){message(e.message,true);}await pollOnce();};
$('vehicle-cancel').onclick=async()=>{const id=snapshot?.vehicles?.find(v=>v.vehicle_id===ownVehicle)?.active?.task_id;if(!id)return;try{await api('/api/cancel/'+encodeURIComponent(id),{});message(ownVehicle+' 的目前任務已取消。');}catch(e){message(e.message,true);}await pollOnce();};
if(role==='agv') [...$('observe-vehicle').options].forEach(o=>{if(o.value!==ownVehicle)o.remove();});
$('observe-vehicle').onchange=()=>observe($('observe-vehicle').value);poll();

function updateTaskKind(){const move=$('task-kind').value==='MOVE';$('submit').textContent=move?'建立移動任務 →':'建立搬運作業 →';for(const id of ['from','to']){for(const option of $(id).options){option.disabled=!move&&!['A','B','C','WIP','CUT','CNC','OVEN','PACK'].includes(option.value);option.hidden=option.disabled;}if($(id).selectedOptions[0]?.disabled)$(id).value=id==='from'?'A':'B';}document.querySelector('label[for=from]').textContent=move?'移動起點':'取料站點';document.querySelector('label[for=to]').textContent=move?'移動終點':'送達站點';$('task-kind-help').textContent=move?'只移動，不取料或放料。車不在起點時會先前往起點，再移到終點；仍須等候道路與點位許可。':'WCS 先確認設備，再協調 RCS 移動與 ECS 模擬取放料交接。';};

$('task-kind').onchange=updateTaskKind;updateTaskKind();

document.title=(role==='central'?'外部介接入口':role==='rcs'?'RCS 車隊調度':ownVehicle+' 車端操作')+'｜AGV 調度工作台';
