//! Operator reconciliation closes canceled work; it never resumes old missions.
use super::*;

#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Confirmation {
    pub request_id: String,
    pub operator: String,
    pub note: String,
    pub target: Option<String>,
    pub vehicle_stopped: bool,
    pub cargo_cleared: bool,
    pub equipment_reset: bool,
    pub roads_clear: bool,
    pub simulation_position: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Recovery {
    pub confirmation: Confirmation,
    pub state: String,
    pub requested_at: u64,
    pub deadline_at: u64,
    pub resolved_at: Option<u64>,
    pub evidence: Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub job_id: String,
    pub vehicle_id: Option<String>,
    pub confirmation: Confirmation,
    pub at: u64,
}
fn valid(c: &Confirmation) -> bool {
    !c.request_id.trim().is_empty() && c.request_id.len()<=100
        && !c.operator.trim().is_empty() && c.operator.len()<=120
        && !c.note.trim().is_empty() && c.note.len()<=1000
        && c.vehicle_stopped && c.cargo_cleared && c.equipment_reset && c.roads_clear
        && c.simulation_position
        && c.target.as_deref().is_none_or(|t|matches!(t,"P1"|"P2"|"P3"|"I1"|"I2"))
}
pub(super) fn request(shared: &Shared<Jobs>, r: &Request) -> Response {
    let Some(id)=r.path.strip_prefix("/api/jobs/").and_then(|s|s.strip_suffix("/recover"))else{return error(404,"找不到作業");};
    let c:Confirmation=match parse(r){Ok(v)=>v,Err(e)=>return e};
    if !valid(&c){return error(400,"需填處理人、說明、請求編號，並確認停止、貨物處理、設備復位、道路清空及模擬位置設定；位置限 P1/P2/P3/I1/I2");}
    let mut s=shared.lock().unwrap();
    if s.storage_error.is_some(){return error(503,"WCS 儲存異常");}
    if s.jobs.iter().any(|j|j.id!=id&&j.recovery.as_ref().is_some_and(|v|v.confirmation.request_id==c.request_id)){return error(409,"復歸請求編號已用於其他作業");}
    let Some(job)=s.jobs.iter_mut().find(|j|j.id==id)else{return error(404,"找不到作業");};
    if let Some(existing)=&job.recovery {
        if existing.confirmation!=c{return error(409,"復歸已保存；重試須使用相同請求編號與確認內容，避免重複設定已解除的車輛");}
        if existing.state=="DONE"{return ok(json!({"job":job,"duplicate":true}));}
        if job.status=="RECOVERING"{let mut out=ok(json!({"job":job,"duplicate":true}));out.code=202;return out;}
    }
    if job.status!="INTERRUPTED"||job.cancel_requested_at.is_none(){return error(409,"僅能復歸已取消且待人工確認的作業；重啟中斷的作業請先取消");}
    if job.vehicle_id.is_some()&&c.target.is_none(){return error(400,"有作業用車時必須選擇模擬復歸點");}
    job.recovery=Some(Recovery{confirmation:c,state:"REQUESTED".into(),requested_at:now(),deadline_at:now()+60_000,resolved_at:None,evidence:Value::Null});
    job.status="RECOVERING".into();job.paused=false;job.updated_at=now();
    job.wait_reason=Some("人工確認已保存，正在核對設備與車況，尚未解除保留".into());
    job.events.push(json!({"kind":"RECOVERY_REQUESTED","at":now(),"step":job.step,"message":"人工確認已保存，開始核對設備與車況；尚未解除保留","confirmation":job.recovery.as_ref().unwrap().confirmation}));
    let result=job.clone();save_jobs(&mut s);
    if s.storage_error.is_some(){return error(503,"確認紀錄保存失敗，請以原請求重試");}
    let mut out=ok(json!({"job":result,"accepted":true}));out.code=202;out
}
pub(super) fn advance(job: &mut Job) {
    let recovery=job.recovery.as_ref().unwrap();
    let body=json!({"job_id":job.id,"vehicle_id":job.vehicle_id,"confirmation":recovery.confirmation});
    let result=(||->Result<Value,String>{
        http(RCS,"POST","/api/job-cancel",json!({"job_id":job.id}))?;
        let ecs=http(ECS,"POST","/api/interactions/reconcile-job",body.clone())?;
        let rcs=http(RCS,"POST","/api/job-recover",body)?;
        Ok(json!({"ecs":ecs,"rcs":rcs}))
    })();
    job.updated_at=now();
    let recovery=job.recovery.as_mut().unwrap();
    match result {
        Ok(evidence)=>{
            recovery.state="DONE".into();recovery.resolved_at=Some(now());recovery.evidence=evidence;
            job.status="CANCELED".into();job.loaded=false;job.cancel_requires_review=false;
            job.cancel_phase=Some("MANUALLY_RESOLVED".into());job.canceled_at=Some(now());job.wait_reason=None;
            job.events.push(json!({"kind":"RECOVERY_DONE","at":now(),"step":job.step,"message":"人工對帳與模擬復歸完成，貨物已人工處理，車輛與道路保留已解除；原交接紀錄保留，舊作業不再執行"}));
        },
        Err(reason)=>{
            recovery.evidence=json!({"checked_at":now(),"error":reason});
            job.wait_reason=Some(format!("人工復歸尚未完成：{reason}"));
            if now()>=recovery.deadline_at {
                recovery.state="BLOCKED".into();job.status="INTERRUPTED".into();
                job.events.push(json!({"at":now(),"step":job.step,"message":"人工復歸確認逾時；保留未解除資源，可用原確認內容重試"}));
            } else {recovery.state="VERIFYING".into();}
        }
    }
}
#[derive(Deserialize)]
struct Internal {job_id:String,vehicle_id:Option<String>,confirmation:Confirmation}
fn internal(r:&Request)->Result<Internal,Response>{
    if r.source!="wcs"{return Err(error(403,"僅供 WCS 人工復歸協調（本機標頭非正式認證）"));}
    let v:Internal=parse(r)?;
    if v.job_id.is_empty()||v.job_id.len()>120||!valid(&v.confirmation){return Err(error(400,"無效人工確認內容"));}
    Ok(v)
}
pub(super) fn equipment(s:&mut Equipment,r:&Request)->Response{
    let v=match internal(r){Ok(v)=>v,Err(e)=>return e};
    if let Some(receipt)=s.reconciliations.iter().find(|x|x.job_id==v.job_id){
        return if receipt.confirmation==v.confirmation&&receipt.vehicle_id==v.vehicle_id{ok(json!({"receipt":receipt,"duplicate":true}))}else{error(409,"設備已有不同人工確認紀錄")};
    }
    // Fence pending simulator work before recording the operator's separate evidence.
    let stop=Request{method:"POST".into(),path:"/api/interactions/cancel-job".into(),body:serde_json::to_vec(&json!({"job_id":v.job_id})).unwrap(),source:"wcs".into()};
    let response=cancel::equipment(s,&stop);if response.code!=200{return response;}
    if s.interactions.iter().any(|op|op.job_id==v.job_id&&matches!(op.status.as_str(),"REQUESTED"|"ALLOWED")){return error(409,"設備交接仍在執行");}
    let receipt=Receipt{job_id:v.job_id,vehicle_id:v.vehicle_id,confirmation:v.confirmation,at:now()};
    s.reconciliations.push(receipt.clone());save_equipment(s);
    if s.storage_error.is_some(){return error(503,"設備人工確認尚未保存");}
    ok(json!({"receipt":receipt,"simulation":true}))
}
pub fn rcs(shared:&Shared<RcsState>,r:&Request)->Response{
    let v=match internal(r){Ok(v)=>v,Err(e)=>return e};
    let mut s=shared.lock().unwrap();
    if s.storage_error.is_some(){return error(503,"RCS 儲存異常");}
    // Durable receipt must be checked before touching a car that may now belong to another job.
    if let Some(receipt)=s.job_recoveries.iter().find(|x|x.job_id==v.job_id){
        return if receipt.confirmation==v.confirmation&&receipt.vehicle_id==v.vehicle_id{ok(json!({"receipt":receipt,"duplicate":true}))}else{error(409,"作業已有不同復歸紀錄")};
    }
    if !canceled(&s,&v.job_id){return error(409,"作業尚未建立取消屏障");}
    let lock=s.job_locks.iter().find(|l|l.job_id==v.job_id).cloned();
    if lock.as_ref().map(|l|l.vehicle_id.as_str())!=v.vehicle_id.as_deref(){return error(409,"作業車號與保留不符；不操作可能屬於其他作業的車輛");}
    let ids:Vec<String>=s.tasks.iter().filter(|t|t.job_id.as_deref()==Some(&v.job_id)).map(|t|t.id.clone()).collect();
    let related=|t:&Task|t.job_id.as_deref()==Some(&v.job_id)||t.parent_task.as_ref().is_some_and(|id|ids.contains(id));
    if s.tasks.iter().any(|t|related(t)&&!terminal(&t.status)){return error(409,"作業或關聯讓位任務尚未停止");}
    if let Some(id)=&v.vehicle_id {
        let Some(target)=v.confirmation.target.as_deref()else{return error(400,"缺少復歸點");};
        let fleet=fleet_states();s.traffic.refresh(&fleet);
        let Some(car)=fleet.iter().find(|c|c["vehicle_id"]==*id&&c["online"]==true)else{return error(409,"車輛未連線，不能復歸");};
        if !car["active"].is_null()||car["pending_callbacks"]!=0||now().saturating_sub(car["sample_at"].as_u64().unwrap_or(0))>5000
            ||s.tasks.iter().any(|t|t.assigned_vehicle.as_deref()==Some(id)&&!terminal(&t.status)) {return error(409,"車輛仍在執行、回報未送完或車況過期");}
        if s.roads.leases.iter().any(|l|l.vehicle_id==*id&&!ids.contains(&l.task_id))||s.traffic.leases.iter().any(|l|l.vehicle_id==*id&&!ids.contains(&l.task_id)){return error(409,"該車存在其他作業的預約，不跨作業清除");}
        let mut blockers=s.traffic.blockers(target,id);blockers.extend(s.roads.conflict(&[target.into()],id));
        if !blockers.is_empty(){return error(409,&format!("復歸點被占用或預約：{}",blockers.join("；")));}
        let port=vehicle_port(id).unwrap();
        let position=match http(port,"POST","/api/sim/position",json!({"node":target})){Ok(p)=>p,Err(e)=>return error(502,&format!("模擬位置未確認，保留資源：{e}"))};
        if position["current_node"]!=target||!position["road_edge"].is_null()||!position["active"].is_null(){return error(409,"車端未確認復歸位置");}
        s.roads.leases.retain(|l|l.vehicle_id!=*id);
        s.traffic.leases.retain(|l|l.vehicle_id!=*id);
        for task in s.tasks.iter_mut().filter(|t|ids.contains(&t.id)){for route in &mut task.routes{route.released_at.get_or_insert(now());}}
        s.traffic.refresh(&fleet_states());
    }
    let receipt=Receipt{job_id:v.job_id.clone(),vehicle_id:v.vehicle_id,confirmation:v.confirmation,at:now()};
    s.job_locks.retain(|l|l.job_id!=v.job_id);
    if let Some(f)=s.job_cancellations.iter_mut().find(|f|f.job_id==v.job_id){f.released_at.get_or_insert(now());}
    s.job_recoveries.push(receipt.clone());
    log_event(&mut s,"SYSTEM",&format!("人工復歸 {}：{}；原任務結果保留，模擬車位置已確認，作業保留解除",v.job_id,receipt.confirmation.operator));
    save(&mut s);
    if s.storage_error.is_some(){return error(503,"復歸紀錄尚未持久保存，停止放行");}
    ok(json!({"receipt":receipt,"simulation":true}))
}
