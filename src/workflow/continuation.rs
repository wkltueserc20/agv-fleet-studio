//! Continue a stopped workflow without canceling it or changing vehicle position.
use super::*;
#[derive(Clone, Serialize, Deserialize, PartialEq)]
pub struct Confirmation {
    pub request_id:String,pub operator:String,pub note:String,
    pub equipment_ready:bool,pub vehicle_stopped:bool,pub cargo_confirmed:bool,
    pub interaction_result:Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Attempt {
    pub confirmation:Confirmation,pub step:String,pub state:String,
    pub requested_at:u64,pub deadline_at:u64,pub resumed_at:Option<u64>,
    pub original_alarm:Option<limits::Alarm>,pub evidence:Value,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Review {
    pub job_id:String,pub command_id:String,pub vehicle_id:String,pub station:String,pub action:String,
    pub confirmation:Confirmation,pub at:u64,
}
fn valid(c:&Confirmation)->bool{
    !c.request_id.trim().is_empty()&&c.request_id.len()<=100&&!c.operator.trim().is_empty()&&c.operator.len()<=120
        &&!c.note.trim().is_empty()&&c.note.len()<=1000&&c.equipment_ready&&c.vehicle_stopped&&c.cargo_confirmed
        &&c.interaction_result.as_deref().is_none_or(|v|v=="COMPLETED")
}
pub(super) fn request(shared:&Shared<Jobs>,r:&Request)->Response{
    let Some(id)=r.path.strip_prefix("/api/jobs/").and_then(|p|p.strip_suffix("/continue"))else{return error(404,"找不到作業");};
    let c:Confirmation=match parse(r){Ok(v)=>v,Err(e)=>return e};
    if !valid(&c){return error(400,"請填處理人、說明及請求編號，確認設備、停止及實際載貨狀態；交接結果僅可確認已完成");}
    let mut s=shared.lock().unwrap();if s.storage_error.is_some(){return error(503,"WCS 儲存異常");}
    if s.jobs.iter().any(|j|j.id!=id&&j.continuations.iter().any(|a|a.confirmation.request_id==c.request_id)){return error(409,"請求編號已用於其他作業");}
    let Some(j)=s.jobs.iter_mut().find(|j|j.id==id)else{return error(404,"找不到作業");};
    if let Some(index)=j.continuations.iter().position(|a|a.confirmation.request_id==c.request_id){
        let a=&j.continuations[index];
        if a.confirmation!=c{return error(409,"相同請求編號不能改變確認內容");}
        if a.state=="DONE"{return ok(json!({"job":j,"duplicate":true}));}
        if j.status=="RESUMING"{let mut out=ok(json!({"job":j,"duplicate":true}));out.code=202;return out;}
        if index+1!=j.continuations.len(){return error(409,"舊確認已被後續處理取代");}
    }
    if j.status!="INTERRUPTED"||j.cancel_requested_at.is_some()||j.recovery.is_some(){return error(409,"僅能接續未取消、待人工確認的作業；已取消請使用結案復歸");}
    if j.continuations.last().is_some_and(|a|a.state!="DONE"&&a.confirmation!=c){return error(409,"尚有未完成的接續確認，請用原內容重試或取消，避免變更已保存的交接證據");}
    if !matches!(j.step.as_str(),"WAIT_SOURCE"|"WAIT_DEST"|"PICK"|"DROP"){return error(409,"目前步驟不支援接續；路段中斷或位置不明需取消後人工處理");}
    if matches!(j.step.as_str(),"PICK"|"DROP")!=(c.interaction_result.as_deref()==Some("COMPLETED")){return error(400,"取放料步驟必須確認交接已完成；設備等待步驟不應填交接完成結果");}
    if j.continuations.len()>=100&&!j.continuations.iter().any(|a|a.confirmation.request_id==c.request_id){return error(409,"此作業人工接續次數達上限");}
    if let Some(a)=j.continuations.last_mut().filter(|a|a.confirmation.request_id==c.request_id){a.state="REQUESTED".into();a.deadline_at=now()+60_000;}
    else{j.continuations.push(Attempt{confirmation:c,step:j.step.clone(),state:"REQUESTED".into(),requested_at:now(),deadline_at:now()+60_000,resumed_at:None,original_alarm:j.alarm.clone(),evidence:Value::Null});}
    j.status="RESUMING".into();j.paused=false;j.updated_at=now();j.wait_reason=Some("已保存確認，核對原車、載貨與設備後接續；尚未恢復作業".into());
    j.events.push(json!({"kind":"CONTINUE_REQUESTED","at":now(),"step":j.step,"message":"人工確認後接續要求已保存；保留原車、貨物及位置，不重做已確認交接","confirmation":j.continuations.last().unwrap().confirmation}));
    let job=j.clone();save_jobs(&mut s);if s.storage_error.is_some(){return error(503,"確認尚未保存，請使用原請求重試");}
    let mut out=ok(json!({"job":job,"accepted":true}));out.code=202;out
}
pub(super) fn advance(j:&mut Job){
    let attempt=j.continuations.last().unwrap().clone();
    let station=if matches!(j.step.as_str(),"WAIT_SOURCE"|"PICK"){j.from.clone()}else{j.to.clone()};
    let result=(||->Result<Value,String>{
        if j.step!=attempt.step{return Err("作業步驟與保存的確認不一致".into());}
        if j.step=="WAIT_SOURCE"&&(j.vehicle_id.is_some()||j.loaded){return Err("來源等待的車輛／載貨紀錄不符".into());}
        if j.step=="WAIT_DEST"&&!j.loaded{return Err("尚未確認貨物在原車上，不能接續送貨".into());}
        let rcs=http(RCS,"POST","/api/job-continue-check",json!({"job_id":j.id,"vehicle_id":j.vehicle_id,"expected_node":if j.step=="WAIT_DEST"{&j.from}else{&station}}))?;
        let ecs=http(ECS,"GET","/api/equipment",Value::Null)?;
        if !ecs["storage_error"].is_null(){return Err("ECS 儲存異常".into());}
        if ecs["canceled_jobs"].as_array().is_some_and(|xs|xs.iter().any(|x|x==&json!(j.id))){return Err("設備已收到作業取消，不可接續".into());}
        let device=ecs["devices"].as_array().and_then(|ds|ds.iter().find(|d|d["id"]==station)).ok_or("缺少設備資訊")?;
        if device["fault"]!=false{return Err("設備仍有故障，請先處理設備".into());}
        let review=if matches!(j.step.as_str(),"PICK"|"DROP"){
            http(ECS,"POST","/api/interactions/confirm-result",json!({"job_id":j.id,"command_id":format!("{}-{}",j.id,j.step),"vehicle_id":j.vehicle_id,"station":station,"action":j.step,"confirmation":attempt.confirmation}))?
        }else{
            if device[if j.step=="WAIT_SOURCE"{"material_ready"}else{"can_receive"}]!=true{return Err("設備尚未恢復有料／可收料條件".into());}
            if ecs["interactions"].as_array().is_some_and(|ops|ops.iter().any(|op|op["station"]==station&&matches!(op["status"].as_str(),Some("REQUESTED"|"ALLOWED"|"TIMED_OUT"|"FAILED"|"INTERRUPTED"))
                &&!ecs["reconciliations"].as_array().is_some_and(|rs|rs.iter().any(|r|r["job_id"]==op["job_id"]))
                &&!ecs["confirmed_results"].as_array().is_some_and(|rs|rs.iter().any(|r|r["command_id"]==op["command_id"])))){return Err("設備仍有執行中或未對帳的交接".into());}
            Value::Null
        };
        Ok(json!({"rcs":rcs,"equipment":device,"review":review}))
    })();
    j.updated_at=now();
    match result{
        Ok(evidence)=>{
            let a=j.continuations.last_mut().unwrap();a.state="DONE".into();a.resumed_at=Some(now());a.evidence=evidence;
            j.alarm=None;j.paused=false;j.attention_since=None;
            let next=match j.step.as_str(){"PICK"=>{j.loaded=true;"WAIT_DEST".to_owned()},"DROP"=>{j.loaded=false;"RELEASE".to_owned()},_=>j.step.clone()};
            change(j,&next,"人工確認與原車核對完成，接續原作業；等待期限重新計算，不重做已確認的取放料");
            j.events.push(json!({"kind":"CONTINUE_DONE","at":now(),"step":j.step,"message":"原作業已接續，處理人與原異常保留於人工接續紀錄"}));
        },
        Err(reason)=>{
            let a=j.continuations.last_mut().unwrap();a.evidence=json!({"checked_at":now(),"error":reason});a.state="VERIFYING".into();
            j.wait_reason=Some(format!("尚未接續：{reason}"));
            if now()>=a.deadline_at{a.state="BLOCKED".into();j.status="INTERRUPTED".into();j.events.push(json!({"at":now(),"step":j.step,"message":"接續核對逾時，維持待人工確認，可重試或取消並結案"}));}
        }
    }
}
pub fn rcs_check(shared:&Shared<RcsState>,r:&Request)->Response{
    if r.source!="wcs"{return error(403,"僅供 WCS 接續核對（來源標頭非正式認證）");}
    let v:Value=match parse(r){Ok(v)=>v,Err(e)=>return e};
    let Some(id)=v["job_id"].as_str().filter(|id|!id.is_empty())else{return error(400,"缺少 job_id");};
    let fleet=fleet_states();let s=shared.lock().unwrap();
    if s.storage_error.is_some(){return error(503,"RCS 儲存異常");}
    if canceled(&s,id){return error(409,"已取消作業不得接續");}
    let lock=s.job_locks.iter().find(|l|l.job_id==id);
    if s.tasks.iter().any(|t|t.job_id.as_deref()==Some(id)&&t.status!="COMPLETED"){return error(409,"有未完成或中止的子任務，不能接續");}
    let Some(car_id)=v["vehicle_id"].as_str()else{
        return if lock.is_none()&&!s.tasks.iter().any(|t|t.job_id.as_deref()==Some(id)){ok(json!({"checked_at":now(),"vehicle":null,"retained":false}))}else{error(409,"作業缺少原用車資訊")};
    };
    if !lock.is_some_and(|l|l.vehicle_id==car_id){return error(409,"原用車保留不符，不能換車接續");}
    let Some(car)=fleet.iter().find(|c|c["vehicle_id"]==car_id)else{return error(409,"缺少原車車況");};
    if car["online"]!=true||car["paused"]!=false||!car["active"].is_null()||car["pending_callbacks"]!=0||!car["road_edge"].is_null()
        ||now().saturating_sub(car["sample_at"].as_u64().unwrap_or(0))>5000||v["expected_node"].as_str().and_then(node).is_none()||car["current_node"]!=v["expected_node"]{return error(409,"原車須在線、停止於預期站點、未暫停且回報送完；不會設定車輛位置");}
    if s.tasks.iter().any(|t|t.assigned_vehicle.as_deref()==Some(car_id)&&!terminal(&t.status))||s.roads.leases.iter().any(|l|l.vehicle_id==car_id)||s.traffic.leases.iter().any(|l|l.vehicle_id==car_id){return error(409,"原車仍有任務或未確認的道路／站點預約");}
    ok(json!({"checked_at":now(),"vehicle":car,"retained":true}))
}
pub(super) fn equipment(s:&mut Equipment,r:&Request)->Response{
    if r.source!="wcs"{return error(403,"僅供 WCS 確認交接結果");}
    #[derive(Deserialize)]struct Input{job_id:String,command_id:String,vehicle_id:String,station:String,action:String,confirmation:Confirmation}
    let v:Input=match parse(r){Ok(v)=>v,Err(e)=>return e};
    if !valid(&v.confirmation)||v.confirmation.interaction_result.as_deref()!=Some("COMPLETED"){return error(400,"須明確確認交接已完成及載貨狀態");}
    if let Some(review)=s.confirmed_results.iter().find(|r|r.command_id==v.command_id){
        return if review.job_id==v.job_id&&review.vehicle_id==v.vehicle_id&&review.station==v.station&&review.action==v.action&&review.confirmation==v.confirmation{ok(json!({"review":review,"duplicate":true}))}else{error(409,"交接已有不同的確認紀錄")};
    }
    if s.canceled_jobs.contains(&v.job_id){return error(409,"作業已取消");}
    let Some(op)=s.interactions.iter().find(|op|op.command_id==v.command_id)else{return error(409,"無交接紀錄，不可推定已完成");};
    if op.job_id!=v.job_id||op.vehicle_id!=v.vehicle_id||op.station!=v.station||op.action!=v.action||!matches!(op.status.as_str(),"COMPLETED"|"TIMED_OUT"|"FAILED"|"INTERRUPTED"){return error(409,"交接身分不符或尚未停止，不能確認接續");}
    if s.interactions.iter().any(|other|other.station==v.station&&other.command_id!=v.command_id&&(matches!(other.status.as_str(),"REQUESTED"|"ALLOWED")||other.requested_at>op.requested_at)){return error(409,"設備已有其他執行中或較新的交接，不以舊結果覆寫設備狀態");}
    let Some(device)=s.devices.iter_mut().find(|d|d.id==v.station&& !d.fault)else{return error(409,"設備仍有故障");};
    // Update the simulated observed goods state; preserve the original timed-out operation.
    device.material_ready=v.action=="DROP";
    let review=Review{job_id:v.job_id,command_id:v.command_id,vehicle_id:v.vehicle_id,station:v.station,action:v.action,confirmation:v.confirmation,at:now()};
    s.confirmed_results.push(review.clone());save_equipment(s);if s.storage_error.is_some(){return error(503,"交接確認尚未保存");}
    ok(json!({"review":review,"simulation":true}))
}
