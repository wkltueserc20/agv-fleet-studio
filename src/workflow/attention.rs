//! Read-only operator queue. Escalation changes presentation, never resource ownership.
use super::*;
#[derive(Clone,Serialize,Deserialize)]
#[serde(default,deny_unknown_fields)]
pub struct Policy{pub warning_ms:u64,pub critical_ms:u64}
impl Default for Policy{fn default()->Self{Self{warning_ms:300_000,critical_ms:900_000}}}
pub(super) fn track(j:&mut Job){
    if matches!(j.status.as_str(),"COMPLETED"|"CANCELED"){j.attention_since=None;}
    else if matches!(j.status.as_str(),"INTERRUPTED"|"RESUMING"|"RECOVERING"){j.attention_since.get_or_insert_with(||j.alarm.as_ref().map_or(j.updated_at,|a|a.occurred_at));}
}
pub(super) fn policy(shared:&Shared<Jobs>,r:&Request)->Response{
    let p:Policy=match parse(r){Ok(v)=>v,Err(e)=>return e};
    if p.warning_ms<1000||p.warning_ms>=p.critical_ms||p.critical_ms>86_400_000{return error(400,"提醒門檻需符合 1000 ≤ 黃色 < 紅色 ≤ 86400000 毫秒");}
    let mut s=shared.lock().unwrap();if s.storage_error.is_some(){return error(503,"WCS 儲存異常");}
    s.attention_policy=p;save_jobs(&mut s);if s.storage_error.is_some(){return error(503,"提醒門檻尚未保存");}
    ok(json!({"policy":s.attention_policy,"applies_to":"current_and_future_attention","notification":"workbench_only","releases_resources":false}))
}
fn severity(age:u64,p:&Policy)->&'static str{if age>=p.critical_ms{"CRITICAL"}else if age>=p.warning_ms{"WARNING"}else{"WAITING"}}
pub(super) fn snapshot(shared:&Shared<Jobs>)->Value{
    let (jobs,policy,storage_error)={let s=shared.lock().unwrap();(s.jobs.clone(),s.attention_policy.clone(),s.storage_error.clone())};
    let pending:Vec<_>=jobs.iter().filter(|j|j.status=="INTERRUPTED"||j.attention_since.is_some()&&matches!(j.status.as_str(),"RESUMING"|"RECOVERING"|"CANCELING")).collect();
    let rcs=if pending.is_empty(){Ok(Value::Null)}else{http(RCS,"GET","/api/state",Value::Null).and_then(|v|if !v["storage_error"].is_null(){Err("RCS 儲存異常，資源保留需另行確認".into())}else if [ &v["tasks"], &v["job_locks"], &v["roads"]["reservations"], &v["stations"]["stations"] ].iter().all(|a|a.is_array()){Ok(v)}else{Err("RCS 資源格式不完整，無法確認保留".into())})};
    let checked_at=now();let empty=vec![];
    let mut items:Vec<Value>=pending.into_iter().map(|j|{
        let since=j.attention_since.unwrap_or_else(||j.alarm.as_ref().map_or(j.updated_at,|a|a.occurred_at));
        let age=checked_at.saturating_sub(since);
        let resources=rcs.as_ref().ok().filter(|v|!v.is_null()).map(|s|{
            let tasks=s["tasks"].as_array().unwrap_or(&empty);
            let own:Vec<_>=tasks.iter().filter(|t|t["job_id"]==j.id).filter_map(|t|t["id"].as_str()).collect();
            let related:Vec<_>=tasks.iter().filter(|t|t["kind"]=="YIELD"&&t["parent_task"].as_str().is_some_and(|id|own.contains(&id))).filter_map(|t|t["id"].as_str()).collect();
            let belongs=|v:&&Value|v["task_id"].as_str().is_some_and(|id|own.contains(&id)||related.contains(&id));
            json!({"known":true,"checked_at":checked_at,
                "vehicle_locks":s["job_locks"].as_array().unwrap_or(&empty).iter().filter(|l|l["job_id"]==j.id).collect::<Vec<_>>(),
                "road_reservations":s["roads"]["reservations"].as_array().unwrap_or(&empty).iter().filter(belongs).collect::<Vec<_>>(),
                "station_reservations":s["stations"]["stations"].as_array().unwrap_or(&empty).iter().flat_map(|station|station["reservations"].as_array().unwrap_or(&empty)).filter(belongs).collect::<Vec<_>>()})
        }).unwrap_or_else(||json!({"known":false,"checked_at":checked_at,"reason":"無法確認目前保留，不代表資源已解除"}));
        let station=j.alarm.as_ref().map(|a|a.station.clone()).unwrap_or_else(||if matches!(j.step.as_str(),"WAIT_DEST"|"TO_DEST"|"DROP"|"RELEASE"){j.to.clone()}else{j.from.clone()});
        json!({"job_id":j.id,"status":j.status,"step":j.step,"from":j.from,"to":j.to,"vehicle_id":j.vehicle_id,"loaded":j.loaded,
            "station":station,"reason":j.wait_reason,"alarm":j.alarm,"attention_since":since,"waiting_ms":age,"severity":severity(age,&policy),"resources":resources,
            "can_continue":j.status=="INTERRUPTED"&&j.cancel_requested_at.is_none()&&j.recovery.is_none()&&matches!(j.step.as_str(),"WAIT_SOURCE"|"WAIT_DEST"|"PICK"|"DROP"),
            "can_recover":j.status=="INTERRUPTED"&&j.cancel_requested_at.is_some(),"can_cancel":j.cancel_requested_at.is_none()})
    }).collect();
    items.sort_by_key(|v|v["attention_since"].as_u64().unwrap_or(0));
    json!({"checked_at":checked_at,"policy":policy,"storage_error":storage_error,"rcs_error":rcs.err(),"notification":"workbench_only","releases_resources":false,
        "summary":{"total":items.len(),"warning":items.iter().filter(|i|i["severity"]=="WARNING").count(),"critical":items.iter().filter(|i|i["severity"]=="CRITICAL").count()},"items":items})
}
#[cfg(test)]mod tests{
    use super::*;
    #[test]fn thresholds_are_inclusive_and_do_not_change_execution(){let p=Policy::default();assert_eq!(severity(299999,&p),"WAITING");assert_eq!(severity(300000,&p),"WARNING");assert_eq!(severity(899999,&p),"WARNING");assert_eq!(severity(900000,&p),"CRITICAL");}
}
