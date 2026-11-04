let attentionPolicyReady=false,attentionKey='';
const attentionNames={WAITING:'未達提醒門檻',WARNING:'黃色提醒',CRITICAL:'紅色提醒'};
function ageText(ms){let seconds=Math.floor(ms/1000);const days=Math.floor(seconds/86400);seconds%=86400;const hours=Math.floor(seconds/3600);seconds%=3600;const minutes=Math.floor(seconds/60);return(days?days+' 天 ':'')+(hours?hours+' 時 ':'')+minutes+' 分 '+seconds%60+' 秒';}
function showAttentionJob(id){el('job-search').value=id;el('job-filter').value='all';jobKey='';renderJobs();setPanel('jobs',true);}
function renderAttention(){
  if(service!=='wcs')return;const a=snapshot?.attention;if(!a)return;
  el('attention-count').textContent=a.summary.total?'（'+a.summary.total+'）':'';
  const banner=el('attention-banner');banner.hidden=!a.summary.total;banner.dataset.severity=a.summary.critical?'CRITICAL':a.summary.warning?'WARNING':'WAITING';
  el('attention-banner-text').textContent='待處理／核對中 '+a.summary.total+' 張 · 紅色 '+a.summary.critical+' · 黃色 '+a.summary.warning;
  el('attention-fresh').textContent='查詢時間 '+time(a.checked_at)+(a.rcs_error?' · RCS 無法確認資源：'+a.rcs_error:' · 資源依本次 RCS 查詢結果');
  el('attention-summary').textContent='共 '+a.summary.total+' 張 · 黃色 '+a.summary.warning+' · 紅色 '+a.summary.critical+'；提醒不解除資源，也不發送外部通知。';
  if(!attentionPolicyReady){el('attention-warning-min').value=a.policy.warning_ms/60000;el('attention-critical-min').value=a.policy.critical_ms/60000;attentionPolicyReady=true;}
  const query=el('attention-search').value.trim().toLowerCase(),level=el('attention-level').value;
  const items=a.items.filter(i=>(level==='ALL'||i.severity===level)&&[i.job_id,i.vehicle_id,i.station,i.reason,i.alarm?.code].join(' ').toLowerCase().includes(query));
  el('attention-matches').textContent='符合 '+items.length+' 張，依等待最久排序';
  const key=JSON.stringify(items.map(({waiting_ms,resources,...i})=>({...i,resources:{...resources,checked_at:undefined}})));
  if(key!==attentionKey){attentionKey=key;const list=el('attention-list');list.replaceChildren();
    if(!items.length)empty(list,a.summary.total?'沒有符合篩選條件的作業。':'目前沒有待人工確認的作業。');
    for(const item of items){
      const card=txt('article','',list,'attention-card');card.dataset.attentionJob=item.job_id;card.dataset.severity=item.severity;
      const head=txt('div','',card,'service-job-head'),identity=txt('div','',head);txt('h3',item.from+' → '+item.to,identity);txt('span',item.job_id,identity,'mono');
      const badges=txt('div','',head,'job-badges');txt('strong',attentionNames[item.severity],badges,'attention-level');pill(badges,item.status);
      const age=txt('div','',card,'attention-wait');txt('strong','',age,'attention-age');txt('span','首次待確認 '+time(item.attention_since),age);
      txt('div',(item.alarm?item.alarm.owner+' · '+item.alarm.code+' · ':'')+(item.reason||'請查看作業紀錄確認原因'),card,'job-wait interrupted');
      txt('div','關聯設備：'+(stationNames[item.station]||item.station)+' ／ 原用車：'+(item.vehicle_id||'尚未指派')+' ／ 載貨紀錄：'+(item.loaded?'已取料':'未記錄有貨，仍需核對現場'),card,'job-context');
      const resources=txt('div','',card,'attention-resources');
      if(!item.resources.known){txt('strong','目前保留無法確認，不代表已解除',resources);}
      else{
        const r=item.resources;txt('strong','RCS 目前保留：車輛 '+r.vehicle_locks.length+' 台 · 路徑 '+r.road_reservations.length+' 趟 · 點位 '+r.station_reservations.length+' 筆',resources);
        if(r.vehicle_locks.length)txt('div','用車：'+r.vehicle_locks.map(l=>l.vehicle_id).join('、'),resources);
        for(const road of r.road_reservations)txt('div','道路：'+road.vehicle_id+' · '+road.path.join(' → ')+' · '+road.task_id,resources);
        if(r.station_reservations.length)txt('div','預留點位：'+r.station_reservations.map(l=>l.node+'（'+l.vehicle_id+'）').join('、'),resources);
        txt('small','共用這些車輛／預約的後續任務可能等待；關聯設備不等於已確認被占用。',resources);
      }
      const actions=txt('div','',card,'attention-actions'),job=(snapshot.jobs||[]).find(j=>j.id===item.job_id);
      const view=txt('button','查看作業',actions,'button');view.type='button';view.onclick=()=>showAttentionJob(item.job_id);
      if(job&&item.can_continue){const b=txt('button','確認異常後繼續',actions,'button primary');b.type='button';b.onclick=()=>openContinue(job);}
      if(job&&item.can_recover){const b=txt('button',job.recovery?'重試人工結案':'人工確認與模擬復歸',actions,'button primary');b.type='button';b.onclick=()=>openRecovery(job);}
      if(item.can_cancel){const b=txt('button','取消並進入結案流程',actions,'button danger');b.type='button';b.onclick=async()=>{b.disabled=true;try{await api('/api/jobs/'+encodeURIComponent(item.job_id)+'/cancel',{reason:'操作員從待人工確認清單提出取消並結案'});actionError='';}catch(e){actionError=e.message;}attentionKey='';jobKey='';await refresh();};}
      if(item.status!=='INTERRUPTED')txt('small','已提出處理，系統仍在核對；通過後才移出清單。',actions);
    }
  }
  for(const item of items){const node=[...el('attention-list').querySelectorAll('[data-attention-job]')].find(n=>n.dataset.attentionJob===item.job_id);if(node)node.querySelector('.attention-age').textContent='已等待 '+ageText(item.waiting_ms);}
}
el('attention-search').oninput=()=>renderAttention();el('attention-level').onchange=()=>renderAttention();
el('attention-open').onclick=()=>setPanel('attention',true);
el('attention-policy-form').onsubmit=async event=>{
  event.preventDefault();const warning=Number(el('attention-warning-min').value)*60000,critical=Number(el('attention-critical-min').value)*60000;
  if(!Number.isInteger(warning)||!Number.isInteger(critical)||warning>=critical){el('attention-policy-result').textContent='紅色提醒時間必須大於黃色提醒時間。';return;}
  const button=event.currentTarget.querySelector('button');button.disabled=true;
  try{await api('/api/attention/policy',{warning_ms:warning,critical_ms:critical});attentionPolicyReady=false;el('attention-policy-result').textContent='已保存，立即套用目前清單；不改變作業或資源。';await refresh();}
  catch(e){el('attention-policy-result').textContent=e.message;}finally{button.disabled=false;}
};
