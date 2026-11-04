function renderStations(s){
  const stations=s.rcs?.stations?.stations||[],tasks=s.rcs?.tasks||[];
  const waiting=tasks.filter(t=>t.wait_reason&&!['COMPLETED','CANCELED','FAILED','INTERRUPTED'].includes(t.status));
  $('station-wait-count').textContent=s.rcs?waiting.length+' 台等待進站':'RCS 未連線';
  $('station-grid').replaceChildren();
  stations.forEach(st=>{
    const occupied=st.occupants||[],reserved=st.reservations||[];
    const card=document.createElement('article');card.className='station-slot'+(occupied.length?' occupied':'')+(reserved.length?' reserved':'');card.dataset.station=st.node;
    const heading=document.createElement('strong');heading.textContent=names[st.node]||st.node;card.append(heading);
    const line=document.createElement('p');line.textContent=occupied.length?'占用：'+occupied.map(v=>v.vehicle_id+(v.online?'':'（離線，最後位置）')).join('、'):'目前無車占用';card.append(line);
    const lease=document.createElement('p');lease.textContent=reserved.length?'預留：'+reserved.map(l=>l.vehicle_id+' / '+l.task_id).join('、'):'未預留';card.append(lease);$('station-grid').append(card);
    const marker=document.querySelector('[data-node="'+st.node+'"]');
    if(marker){marker.classList.toggle('occupied',occupied.length>0);marker.classList.toggle('reserved',reserved.length>0);let detail=marker.querySelector('small');if(!detail){detail=document.createElement('small');marker.append(detail);}detail.textContent=occupied.length?'占用 '+occupied.map(v=>v.vehicle_id.replace('CT-SIM-','')).join('/'):(reserved.length?'預留 '+reserved.map(l=>l.vehicle_id.replace('CT-SIM-','')).join('/'):'空閒');}
  });
  $('station-waits').replaceChildren();
  waiting.forEach(t=>{const row=document.createElement('div');row.className='station-wait';const reason=document.createElement('strong');reason.textContent=t.assigned_vehicle+'：'+t.wait_reason;const meta=document.createElement('small');meta.textContent=t.id+' · '+t.from+' → '+t.to+' · 開始等待 '+labTime(t.wait_since);row.append(reason,meta);$('station-waits').append(row);});
  if(!waiting.length){const p=document.createElement('p');p.className='sub';p.textContent=s.rcs?'目前沒有車輛等待進站。':'等待 RCS 資料。';$('station-waits').append(p);}

  $('yield-tasks').replaceChildren();
  const children=tasks.filter(t=>t.kind==='YIELD').slice(-8).reverse();
  children.forEach(t=>{const row=document.createElement('div');row.className='station-wait';const heading=document.createElement('strong');heading.textContent=(t.assigned_vehicle||t.requested_vehicle)+'：'+t.from+' → '+t.to+' · '+(labels[t.status]||t.status);const info=document.createElement('small');info.textContent='讓位 '+t.id+' ／ 原搬運 '+t.parent_task+' ／ '+t.progress+'%';row.append(heading,info);$('yield-tasks').append(row);});
  if(!children.length){const p=document.createElement('p');p.className='sub';p.textContent='目前沒有讓位任務；發生點位占用時，RCS 會自動判斷。';$('yield-tasks').append(p);}
}
