//! Persisted wall-clock budgets. Faults latch until cancellation and reconciliation.
use super::*;
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WcsPolicy {pub source_wait_ms:u64,pub destination_wait_ms:u64,pub equipment_response_ms:u64}
impl Default for WcsPolicy {fn default()->Self{Self{source_wait_ms:300_000,destination_wait_ms:300_000,equipment_response_ms:120_000}}}
#[derive(Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EcsPolicy {pub allow_wait_ms:u64,pub completion_wait_ms:u64}
impl Default for EcsPolicy {fn default()->Self{Self{allow_wait_ms:30_000,completion_wait_ms:60_000}}}
#[derive(Clone, Serialize, Deserialize)]
pub struct Alarm {pub code:String,pub owner:String,pub station:String,pub command_id:Option<String>,pub occurred_at:u64,pub message:String}
pub(super) fn alarm(code:&str,owner:&str,station:&str,command:Option<String>,message:&str)->Alarm{
    Alarm{code:code.into(),owner:owner.into(),station:station.into(),command_id:command,occurred_at:now(),message:message.into()}
}
pub(super) fn raise(job:&mut Job, issue:Alarm){
    if job.alarm.is_some(){return;}
    let message=format!("{}：{}；停止後續流程，保留需確認資源，請確認後接續，或取消並人工結案",issue.code,issue.message);
    job.events.push(json!({"kind":"ALARM","at":issue.occurred_at,"step":job.step,"alarm":issue,"message":message}));
    job.alarm=Some(issue);job.status="INTERRUPTED".into();job.wait_reason=Some(message);job.updated_at=now();
}
pub(super) fn enter(job:&mut Job,step:&str){
    let duration=match step{"WAIT_SOURCE"=>job.timeout_policy.source_wait_ms,"WAIT_DEST"=>job.timeout_policy.destination_wait_ms,"PICK"|"DROP"=>job.timeout_policy.equipment_response_ms,_=>0};
    job.wait_started_at=(duration>0).then(now);job.wait_deadline_at=job.wait_started_at.map(|t|t.saturating_add(duration));
}
pub(super) fn monitor(job:&mut Job){
    if !matches!(job.step.as_str(),"WAIT_SOURCE"|"WAIT_DEST"|"PICK"|"DROP"){return;}
    if job.wait_deadline_at.is_none(){let step=job.step.clone();enter(job,&step);}
    let source=matches!(job.step.as_str(),"WAIT_SOURCE"|"PICK");
    let station=if source{job.from.clone()}else{job.to.clone()};
    let handshaking=matches!(job.step.as_str(),"PICK"|"DROP");
    let command=handshaking.then(||format!("{}-{}",job.id,job.step));
    if let Ok(state)=http(ECS,"GET","/api/equipment",Value::Null) {
        if state["storage_error"].is_null(){
            if handshaking {
                if let Some(op)=state["interactions"].as_array().and_then(|ops|ops.iter().find(|op|op["command_id"].as_str()==command.as_deref())) {
                    if op["status"]=="COMPLETED"{return;}
                    if matches!(op["status"].as_str(),Some("TIMED_OUT"|"FAILED"|"INTERRUPTED"|"CANCELED")){
                        let issue=serde_json::from_value::<Alarm>(op["alarm"].clone()).unwrap_or_else(|_|alarm("ECS_RESULT_UNCERTAIN","ECS",&station,command.clone(),"設備交接已停止或結果待確認"));
                        raise(job,issue);return;
                    }
                }
            }
            if state["devices"].as_array().is_some_and(|ds|ds.iter().any(|d|d["id"]==station&&d["fault"]==true)){
                raise(job,alarm("EQUIPMENT_FAULT","ECS",&station,command,"設備故障信號已啟動"));return;
            }
        }
    }
    if now()>=job.wait_deadline_at.unwrap_or(u64::MAX){
        let (code,message)=match job.step.as_str(){"WAIT_SOURCE"=>("SOURCE_WAIT_TIMEOUT","等待來源有料／設備可用逾時"),"WAIT_DEST"=>("DESTINATION_WAIT_TIMEOUT","等待目的設備可收料逾時"),_=>("ECS_RESPONSE_TIMEOUT","未能在期限內取得設備交接完成結果，可能含通訊中斷")};
        raise(job,alarm(code,"WCS",&station,command,message));
    }
}
pub(super) fn fail_interaction(op:&mut Interaction,code:&str,message:&str,status:&str){
    op.alarm=Some(alarm(code,"ECS",&op.station,Some(op.command_id.clone()),message));
    op.status=status.into();op.phase=message.into();
}
pub(super) fn ecs_tick(s:&mut Equipment)->bool{
    let mut changed=false;
    for i in 0..s.interactions.len(){
        let op=s.interactions[i].clone();
        if !matches!(op.status.as_str(),"REQUESTED"|"ALLOWED"){continue;}
        let Some(d)=s.devices.iter().find(|d|d.id==op.station).cloned()else{continue;};
        if d.fault {fail_interaction(&mut s.interactions[i],"EQUIPMENT_FAULT","交接期間設備故障；結果待人工確認","FAILED");changed=true;continue;}
        let deadline=if op.status=="REQUESTED"{op.allow_deadline_at}else{op.completion_deadline_at};
        if deadline.is_some_and(|at|now()>=at){
            let (code,message)=if op.status=="REQUESTED"{("ECS_ALLOW_TIMEOUT","等待設備允許信號逾時") }else{("ECS_COMPLETION_TIMEOUT","設備已允許，但等待交接完成／復位逾時")};
            fail_interaction(&mut s.interactions[i],code,message,"TIMED_OUT");changed=true;continue;
        }
        let ready=if op.action=="PICK"{d.material_ready}else{d.can_receive};
        if !ready{continue;}
        if op.status=="REQUESTED"{
            s.interactions[i].status="ALLOWED".into();s.interactions[i].allowed_at=Some(now());
            s.interactions[i].completion_deadline_at=Some(now().saturating_add(op.timeout_policy.completion_wait_ms));
            s.interactions[i].phase="設備已允許，等待模擬交接完成與信號復位".into();changed=true;
        }else if d.auto_complete&&now().saturating_sub(op.allowed_at.unwrap_or(now()))>=1200{
            s.interactions[i].status="COMPLETED".into();s.interactions[i].completed_at=Some(now());
            s.interactions[i].phase="模擬交接完成，通知與完成信號已復位（非實際 PLC）".into();
            s.devices.iter_mut().find(|d|d.id==op.station).unwrap().material_ready=op.action=="DROP";changed=true;
        }
    }
    changed
}
fn within(v:u64)->bool{(1000..=86_400_000).contains(&v)}
pub(super) fn wcs_policy(s:&Shared<Jobs>,r:&Request)->Response{
    let mut s=s.lock().unwrap();
    if r.method=="POST"{
        if s.storage_error.is_some(){return error(503,"WCS 儲存異常");}
        let p:WcsPolicy=match parse(r){Ok(v)=>v,Err(e)=>return e};
        if ![p.source_wait_ms,p.destination_wait_ms,p.equipment_response_ms].into_iter().all(within){return error(400,"每項期限必須為 1000～86400000 毫秒");}
        s.policy=p;save_jobs(&mut s);if s.storage_error.is_some(){return error(503,"期限設定保存失敗");}
    }
    ok(json!({"owner":"WCS","policy":s.policy,"applies_to":"new_jobs","clock":"wall_clock_including_pause"}))
}
pub(super) fn ecs_policy(s:&mut Equipment,r:&Request)->Response{
    if r.method=="POST"{
        let p:EcsPolicy=match parse(r){Ok(v)=>v,Err(e)=>return e};
        if ![p.allow_wait_ms,p.completion_wait_ms].into_iter().all(within){return error(400,"每項期限必須為 1000～86400000 毫秒");}
        s.policy=p;save_equipment(s);if s.storage_error.is_some(){return error(503,"期限設定保存失敗");}
    }
    ok(json!({"owner":"ECS","policy":s.policy,"applies_to":"new_interactions","clock":"wall_clock_including_pause"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn equipment(status:&str,deadline:u64)->Equipment{
        serde_json::from_value(json!({"devices":[{"id":"A","material_ready":true,"can_receive":true,"fault":false,"auto_complete":true}],"interactions":[{"command_id":"test-PICK","job_id":"test","vehicle_id":"CT-SIM-01","station":"A","action":"PICK","status":status,"requested_at":1,"allowed_at":1,"completed_at":null,"phase":"test","allow_deadline_at":deadline,"completion_deadline_at":deadline}]})).unwrap()
    }
    #[test]
    fn deadline_wins_over_late_ready_or_completion_and_cannot_resurrect(){
        for status in ["REQUESTED","ALLOWED"]{
            let mut s=equipment(status,now()-1);assert!(ecs_tick(&mut s));
            assert_eq!(s.interactions[0].status,"TIMED_OUT");assert!(s.interactions[0].completed_at.is_none());
            assert!(!ecs_tick(&mut s));assert!(s.devices[0].material_ready);
        }
    }
    #[test]
    fn completed_result_survives_later_fault_and_expired_deadline(){
        let mut s=equipment("ALLOWED",now()+10000);assert!(ecs_tick(&mut s));
        assert_eq!(s.interactions[0].status,"COMPLETED");assert!(!s.devices[0].material_ready);
        s.devices[0].fault=true;s.interactions[0].completion_deadline_at=Some(1);
        assert!(!ecs_tick(&mut s));assert_eq!(s.interactions[0].status,"COMPLETED");assert!(s.interactions[0].alarm.is_none());
    }
}
