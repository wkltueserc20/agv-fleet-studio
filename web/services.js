const el=id=>document.getElementById(id);
const stationNames={A:'組裝機 A',B:'檢測機 B',C:'清洗機 C',WIP:'立庫出入庫',CUT:'雷射切割機',CNC:'CNC 加工機',OVEN:'烘烤爐',PACK:'包裝機'};
const steps={WAIT_SOURCE:'來源就緒',RESERVE:'保留用車',TO_SOURCE:'前往取料',PICK:'取料握手',WAIT_DEST:'目的地就緒',TO_DEST:'送往終點',DROP:'放料握手',RELEASE:'解除用車',DONE:'完成'};
const stateNames={RESUMING:'接續核對中',TIMED_OUT:'交接逾時',FAILED:'設備交接異常',RECOVERING:'人工復歸中',CANCELING:'取消中',CANCELED:'已取消',QUEUED:'等待處理',RUNNING:'處理中',WAITING:'等待條件',COMPLETED:'已完成',INTERRUPTED:'待人工確認',REQUESTED:'等待設備允許',ALLOWED:'已允許，交接中',PAUSED:'流程暫停'};
let snapshot=null,activePanel=service==='wcs'?'jobs':'devices',loading=false,actionError='';
const openHistory=new Set(),pendingDevices=new Set();
const uncertainStatuses=['INTERRUPTED','TIMED_OUT','FAILED'];
let jobKey='',deviceKey='',interactionKey='';
el('job-search').value=new URLSearchParams(location.search).get('job')||'';
function txt(tag,text,parent,className){const node=document.createElement(tag);node.textContent=text;if(className)node.className=className;if(parent)parent.append(node);return node;}
function time(ms){return ms==null?'—':new Date(Number(ms)+8*3600000).toISOString().replace('T',' ').replace('Z',' +08:00');}
function pill(parent,status,label){return txt('span',label||stateNames[status]||status,parent,'status-pill '+status);}
function empty(parent,text){txt('div',text,parent,'empty');}
function reviewed(op,eq){return (eq?.reconciliations||[]).some(r=>r.job_id===op.job_id)||(eq?.confirmed_results||[]).some(r=>r.command_id===op.command_id);}
function equipment(){return service==='ecs'?snapshot:snapshot?.equipment;}
function setPanel(panel,updateHash=false){
  if(!['jobs','devices','interactions','attention'].includes(panel)||(service==='ecs'&&['jobs','attention'].includes(panel)))panel=service==='wcs'?'jobs':'devices';
  activePanel=panel;
  document.querySelectorAll('[role=tabpanel]').forEach(node=>node.hidden=node.id!=='service-'+panel);
  document.querySelectorAll('[data-panel]').forEach(node=>node.setAttribute('aria-selected',String(node.dataset.panel===panel)));
  if(updateHash){history.replaceState(null,'','#'+panel);window.scrollTo({top:0,behavior:'instant'});}
}
document.querySelectorAll('[data-panel]').forEach(button=>button.onclick=()=>setPanel(button.dataset.panel,true));
window.addEventListener('hashchange',()=>setPanel(location.hash.slice(1)));
el('attention-tab').hidden=service!=='wcs';el('jobs-tab').hidden=service!=='wcs';el('open-ecs').hidden=service==='ecs';
el('service-eyebrow').textContent=service==='wcs'?'WORKFLOW / WCS':'EQUIPMENT / ECS';
el('title').textContent=service==='wcs'?'WCS 作業流程':'ECS 設備管理';
el('intro').textContent=service==='wcs'?'追蹤整張搬運作業，協調車輛移動、設備等待與取放料交接。':'查看機台狀態與設備握手，操作模擬信號以確認流程如何前進。';
el('devices-title').textContent=service==='wcs'?'ECS 設備狀態':'模擬設備信號';
el('devices-intro').textContent=service==='wcs'?'此頁唯讀；設備信號統一由 ECS 頁面操作。':'點選信號切換狀態。故障以紅色標示，正常與允許狀態以綠色標示。';
el('devices-tab').textContent=service==='wcs'?'設備狀態':'設備信號';
el('page-scope').textContent=service==='wcs'?'整張作業使用同一台車，直到交接完成才解除保留。':'八個機台／倉儲交接站 · 信號與取放料目前為模擬';
document.title=el('title').textContent+'｜AGV 調度工作台';
setPanel(location.hash.slice(1)||activePanel);

async function api(path,body){const response=await fetch(path,body===undefined?{}:{method:'POST',headers:{'Content-Type':'application/json'},body:JSON.stringify(body)});const value=await response.json();if(!response.ok)throw Error(value.error||'請求失敗');return value;}
function renderError(){const error=actionError||snapshot?.storage_error||snapshot?.equipment_error||snapshot?.equipment?.storage_error||'';el('error').hidden=!error;el('error').textContent=error;}
function renderMetrics(){
  const jobs=snapshot.jobs||[],eq=equipment(),operations=eq?.interactions||[];
  const metrics=service==='wcs'?
    [['處理中作業',jobs.filter(j=>!['COMPLETED','CANCELED','INTERRUPTED'].includes(j.status)).length,''],['等待條件',jobs.filter(j=>j.status==='WAITING'&&!j.paused).length,'warning'],['已完成作業',jobs.filter(j=>j.status==='COMPLETED').length,'success'],['待人工確認',jobs.filter(j=>j.status==='INTERRUPTED').length,'danger']]:
    [['設備總數',eq?.devices?.length||0,''],['故障設備',(eq?.devices||[]).filter(d=>d.fault).length,'danger'],['交接中',operations.filter(op=>['REQUESTED','ALLOWED'].includes(op.status)).length,'warning'],['交接待確認',operations.filter(op=>uncertainStatuses.includes(op.status)&&!reviewed(op,eq)).length,'danger']];
  const key=JSON.stringify(metrics);if(el('service-metrics').dataset.key===key)return;
  el('service-metrics').dataset.key=key;el('service-metrics').replaceChildren();
  metrics.forEach(([label,value,tone])=>{const card=txt('div','',el('service-metrics'),'metric-card');card.dataset.tone=tone;txt('span',label,card);txt('strong',String(value),card);});
}
function renderJobs(){
  if(!snapshot)return;
  const query=el('job-search').value.trim().toLowerCase(),filter=el('job-filter').value;
  const jobs=(snapshot.jobs||[]).filter(job=>{
    const match=filter==='all'||(filter==='paused'?job.paused:filter==='open'?!['COMPLETED','CANCELED','INTERRUPTED'].includes(job.status):job.status===filter);
    return match&&[job.id,job.from,job.to,job.vehicle_id,stationNames[job.from],stationNames[job.to]].join(' ').toLowerCase().includes(query);
  }).reverse();
  el('job-count').textContent='符合 '+jobs.length+' 張作業';
  const key=JSON.stringify(jobs.map(({updated_at,...rest})=>rest));if(jobKey===key)return;jobKey=key;
  const list=el('jobs');list.replaceChildren();if(!jobs.length){empty(list,filter==='all'&&!query?'尚無作業，請從介接服務提出搬運需求。':'沒有符合條件的作業。');return;}
  jobs.forEach(job=>{
    const card=txt('article','',list,'service-job');card.dataset.job=job.id;
    const head=txt('div','',card,'service-job-head'),identity=txt('div','',head);
    txt('div',(stationNames[job.from]||job.from)+' → '+(stationNames[job.to]||job.to),identity,'job-route');
    txt('div',job.id,identity,'mono');const badges=txt('div','',head,'job-badges');pill(badges,job.status);if(job.paused)pill(badges,'PAUSED');
    const body=txt('div','',card,'service-job-body');
    txt('div',job.status==='COMPLETED'?'移動與設備交接均已完成':job.status==='CANCELED'?'取消已確認，未執行後續作業':job.status==='RESUMING'?'核對原車與設備後接續原作業':job.status==='RECOVERING'?'人工確認已保存，核對車況與設備後解除保留':job.status==='CANCELING'?'取消處理中，正在確認停止與資源狀態':job.status==='INTERRUPTED'?'保留用車與進度，等待人工確認':(job.paused?'流程暫停於：':job.status==='WAITING'?'正在等待：':'目前步驟：')+(steps[job.step]||job.step),body,'job-stage-caption');
    const stages=txt('div','',body,'job-stages'),current=Object.keys(steps).indexOf(job.step);
    Object.entries(steps).forEach(([step,label],index)=>txt('span',label,stages,'job-stage'+(step===job.step?(['CANCELED','INTERRUPTED'].includes(job.status)?' stopped':' current'):index<current?' done':'')));
    const context=txt('div','',body,'job-context');txt('span','作業用車：'+(job.vehicle_id||'尚未指派'),context);txt('span',job.status==='INTERRUPTED'?'載貨／交接結果待確認':'WCS 最近載貨紀錄：'+(job.loaded?'模擬已取料':'模擬未載貨'),context);
    if(job.wait_reason)txt('div',job.wait_reason,body,'job-wait'+(job.status==='INTERRUPTED'?' interrupted':''));
    if(job.alarm){const a=job.alarm,box=txt('div','',body,'equipment-alarm');txt('strong',(job.recovery?.state==='DONE'?'歷史異常（已人工對帳） · ':'')+a.owner+' · '+a.code+' · '+a.station,box);txt('span',a.message,box);txt('span','發生 '+time(a.occurred_at)+(a.command_id?' ／ '+a.command_id:''),box);txt('span',job.recovery?.state==='DONE'?'處理：人工確認與模擬復歸已完成，保留此異常紀錄。':'處理：確認異常後接續原作業，或取消並人工結案。',box);}
    if(job.wait_deadline_at)txt('div','WCS 本步等待期限：'+time(job.wait_deadline_at)+'（包含流程暫停時間）',body,'job-wait-clock');
    txt('div','RCS 子任務：'+(job.task_ids.join('、')||'尚未建立；已在站點時會省略原地移動'),body,'job-children');
    if(job.cancel_requested_at){const cancel=txt('div','',body,'job-cancel-info');txt('strong','取消處理：'+({REQUESTED:'已提出要求',STOPPING:'正在協調停止',VERIFYING:'確認最終結果',MANUAL_REVIEW:'待人工對帳',DONE:'已取消',MANUALLY_RESOLVED:'人工確認已結案',TOO_LATE:'交接已完成，未撤銷'}[job.cancel_phase]||'處理中'),cancel);txt('div','提出 '+time(job.cancel_requested_at)+(job.canceled_at?' ／ 取消確認 '+time(job.canceled_at):''),cancel,'mono');if(job.cancel_reason)txt('div','原因：'+job.cancel_reason,cancel);}
    if(job.recovery){const info=txt('div','',body,'job-recovery-info'),r=job.recovery;txt('strong','人工復歸：'+({REQUESTED:'已保存確認',VERIFYING:'核對中',BLOCKED:'尚未完成，可用原內容重試',DONE:'已完成'}[r.state]||r.state),info);txt('div','處理人：'+r.confirmation.operator+' ／ 模擬位置：'+(r.confirmation.target||'未指派車輛'),info);txt('div',r.confirmation.note,info);txt('div','提出 '+time(r.requested_at)+(r.resolved_at?' ／ 完成 '+time(r.resolved_at):''),info,'mono');}
    if(job.continuations?.length){const info=txt('div','',body,'job-recovery-info');for(const a of job.continuations){txt('strong','人工接續：'+({DONE:'已接續',BLOCKED:'尚未通過核對',REQUESTED:'已保存確認',VERIFYING:'核對中'}[a.state]||a.state)+' · '+(steps[a.step]||a.step),info);txt('div',a.confirmation.operator+' · '+a.confirmation.note,info);txt('div','提出 '+time(a.requested_at)+(a.resumed_at?' ／ 接續 '+time(a.resumed_at):''),info);if(a.original_alarm)txt('div','原異常：'+a.original_alarm.code,info);}}
    const bottom=txt('div','',body,'job-bottom'),details=document.createElement('details');details.open=openHistory.has(job.id);details.ontoggle=()=>details.open?openHistory.add(job.id):openHistory.delete(job.id);bottom.append(details);txt('summary','流程時間點',details);const timeline=txt('ol','',details,'job-history');
    job.events.forEach(event=>{const item=txt('li',event.message,timeline);txt('time',time(event.at),item);});
    if(!['COMPLETED','CANCELED','CANCELING','RECOVERING','RESUMING','INTERRUPTED'].includes(job.status)){
      const button=txt('button',job.paused?'繼續流程':'暫停後續步驟',bottom,'button');button.type='button';
      button.onclick=async()=>{button.disabled=true;try{await api('/api/jobs/'+encodeURIComponent(job.id)+'/pause',{paused:!job.paused});actionError='';}catch(error){actionError=error.message;}jobKey='';await refresh();};
    }
    if(job.status==='INTERRUPTED'&&!job.cancel_requested_at&&['WAIT_SOURCE','WAIT_DEST','PICK','DROP'].includes(job.step)){const b=txt('button','確認異常後繼續',bottom,'button primary');b.type='button';b.onclick=()=>openContinue(job);}
    if(job.status==='INTERRUPTED'&&job.cancel_requested_at){const button=txt('button',job.recovery?'重試人工復歸':'人工確認與模擬復歸',bottom,'button primary');button.type='button';button.onclick=()=>openRecovery(job);}
    if(!['COMPLETED','CANCELED'].includes(job.status)&&!job.cancel_requested_at){const button=txt('button','取消整張作業',bottom,'button danger');button.type='button';button.onclick=async()=>{button.disabled=true;try{await api('/api/jobs/'+encodeURIComponent(job.id)+'/cancel',{reason:'操作員從 WCS 頁面取消整張作業'});actionError='';}catch(error){actionError=error.message;}jobKey='';await refresh();};}
  });
}
function renderDevices(){
  const eq=equipment(),query=el('device-search').value.trim().toLowerCase(),filter=el('device-filter').value,operations=eq?.interactions||[];
  const devices=(eq?.devices||[]).filter(device=>{
    const related=operations.filter(op=>op.station===device.id);
    return (filter==='all'||(filter==='attention'?device.fault||related.some(op=>uncertainStatuses.includes(op.status)&&!reviewed(op,eq)):related.some(op=>['REQUESTED','ALLOWED'].includes(op.status))))
      &&[device.id,stationNames[device.id]].join(' ').toLowerCase().includes(query);
  });
  el('device-count').textContent='符合 '+devices.length+' 個設備';
  const key=JSON.stringify([devices,operations.filter(op=>uncertainStatuses.includes(op.status)).map(op=>op.station),eq?.storage_error,eq?.reconciliations,eq?.confirmed_results]);if(deviceKey===key)return;deviceKey=key;
  const list=el('devices');list.replaceChildren();if(!devices.length){empty(list,eq?'沒有符合條件的設備。':'ECS 尚未連線，暫無設備資料。');return;}
  devices.forEach(device=>{
    const uncertain=operations.some(op=>op.station===device.id&&uncertainStatuses.includes(op.status)&&!reviewed(op,eq));
    const card=txt('article','',list,'equipment-card'+(device.fault||uncertain?' has-fault':''));card.dataset.station=device.id;
    const head=txt('div','',card,'equipment-card-head'),identity=txt('div','',head);txt('h3',stationNames[device.id]||device.id,identity);txt('span',device.id+' · 模擬設備',identity,'station-id');pill(head,device.fault?'fault':uncertain?'INTERRUPTED':'healthy',device.fault?'故障':uncertain?'待確認':'正常');
    const signals=txt('div','',card,'equipment-signals');
    for(const [key,label,on,off] of [['material_ready','有料信號','有料','無料'],['can_receive','收料允許','允許','未允許'],['fault','設備故障','故障','正常'],['auto_complete','交接完成模式','自動完成','手動等待']]){
      const active=!!device[key],row=txt(service==='ecs'?'button':'div','',signals,(service==='ecs'?'signal-toggle':'signal-row')+(key==='fault'&&active?' alert':active?' active':''));txt('span',label,row);txt('span',active?on:off,row,'signal-value');
      if(service==='ecs'){
        row.type='button';row.setAttribute('aria-pressed',String(active));row.disabled=!!eq.storage_error;
        row.onclick=async()=>{if(pendingDevices.has(device.id))return;pendingDevices.add(device.id);row.disabled=true;try{await api('/api/equipment/sim/signals',{...device,[key]:!active});actionError='';}catch(error){actionError=error.message;}finally{pendingDevices.delete(device.id);deviceKey='';await refresh();}};
      }
    }
    txt('div',service==='ecs'?'可切換模擬信號 · 不寫入實際 PLC':'唯讀設備狀態 · 請至 ECS 操作',card,'equipment-mode');
  });
}
function renderInteractions(){
  const operations=(equipment()?.interactions||[]).slice(-30).reverse();el('interaction-count').textContent='最近 '+operations.length+' 筆';
  const key=JSON.stringify([operations,equipment()?.reconciliations,equipment()?.confirmed_results]);if(interactionKey===key)return;interactionKey=key;
  const table=el('interactions');table.replaceChildren();if(!operations.length){const row=txt('tr','',table),cell=txt('td','尚無設備握手紀錄。作業進入取料或放料步驟後會顯示。',row,'empty');cell.colSpan=5;return;}
  operations.forEach(op=>{
    const row=txt('tr','',table);const cell=label=>{const td=txt('td','',row);td.dataset.label=label;return txt('div','',td);};
    const station=cell('設備／動作');txt('strong',(stationNames[op.station]||op.station)+' · '+(op.action==='PICK'?'取料':'放料'),station);txt('span',op.station,station,'mono');
    pill(cell('狀態'),op.status);
    const owner=cell('車輛／作業');txt('div',op.vehicle_id,owner);txt('span',op.job_id,owner,'mono');txt('span',op.command_id,owner,'mono');
    const times=cell('時間點');times.className='interaction-times';[['請求',op.requested_at],['允許',op.allowed_at],['完成',op.completed_at],...(op.canceled_at?[['取消',op.canceled_at]]:[]),...(op.alarm?[['異常',op.alarm.occurred_at]]:[]),...(op.allow_deadline_at?[['允許期限',op.allow_deadline_at]]:[]),...(op.completion_deadline_at?[['完成期限',op.completion_deadline_at]]:[])].forEach(([label,at])=>{const line=txt('div','',times);txt('span',label,line);txt('time',time(at).replace(' +08:00',''),line);});
    const phaseCell=cell('處理說明');txt('div',op.phase,phaseCell);if(op.alarm)txt('div',op.alarm.code+' · '+op.alarm.owner+' · '+time(op.alarm.occurred_at),phaseCell,'equipment-alarm');const confirmed=(equipment()?.confirmed_results||[]).find(r=>r.command_id===op.command_id);if(confirmed)txt('div','人工確認實際已完成（原狀態保留） · '+confirmed.confirmation.operator+' · '+time(confirmed.at)+' · '+confirmed.confirmation.note,phaseCell,'job-recovery-info');const review=(equipment()?.reconciliations||[]).find(r=>r.job_id===op.job_id);if(review)txt('div','已人工對帳 · '+review.confirmation.operator+' · '+time(review.at)+' · '+review.confirmation.note,phaseCell,'job-recovery-info');
  });
}
function render(value){snapshot=value;renderTimeoutPolicy();renderError();renderMetrics();renderAttention();renderJobs();renderDevices();renderInteractions();}
async function refresh(){
  if(loading)return;loading=true;el('refresh').disabled=true;
  try{
    render(await api(service==='wcs'?'/api/dashboard':'/api/equipment'));
    el('service-light').className='online';el('service-light').textContent=(service==='wcs'?'WCS':'ECS')+' 已連線';
    el('equipment-light').hidden=service!=='wcs';el('equipment-light').className=snapshot.equipment&&!snapshot.equipment_error?'online':'offline';el('equipment-light').textContent=snapshot.equipment&&!snapshot.equipment_error?'ECS 已連線':'ECS 未連線';
    el('last-sync').textContent='最近更新 '+time(Date.now()).slice(11,19)+'（台北）';
  }catch(error){if(service==='wcs')el('attention-fresh').textContent='更新中斷：保留上次資料，等待時間與資源狀態尚未更新。';el('service-light').className='offline';el('service-light').textContent='服務連線中斷';el('error').hidden=false;el('error').textContent='無法取得最新資料：'+error.message+'。目前保留上次畫面。';}
  finally{loading=false;el('refresh').disabled=false;}
}
el('job-filter').onchange=renderJobs;el('job-search').oninput=renderJobs;el('device-filter').onchange=renderDevices;el('device-search').oninput=renderDevices;
el('refresh').onclick=()=>{actionError='';refresh();};
async function poll(){await refresh();setTimeout(poll,1000);}poll();

let recoveryJob=null,recoveryConfirmation=null;
function openRecovery(job){
  recoveryJob=job;const form=el('recovery-form');form.reset();recoveryConfirmation=job.recovery?.confirmation||null;
  el('recovery-job').textContent=job.id+' · '+(job.vehicle_id||'未指派車輛');el('recovery-error').hidden=true;
  for(const name of ['operator','note','target','vehicle_stopped','cargo_cleared','equipment_reset','roads_clear','simulation_position']){
    const field=form.elements.namedItem(name);field.disabled=!!recoveryConfirmation;
    if(recoveryConfirmation){if(field.type==='checkbox')field.checked=recoveryConfirmation[name];else field.value=recoveryConfirmation[name]||'';}
  }
  if(!job.vehicle_id)form.elements.namedItem('target').disabled=true;
  el('recovery-submit').disabled=false;el('recovery-submit').textContent=recoveryConfirmation?'用原確認內容重試':'確認處理並開始模擬復歸';el('recovery-dialog').showModal();
}
el('recovery-close').onclick=()=>el('recovery-dialog').close();
el('recovery-form').onsubmit=async event=>{
  event.preventDefault();const form=event.currentTarget;
  const confirmation=recoveryConfirmation||{request_id:crypto.randomUUID(),operator:form.elements.namedItem('operator').value.trim(),note:form.elements.namedItem('note').value.trim(),target:recoveryJob.vehicle_id?form.elements.namedItem('target').value:null,...Object.fromEntries(['vehicle_stopped','cargo_cleared','equipment_reset','roads_clear','simulation_position'].map(name=>[name,form.elements.namedItem(name).checked]))};
  if(!confirmation.operator.trim()||!confirmation.note.trim()||new TextEncoder().encode(confirmation.operator).length>120||new TextEncoder().encode(confirmation.note).length>1000){el('recovery-error').textContent='請填寫處理人及處理說明；處理人最多 120、說明最多 1000 個 UTF-8 位元組。';el('recovery-error').hidden=false;return;}
  recoveryConfirmation=confirmation;el('recovery-submit').disabled=true;
  for(const field of form.elements)if(field.name)field.disabled=true;
  try{await api('/api/jobs/'+encodeURIComponent(recoveryJob.id)+'/recover',confirmation);actionError='';el('recovery-dialog').close();jobKey='';await refresh();}
  catch(error){el('recovery-error').textContent=error.message+'；可用相同確認內容重試。';el('recovery-error').hidden=false;el('recovery-submit').disabled=false;el('recovery-submit').textContent='用原確認內容重試';}
};

let policyInitialized=false;
const policyFields=service==='wcs'?[['source_wait_ms','來源設備等待'],['destination_wait_ms','目的設備等待'],['equipment_response_ms','ECS 結果確認']]:[['allow_wait_ms','等設備允許'],['completion_wait_ms','等交接完成']];
function renderTimeoutPolicy(){
  const policy=snapshot?.policy;if(!policy)return;
  el('timeout-summary').textContent=policyFields.map(([key,label])=>label+' '+policy[key]/1000+' 秒').join(' ／ ');
  el('timeout-intro').textContent=service==='wcs'?'WCS 管整張作業：設備等待逾時或收到 ECS 異常後停止推進，車輛與未確認資源保持保留。':'ECS 管單次交接：允許與完成分別計時；逾時或故障後停止模擬交接，保存異常碼及發生時間。';
  if(policyInitialized)return;policyInitialized=true;
  el('timeout-fields').replaceChildren();for(const [key,label] of policyFields){const node=txt('label',label+'（秒）',el('timeout-fields'));const input=txt('input','',node);input.name=key;input.type='number';input.min=1;input.max=86400;input.step=1;input.required=true;input.value=policy[key]/1000;}
}
el('timeout-form').onsubmit=async event=>{event.preventDefault();const form=event.currentTarget;const policy=Object.fromEntries(policyFields.map(([key])=>[key,Number(form.elements.namedItem(key).value)*1000]));const button=form.querySelector('button');button.disabled=true;try{await api('/api/policy',policy);policyInitialized=false;el('timeout-result').textContent='已保存，僅套用新建立的作業／交接。';actionError='';await refresh();}catch(error){el('timeout-result').textContent=error.message;}finally{button.disabled=false;}};

let continueJob=null,continueConfirmation=null;
function openContinue(job){
  continueJob=job;const f=el('continue-form');f.reset();const last=job.continuations?.at(-1);continueConfirmation=last&&last.state==='BLOCKED'?last.confirmation:null;
  el('continue-job').textContent=job.id+' · '+(job.vehicle_id||'尚未指派車輛');
  el('continue-vehicle').textContent=job.vehicle_id?'原車已停止於原站點，未被移走或換車':'確認尚未保留用車，未執行取放料';
  const handshake=['PICK','DROP'].includes(job.step);
  el('continue-context').textContent=handshake?'停在'+steps[job.step]+'：僅支援確認實際已完成後接到下一步，不重新執行本次取放料。':'停在'+steps[job.step]+'：重新核對設備條件，保留原車及貨物後重新計時。';
  el('continue-cargo').textContent=job.step==='WAIT_SOURCE'?'確認尚未取料，無作業載貨':job.step==='DROP'?'確認已放料到目的站，原車已無料':'確認貨物仍在原車上，未卸除或轉移';
  el('continue-result-row').hidden=!handshake;f.elements.namedItem('result_completed').required=handshake;
  for(const name of ['operator','note','equipment_ready','vehicle_stopped','cargo_confirmed']){const field=f.elements.namedItem(name);field.disabled=!!continueConfirmation;if(continueConfirmation){if(field.type==='checkbox')field.checked=continueConfirmation[name];else field.value=continueConfirmation[name];}}
  f.elements.namedItem('result_completed').checked=!!continueConfirmation?.interaction_result;f.elements.namedItem('result_completed').disabled=!!continueConfirmation;
  el('continue-error').hidden=true;el('continue-submit').disabled=false;el('continue-submit').textContent=continueConfirmation?'用原確認內容重試':'確認並繼續原作業';el('continue-dialog').showModal();
}
el('continue-close').onclick=()=>el('continue-dialog').close();
el('continue-form').onsubmit=async event=>{
  event.preventDefault();const f=event.currentTarget;
  const c=continueConfirmation||{request_id:crypto.randomUUID(),operator:f.elements.namedItem('operator').value.trim(),note:f.elements.namedItem('note').value.trim(),equipment_ready:f.elements.namedItem('equipment_ready').checked,vehicle_stopped:f.elements.namedItem('vehicle_stopped').checked,cargo_confirmed:f.elements.namedItem('cargo_confirmed').checked,interaction_result:['PICK','DROP'].includes(continueJob.step)?'COMPLETED':null};
  if(!c.operator||!c.note||new TextEncoder().encode(c.operator).length>120||new TextEncoder().encode(c.note).length>1000){el('continue-error').textContent='請填寫處理人與說明，文字過長請縮短。';el('continue-error').hidden=false;return;}
  continueConfirmation=c;el('continue-submit').disabled=true;for(const field of f.elements)if(field.name)field.disabled=true;
  try{await api('/api/jobs/'+encodeURIComponent(continueJob.id)+'/continue',c);el('continue-dialog').close();actionError='';jobKey='';await refresh();}
  catch(error){el('continue-error').textContent=error.message;el('continue-error').hidden=false;el('continue-submit').disabled=false;el('continue-submit').textContent='用原確認內容重試';}
};
