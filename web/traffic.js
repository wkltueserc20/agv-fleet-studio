(() => {
  const get = id => document.getElementById(id);
  let traces = [], selected = null, taskOptions = '', tasks = [];
  let displayPaused = false, latestSnapshot = null, workflowTraces=[];
  get('trace-display-toggle').onclick=()=>{
    displayPaused=!displayPaused;
    get('trace-display-toggle').textContent=displayPaused?'繼續更新':'暫停畫面更新';
    get('trace-display-toggle').setAttribute('aria-pressed',String(displayPaused));
    get('api-traffic').classList.toggle('api-display-paused',displayPaused);
    get('trace-live-badge').textContent=displayPaused?'畫面已暫停':'即時更新中';
    get('trace-display-status').textContent=displayPaused
      ?'畫面停在 '+time(Date.now())+'。可繼續篩選、點選紀錄與閱讀 JSON；背景 API 通訊與 AGV 調度仍持續執行。'
      :'只暫停此區顯示；背景 API 通訊與 AGV 調度會繼續執行。';
    if(!displayPaused&&latestSnapshot) applySnapshot(latestSnapshot);
  };
  const roleLabel = {wcs:'WCS :8215',ecs:'ECS :8216',central:'介接服務 :8210',rcs:'RCS :8211',agv:'AGV :8212',browser:'操作畫面',test:'測試程式','CT-SIM-01':'AGV 01 :8212','CT-SIM-02':'AGV 02 :8213','CT-SIM-03':'AGV 03 :8214'};
  function isRcsQuery(t){return t.source==='rcs'&&t.target==='agv'&&t.method==='GET'&&t.path==='/api/state';}
  function targetLabel(t){return t.target==='agv'?(t.vehicle_id||t.response?.vehicle_id||'AGV')+' :'+t.port:(roleLabel[t.target]||t.target);}
  function openQueries(vehicle='all',trace=null){get('trace-task').value='all';get('trace-type').value='RCS_QUERY';get('trace-vehicle').value=vehicle;selected=trace?.id||traces.find(t=>isRcsQuery(t)&&(vehicle==='all'||traceCar(t)===vehicle))?.id||null;draw();}
  function pollingCards(){
    get('rcs-query-cards').replaceChildren();
    ['CT-SIM-01','CT-SIM-02','CT-SIM-03'].forEach((id,i)=>{const trace=traces.find(t=>isRcsQuery(t)&&traceCar(t)===id);const card=document.createElement('button');card.type='button';card.className='rcs-query-card';card.dataset.vehicle=id;card.setAttribute('aria-pressed',String(trace?.id===selected));card.disabled=!trace;
      const title=document.createElement('strong');title.textContent='RCS → '+id;const endpoint=document.createElement('code');endpoint.textContent='GET :'+(8212+i)+'/api/state';card.append(title,endpoint);
      const info=document.createElement('span');info.textContent=trace?'HTTP '+trace.status+' · '+(trace.response?.phase||'已回應'):'等待第一筆接收端紀錄';const stamp=document.createElement('small');stamp.textContent=trace?'最近收到查詢：'+time(trace.received_at??trace.at):'尚無紀錄；請確認服務連線。';card.append(info,stamp);
      if(trace){const meta=document.createElement('small');meta.textContent='位置 '+(trace.response?.current_node||trace.response?.station||'路段中')+' · 電量 '+(trace.response?.battery??'—')+'% · 點選看 JSON';card.append(meta);card.onclick=()=>{openQueries(id,trace);get('trace-detail').scrollIntoView({block:'nearest'});};}get('rcs-query-cards').append(card);
    });
  }
  get('show-rcs-queries').onclick=()=>openQueries();
  function traceCar(t){return t.vehicle_id||tasks.find(task=>task.id===t.task_id)?.assigned_vehicle||null;}
  function time(ms) {
    if (ms === null || ms === undefined) return '—';
    return new Date(Number(ms) + 8*3600000).toISOString().replace('T',' ').replace('Z',' +08:00');
  }
  function explanation(t) {
    if(t.path.startsWith('/api/jobs/')&&t.path.endsWith('/cancel'))return '提出取消整張作業；HTTP 202 只代表已保存要求。WCS 會協調 RCS／ECS，最終可能為已取消、待人工確認或交接已完成。';
    if(t.path.startsWith('/api/jobs/')&&t.path.endsWith('/recover'))return '內部操作員提出人工確認與模擬復歸；202 表示已保存，WCS 核對 ECS／RCS 後才解除保留，原搬運流程不恢復。';
    if(t.path==='/api/job-recover')return 'WCS 請 RCS 核對停止、最新車況、目標占用與作業保留；直接設定模擬位置後保存收據並解除資源，同一確認不重做。';
    if(t.path==='/api/interactions/reconcile-job')return 'WCS 請 ECS 停止未完成模擬交接並保存人工對帳；保留原本已完成或待確認結果，不改寫取放料紀錄。';
    if(t.path==='/api/policy')return '內部操作員讀取或設定本服務的設備等待期限；只套用新作業／交接，不修改進行中的期限。';
    if(t.path.startsWith('/api/jobs/')&&t.path.endsWith('/continue'))return '內部操作員確認異常後繼續原作業；202 僅表示確認已保存，WCS 會核對原車與設備後接續，不設定位置或清空貨物。';
    if(t.path==='/api/job-continue-check')return 'WCS 請 RCS 核對原車、位置、停止狀態與資源保留；只查驗，不移車、不解除鎖車。';
    if(t.path==='/api/interactions/confirm-result')return 'WCS 請 ECS 保存人工確認交接實際已完成的證據；原逾時／不確定狀態保留，接續不重做本次交接。';
    if(t.path==='/api/attention')return 'WCS 彙整待人工確認與核對中的作業，查證 RCS 原作業資源；離線顯示無法確認，提醒不解除保留。';
    if(t.path==='/api/attention/policy')return '操作員設定工作台黃／紅提醒門檻，立即套用目前清單；不更改作業狀態或釋放資源。';
    if(t.path==='/api/job-cancel')return 'WCS 請 RCS 停止這張作業新增任務、派車與路徑許可；不會直接解除用車或道路預約。';
    if(t.path==='/api/interactions/cancel-job')return 'WCS 請 ECS 停止未完成的模擬交接並阻止新增動作；已完成／待確認結果不回寫為取消。';
    if(t.path==='/api/jobs')return t.target==='central'?'介接服務接收整張作業需求並交給 WCS；不是直接派車。':'WCS 建立或查詢整張作業，協調 RCS 與 ECS。';
    if(t.path.startsWith('/api/job-'))return 'WCS 請 RCS 鎖定／解除整張作業使用的車輛；確保取料及送貨使用同一台車。';
    if(t.path==='/api/interactions')return 'WCS 請 ECS 執行模擬取料／放料握手，以 command_id 去重；回應包含允許與完成狀態。';
    if(t.path==='/api/equipment/sim/signals')return '操作員切換模擬機台信號，非實際 PLC 點位寫入。';
    if(t.path==='/api/tasks'&&t.request?.kind==='MOVE') return t.target==='central'?'介接服務接收點到點移動需求，轉交 RCS 建單；不執行取料或放料。':'RCS 建立 MOVE 點到點移動任務，回傳任務編號；後續再派發給可用 AGV。';
    if(isRcsQuery(t)) return 'RCS 主動查詢 '+(t.vehicle_id||t.response?.vehicle_id||'AGV')+' 的車況；AGV 回傳位置、電量及任務狀態。這筆是 GET 查詢，不是 AGV 主動推送。';
    if(t.path==='/api/roads/recover') return '操作員手動設定模擬車到等待點並解除待確認的道路預約；不是行駛任務或真車控制。';
    if(t.path==='/api/entry') return t.response?.granted?'RCS 確認點位可用並保存預留，允許此車前進。':'車端申請進站；點位占用、預留或任務條件不符時，保持原位等待。';
    if(t.path==='/api/sim/blocking-demo') return '設定三台模擬車的位置，並建立指定 AGV01 執行的 A → B 測試任務。';
    if(t.path==='/api/sim/park'||t.path==='/api/sim/position') return '手動設定模擬位置，不是讓位行駛任務。RCS 會檢查車輛及目標點是否可用。';
    if(t.path==='/api/tasks') return t.target==='central'?'操作畫面把搬運需求交給介接服務，介接服務再轉送 RCS。':'介接服務請 RCS 建立搬運任務；RCS 回傳正式任務編號。';
    if(t.path==='/api/missions'&&t.request?.kind==='MOVE') return 'RCS 派發點到點移動：先前往起點，再沿核准路徑移至終點，不執行取料或放料。';
    if(t.path==='/api/missions') return t.request?.kind==='YIELD'?'RCS 自動派出讓位任務：車端實際移到等待點，parent_task 關聯原任務。':'RCS 把搬運任務與取放料站交給 AGV 執行。';
    if(t.path==='/api/events') return 'AGV 向 RCS 回報任務狀態、進度與目前階段。';
    if(t.path.startsWith('/api/cancel/')) return '取消指定任務；執行中的任務需交由車端處理。';
    if(t.path==='/api/dispatch') return '操作人員切換 RCS 是否繼續派發排隊任務。';
    if(t.path==='/api/control') return '操作人員暫停或恢復模擬 AGV。';
    return '服務讀取目前任務或車輛狀態，供畫面刷新與派車判斷。';
  }
  function describe(t) {
    if(t.path==='/api/entry'&&tasks.find(task=>task.id===t.task_id)?.kind==='MOVE') return ['MOVE 點到點任務：pickup＝先前往移動起點；destination＝移動到終點；均不取放料。','granted＝通行許可；path＝核准路徑；route_token＝路徑預約。HTTP 200 仍可能需要等待。'];
    if(t.path==='/api/entry') return ['task_id＝執行任務；vehicle_id＝申請車號；stage＝pickup 取料站、destination 目的地或 move 讓位移動。','granted＝是否允許前進；path＝核准路徑；route_token＝路徑預約編號；node＝目標點；reason／blockers＝等待原因。HTTP 200 仍可能 granted:false。'];
    if(t.path==='/api/tasks') return ['request_id＝下單去重編號；kind＝TRANSPORT 搬運或 MOVE 點到點移動；from／to＝起終點。MOVE 不執行取放料。','task.id＝RCS 任務單號；timing＝任務時間點（Unix 毫秒）；duplicate＝是否重送。'];
    if(t.path==='/api/missions') return ['task_id＝RCS 任務編號；vehicle_id＝指定車號；kind＝TRANSPORT 搬運、MOVE 點到點移動或 YIELD 讓位；parent_task＝原任務；from／to＝起終點。','accepted＝車端是否接受；duplicate＝是否已收過此任務。'];
    if(t.path==='/api/events') return ['vehicle_id＝回報車號；seq＝順序；status＝狀態；progress＝百分比；phase＝目前步驟。','ok＝RCS 是否接收；ignored＝重複、舊順序或任務已結束而略過。'];
    if(t.method==='GET') return ['GET 沒有 JSON 本文，查詢目標由網址決定。','這是該次查詢的實際回應快照，不是目前即時值。'];
    return ['實際送出的控制參數。','實際回應；error 代表失敗原因。'];
  }
  function addClock(parent,label,value) {
    const item=document.createElement('div'); item.className='api-clock';
    const l=document.createElement('span'); l.textContent=label;
    const v=document.createElement('time'); v.textContent=time(value);
    if(value!==null&&value!==undefined) v.dateTime=new Date(value).toISOString();
    item.append(l,v); parent.append(item);
  }
  function milestone() {
    const requested=get('trace-task').value;
    const current=traces.find(t=>t.id===selected);
    const task=tasks.find(t=>t.id===(requested==='all'?current?.task_id:requested)) || (requested==='all'?tasks[tasks.length-1]:null);
    get('task-times').replaceChildren();
    if(!task) {get('task-times-title').textContent='任務時間點：等待建立任務';return;}
    get('task-times-title').textContent='任務時間點：'+task.id+' ｜ '+(task.assigned_vehicle||'尚未指派／舊版未記錄');
    const t=task.timing||{};
    addClock(get('task-times'),'RCS 建立任務',task.created_at);
    addClock(get('task-times'),'RCS 開始派發',t.dispatched_at);
    addClock(get('task-times'),'RCS 確認車端已接單',t.accepted_at);
    addClock(get('task-times'),'RCS 收到首次進度',t.first_progress_at);
    const end=t.completed_at||t.canceled_at||t.interrupted_at;
    addClock(get('task-times'),t.completed_at?'RCS 收到完成':t.canceled_at?'RCS 記錄取消':t.interrupted_at?'RCS 標記待確認':'任務結束',end);
    addClock(get('task-times'),'RCS 最近更新',task.updated_at);
  }
  function show(t) {
    if(!t) return;
    get('trace-empty').hidden=true;get('trace-detail').hidden=false;
    get('trace-explain').textContent=explanation(t);
    get('trace-meta').textContent=`${roleLabel[t.source]||t.source} → ${targetLabel(t)}\n${t.method} http://127.0.0.1:${t.port}${t.path}\nHTTP ${t.status} · 接收端處理 ${t.duration_ms.toFixed(2)} ms${t.write_error?' · 回應傳送失敗：'+t.write_error:''}`;
    get('trace-times').replaceChildren();
    addClock(get('trace-times'),'① 完整收到請求',t.received_at??t.at);
    addClock(get('trace-times'),'② 回應內容已產生',t.response_ready_at);
    addClock(get('trace-times'),'③ 回應已寫入連線',t.responded_at);
    const desc=describe(t);get('request-desc').textContent=desc[0];get('response-desc').textContent=desc[1];
    get('trace-request').textContent=t.request===null?'（沒有 JSON 本文）':JSON.stringify(t.request,null,2);
    get('trace-response').textContent=JSON.stringify(t.response,null,2);
    get('trace-identity').textContent=`作業：${t.job_id||'不適用'} ｜ 指令：${t.command_id||'不適用'} ｜ 任務：${t.task_id||'此筆沒有任務編號'} ｜ 關聯車輛：${traceCar(t)||'尚未指派／不適用'} ｜ 紀錄 ID：${t.id}`;
  }
  function draw() {
    pollingCards();
    const savedScroll=get('trace-list').scrollTop;
    const task=get('trace-task').value,type=get('trace-type').value,car=get('trace-vehicle').value;
    get('api-traffic').classList.toggle('query-view',type==='RCS_QUERY');
    const visible=traces.filter(t=>(type==='ALL'||(type==='RCS_QUERY'?isRcsQuery(t):t.method===type))&&(task==='all'||t.task_id===task)&&(car==='all'||traceCar(t)===car));
    get('trace-count').textContent=`符合 ${visible.length} 筆，顯示最新 60 筆`;
    get('trace-list').replaceChildren();
    if(!visible.length) {
      const empty=document.createElement('div');empty.className='empty';empty.textContent=get('trace-task').value!=='all'&&['GET','RCS_QUERY'].includes(get('trace-type').value)?'車況查詢通常不帶任務編號。請選全部任務，或按「看 RCS 查詢車況」。':'尚無符合的傳輸紀錄；可按上方「看 RCS 查詢車況」。';
      get('trace-list').append(empty);get('trace-empty').hidden=false;get('trace-detail').hidden=true;milestone();return;
    }
    if(!visible.some(t=>t.id===selected)) selected=(visible.find(t=>t.path==='/api/tasks'&&t.target==='rcs')||visible[0]).id;
    visible.slice(0,60).forEach(t=>{
      const b=document.createElement('button');b.type='button';b.className='api-entry';b.setAttribute('aria-pressed',String(t.id===selected));
      const title=document.createElement('strong');title.textContent=(roleLabel[t.source]||t.source)+' → '+targetLabel(t);
      const path=document.createElement('small');path.textContent=t.method+' '+t.path;
      const info=document.createElement('div');info.className='trace-time';
      const stamp=document.createElement('time');stamp.textContent=time(t.received_at??t.at);stamp.dateTime=new Date(t.received_at??t.at).toISOString();
      const status=document.createElement('span');status.textContent='HTTP '+t.status;
      info.append(stamp,status);b.append(title,path,info);b.onclick=()=>{selected=t.id;draw();};get('trace-list').append(b);
    });
    show(visible.find(t=>t.id===selected));milestone();get('trace-list').scrollTop=savedScroll;
  }
  function applySnapshot(snapshot){
    traces=[...new Map([...(snapshot.traffic||[]),...workflowTraces].map(t=>[t.id,t])).values()].sort((a,b)=>b.at-a.at);tasks=snapshot.rcs?.tasks||[];
    const ids=[...new Set([...tasks].reverse().map(t=>t.id).concat(traces.map(t=>t.task_id).filter(Boolean)))];
    const fingerprint=ids.join('|');
    if(taskOptions!==fingerprint) {
      const previous=get('trace-task').value;get('trace-task').replaceChildren();
      const all=document.createElement('option');all.value='all';all.textContent='全部任務';get('trace-task').append(all);
      ids.forEach(id=>{const o=document.createElement('option');o.value=id;o.textContent=id;get('trace-task').append(o);});
      get('trace-task').value=ids.includes(previous)?previous:'all';taskOptions=fingerprint;
    }
    draw();
  }
  document.addEventListener('workflow-snapshot',event=>{workflowTraces=event.detail.traffic||[];if(!displayPaused&&latestSnapshot)applySnapshot(latestSnapshot);});
  document.addEventListener('rcs-snapshot',event=>{
    latestSnapshot=event.detail;
    if(!displayPaused) applySnapshot(latestSnapshot);
  });
  document.querySelectorAll('[data-hop]').forEach(b=>b.onclick=()=>{
    const task=get('trace-task').value,car=get('trace-vehicle').value;
    const match=traces.find(t=>(task==='all'||t.task_id===task)&&(car==='all'||traceCar(t)===car)&&(b.dataset.hop==='create'?((t.path==='/api/tasks'&&t.target==='rcs')||(t.path==='/api/jobs'&&t.target==='wcs')):b.dataset.hop==='mission'?t.path==='/api/missions':b.dataset.hop==='equipment'?t.path==='/api/interactions':t.path==='/api/events'&&(t.source==='agv'||t.source.startsWith('CT-SIM-'))));
    if(match){get('trace-type').value='POST';selected=match.id;draw();}
  });
  get('trace-task').onchange=()=>{selected=null;draw();};get('trace-type').onchange=()=>{selected=null;draw();};get('trace-vehicle').onchange=()=>{selected=null;draw();};
})();
