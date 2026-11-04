let trackedTask=null,consoleTasksKey='';
function setComposer(open){document.body.classList.toggle('composer-open',open);$('compose-backdrop').hidden=!open;$('mobile-order-toggle').setAttribute('aria-expanded',String(open));}
$('mobile-order-toggle').onclick=()=>setComposer(!document.body.classList.contains('composer-open'));$('mobile-order-close').onclick=()=>setComposer(false);$('compose-backdrop').onclick=()=>setComposer(false);document.addEventListener('keydown',e=>{if(e.key==='Escape')setComposer(false);});
function setView(view,updateHash=false){
  if(!['overview','stations','api','history','notes'].includes(view))view='overview';
  setComposer(false);$('mobile-order-toggle').hidden=view!=='overview'||role==='agv';
  document.querySelectorAll('[role="tabpanel"]').forEach(p=>p.hidden=p.id!=='view-'+view);
  document.querySelectorAll('[data-view]').forEach(b=>b.setAttribute('aria-selected',String(b.dataset.view===view)));
  if(updateHash){history.replaceState(null,'','#'+({overview:'fleet-overview',stations:'station-control',api:'api-traffic',history:'tasks',notes:'project-notes'}[view]));window.scrollTo({top:0,behavior:'instant'});}
}
function viewForHash(){const h=location.hash;setView(/notes|todo/.test(h)?'notes':/api-/.test(h)?'api':/station|auto-yield/.test(h)?'stations':/tasks|events/.test(h)?'history':'overview');}
document.querySelectorAll('[data-view]').forEach(b=>b.onclick=()=>setView(b.dataset.view,true));
window.addEventListener('hashchange',viewForHash);viewForHash();
function trackTask(id){setComposer(false);trackedTask=id;if(snapshot?.rcs?.tasks?.some(t=>t.id===id)){renderConsole(snapshot);renderRoads(snapshot);}}
$('flow-task').onchange=()=>trackTask($('flow-task').value);
function shortTime(ms){return ms?labTime(ms).slice(11,23):'尚未發生';}
function renderConsole(s){
  const tasks=s.rcs?.tasks||[],ordered=[...tasks].reverse();
  if(!trackedTask)trackedTask=(ordered.find(t=>t.kind!=='YIELD'&&!['COMPLETED','CANCELED','FAILED','INTERRUPTED'].includes(t.status))||ordered.find(t=>t.kind!=='YIELD')||ordered[0])?.id||null;
  const key=ordered.map(t=>t.id).join('|');
  if(key!==consoleTasksKey){consoleTasksKey=key;$('flow-task').replaceChildren();ordered.forEach(t=>{const o=document.createElement('option');o.value=t.id;o.textContent=taskKindLabel(t.kind)+' '+t.from+' → '+t.to+' · '+t.id;$('flow-task').append(o);});}
  $('flow-task').value=trackedTask||'';
  const t=tasks.find(t=>t.id===trackedTask);
  $('flow-route').textContent=t?(taskKindLabel(t.kind)+'：')+(names[t.from]||t.from)+' → '+(names[t.to]||t.to):'尚未建立任務';
  $('flow-status').textContent=t?(t.wait_reason?'等待進站':labels[t.status]||t.status):'等待下單';
  $('flow-status').className='badge '+(t?.status||'');
  $('flow-car').textContent=t?.assigned_vehicle?'指派給 '+t.assigned_vehicle:t?.requested_vehicle?'等待指派 '+t.requested_vehicle:'尚未指派車輛';
  $('flow-id').textContent=t?t.id+' · '+labTime(t.created_at).slice(0,10):'';
  const steps=[['① RCS 已建單',t?.created_at],['② RCS 已派發',t?.timing?.dispatched_at],['③ 車端已接單',t?.timing?.accepted_at],['④ 執行完成',t?.timing?.completed_at]];
  const firstMissing=steps.findIndex(step=>!step[1]);$('flow-steps').replaceChildren();
  steps.forEach(([label,at],i)=>{const step=document.createElement('div');step.className='flow-step'+(at?' done':t&&i===firstMissing&&!['CANCELED','FAILED','INTERRUPTED'].includes(t.status)?' current':'');const name=document.createElement('b');name.textContent=label;const time=document.createElement('small');time.textContent=at?shortTime(at):t?'等待中':'—';step.append(name,time);$('flow-steps').append(step);});
  let note='提出任務需求後，RCS 建立任務，再選車派發；這裡顯示實際回報時間。';
  if(t){
    if(t.wait_reason)note=t.wait_reason;
    else if(t.status==='QUEUED')note=s.rcs?.auto_dispatch?'RCS 已保存任務，等待可用車輛'+(t.requested_vehicle?' '+t.requested_vehicle:'')+'。':'RCS 已保存任務；目前暫停新任務派發，尚未交給車端。';
    else if(t.status==='DISPATCHING')note='RCS 正在把任務交給 '+t.assigned_vehicle+'，等待車端確認接單。';
    else if(t.status==='RUNNING')note=(t.assigned_vehicle||'車端')+' 已接受任務 · '+t.phase+' · '+t.progress+'%';
    else if(t.status==='COMPLETED')note='任務已完成；RCS 已收到車端完成回報。';
    else note=(labels[t.status]||t.status)+' · '+(t.note||t.phase);
    if(t.parent_task)note+=' 原任務：'+t.parent_task;
  }
  $('flow-note').textContent=note;$('flow-note').className='flow-note'+(t?.wait_reason||['CANCELED','FAILED','INTERRUPTED'].includes(t?.status)?' warning':'');
  $('current-jobs').replaceChildren();
  const active=ordered.filter(t=>!['COMPLETED','CANCELED','FAILED','INTERRUPTED'].includes(t.status));
  const recent=[...active,...ordered.filter(t=>!active.includes(t))].slice(0,6);
  recent.forEach(task=>{const b=document.createElement('button');b.type='button';b.className='job-card'+(trackedTask===task.id?' selected':'');b.dataset.task=task.id;b.setAttribute('aria-pressed',String(trackedTask===task.id));const route=document.createElement('strong');route.textContent=(taskKindLabel(task.kind)+' · ')+task.from+' → '+task.to;const status=document.createElement('span');status.textContent=(task.assigned_vehicle||'尚未派車')+' · '+(task.wait_reason?'等待進站':labels[task.status]||task.status)+' · '+task.progress+'%';const id=document.createElement('small');id.textContent=task.id;b.append(route,status,id);b.onclick=()=>{trackTask(task.id);$('dispatch-flow').scrollIntoView({block:'nearest'});};$('current-jobs').append(b);});
  if(!recent.length){const p=document.createElement('p');p.className='sub';p.textContent=s.rcs?'還沒有任務，從上方提出第一張搬運需求。':'等待 RCS 連線。';$('current-jobs').append(p);}
}
