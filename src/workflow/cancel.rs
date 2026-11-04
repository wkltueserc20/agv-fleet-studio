//! Cancellation is a persisted coordination request, never an immediate release.
use super::*;

pub(super) fn request(shared: &Shared<Jobs>, r: &Request) -> Response {
    let Some(id)=r.path.strip_prefix("/api/jobs/").and_then(|s|s.strip_suffix("/cancel")) else{return error(404,"找不到取消接口");};
    #[derive(Deserialize)] struct CancelRequest {#[serde(default)] reason:Option<String>}
    let value:CancelRequest=if r.body.is_empty() || r.body==b"null"{CancelRequest{reason:None}}else{match parse(r){Ok(v)=>v,Err(e)=>return e}};
    if value.reason.as_ref().is_some_and(|s|s.len()>500){return error(400,"取消原因最多 500 位元組");}
    let mut state=shared.lock().unwrap();
    if state.storage_error.is_some(){return error(503,"WCS 儲存異常，無法保存取消要求");}
    let Some(job)=state.jobs.iter_mut().find(|j|j.id==id)else{return error(404,"找不到作業");};
    if job.status=="COMPLETED" {return error(409,"整張作業已完成，不能以取消撤銷已完成的交接");}
    if job.cancel_requested_at.is_some() {
        let mut response=ok(json!({"job":job,"duplicate":true,"accepted":true}));
        if job.status=="CANCELING"{response.code=202;}
        return response;
    }
    let timestamp=now();
    let timeout=std::env::var("WCS_CANCEL_TIMEOUT_MS").ok().and_then(|s|s.parse::<u64>().ok()).unwrap_or(60_000).clamp(1000,300_000);
    job.cancel_requires_review=job.status=="INTERRUPTED";
    job.cancel_requested_at=Some(timestamp);job.cancel_deadline_at=Some(timestamp+timeout);
    job.cancel_reason=value.reason.filter(|s|!s.trim().is_empty());
    job.cancel_phase=Some("REQUESTED".into());job.status="CANCELING".into();job.paused=false;
    job.wait_reason=Some("已保存取消要求，正在協調車輛與設備；尚未確認停止或解除保留".into());
    job.updated_at=timestamp;
    job.events.push(json!({"kind":"CANCEL_REQUESTED","at":timestamp,"step":job.step,"message":"收到整張作業取消要求，停止推進正常流程"}));
    let response_job=job.clone();save_jobs(&mut state);
    if state.storage_error.is_some(){return error(503,"取消要求保存失敗，請對同一 job_id 重試");}
    let mut response=ok(json!({"job":response_job,"accepted":true,"duplicate":false}));response.code=202;response
}

pub(super) fn equipment(state: &mut Equipment, r: &Request) -> Response {
    if r.source!="wcs"{return error(403,"僅供 WCS 協調取消模擬交接");}
    let value:Value=match parse(r){Ok(v)=>v,Err(e)=>return e};
    let Some(id)=value["job_id"].as_str().filter(|s|!s.is_empty()&&s.len()<=120)else{return error(400,"job_id 必填");};
    let duplicate=state.canceled_jobs.iter().any(|s|s==id);
    if !duplicate {
        if state.canceled_jobs.len()>=10000{return error(409,"設備取消紀錄達上限");}
        state.canceled_jobs.push(id.into());
    }
    // Serialized with the simulator's completion loop. Completed or uncertain
    // operations retain their result; only unfinished simulated work is stopped.
    for operation in state.interactions.iter_mut().filter(|op|op.job_id==id) {
        if matches!(operation.status.as_str(),"REQUESTED"|"ALLOWED") {
            operation.status="CANCELED".into();operation.canceled_at=Some(now());
            operation.phase="模擬交接已取消，通知／完成信號已復位；未完成的取放料不自動視為成功".into();
        }
    }
    save_equipment(state);
    if state.storage_error.is_some(){return error(503,"設備取消結果保存失敗");}
    ok(json!({"job_id":id,"fenced":true,"duplicate":duplicate,"simulation":true,
        "interactions":state.interactions.iter().filter(|op|op.job_id==id).collect::<Vec<_>>(),"confirmed_results":state.confirmed_results.iter().filter(|r|r.job_id==id).collect::<Vec<_>>()}))
}

fn phase(job: &mut Job, code: &str, reason: &str) {
    job.status="CANCELING".into();job.paused=false;job.cancel_phase=Some(code.into());
    job.wait_reason=Some(reason.into());job.updated_at=now();
}
fn manual(job: &mut Job, reason: &str) {
    job.cancel_phase=Some("MANUAL_REVIEW".into());job.cancel_requires_review=true;
    interrupt(job,reason);
}
pub(super) fn advance(job: &mut Job) {
    if let Err(reason)=coordinate(job) {
        if now()>=job.cancel_deadline_at.unwrap_or(u64::MAX) {
            manual(job,&format!("取消確認逾時：{reason}；尚未確認的車輛、設備與道路保留不自動解除"));
        } else {
            job.status="CANCELING".into();job.wait_reason=Some(reason);job.updated_at=now();
        }
    }
}

fn coordinate(job: &mut Job) -> Result<(),String> {
    phase(job,"STOPPING","阻止新增動作，協調停止車輛與模擬設備交接");
    // Attempt both sides independently: an offline ECS must not prevent a
    // reachable RCS from receiving the cancellation, or vice versa.
    let rcs_fence=http(RCS,"POST","/api/job-cancel",json!({"job_id":job.id}));
    let ecs_stop=http(ECS,"POST","/api/interactions/cancel-job",json!({"job_id":job.id}));
    let before=http(RCS,"GET","/api/state",Value::Null);
    if let Ok(state)=&before {
        if let Some(tasks)=state["tasks"].as_array() {
            for task in tasks.iter().filter(|t|t["job_id"]==job.id) {
                if let Some(id)=task["id"].as_str() {
                    if !job.task_ids.iter().any(|s|s==id){job.task_ids.push(id.into());}
                    if matches!(task["status"].as_str(),Some("QUEUED"|"RUNNING"|"INTERRUPTED")) {
                        // A 409 can mean completion won the race. Fresh state
                        // below determines the result, never the cancel ACK.
                        let _=http(RCS,"POST",&format!("/api/cancel/{id}"),json!({}));
                    }
                }
            }
        }
    }
    job.cancel_evidence=json!({"checked_at":now(),"rcs_fenced":rcs_fence.is_ok(),"ecs_fenced":ecs_stop.is_ok(),
        "rcs_error":rcs_fence.as_ref().err(),"ecs_error":ecs_stop.as_ref().err(),
        "interactions":ecs_stop.as_ref().ok().map(|v|&v["interactions"])});
    if let Err(e)=rcs_fence{return Err(format!("等待 RCS 確認停止新增任務：{e}"));}
    if let Err(e)=ecs_stop.as_ref(){return Err(format!("等待 ECS 確認設備取消：{e}"));}
    let state=http(RCS,"GET","/api/state",Value::Null)?;
    if !state["storage_error"].is_null(){return Err("RCS 儲存異常，取消結果尚未持久確認".into());}
    let tasks=state["tasks"].as_array().ok_or("RCS 任務狀態格式不符")?;
    let own:Vec<Value>=tasks.iter().filter(|t|t["job_id"]==job.id).cloned().collect();
    let own_ids:Vec<String>=own.iter().filter_map(|t|t["id"].as_str().map(str::to_owned)).collect();
    let related:Vec<Value>=tasks.iter().filter(|t|t["kind"]=="YIELD"&&t["parent_task"].as_str().is_some_and(|id|own_ids.iter().any(|s|s==id))).cloned().collect();
    let interactions=ecs_stop.as_ref().unwrap()["interactions"].as_array().ok_or("ECS 取消結果格式不符")?;
    let locks=state["job_locks"].as_array().ok_or("RCS 用車保留格式不符")?;
    let lock=locks.iter().find(|l|l["job_id"]==job.id);
    let released_proof=state["job_cancellations"].as_array().is_some_and(|fs|fs.iter().any(|f|f["job_id"]==job.id&&!f["released_at"].is_null()));
    if let Some(lock)=lock {
        let actual=lock["vehicle_id"].as_str().ok_or("RCS 缺少保留車號")?;
        if job.vehicle_id.as_deref().is_some_and(|id|id!=actual){manual(job,"作業車號與 RCS 保留紀錄不一致，需人工確認");return Ok(());}
        job.vehicle_id=Some(actual.into());
    }
    let fleet=http(RCS,"GET","/api/vehicles",Value::Null)?;
    let observes_own_car=lock.is_some()||!released_proof;
    let car=if observes_own_car{job.vehicle_id.as_deref().and_then(|id|fleet["vehicles"].as_array()?.iter().find(|v|v["vehicle_id"]==id)).cloned()}else{None};
    let related_ids:Vec<String>=related.iter().filter_map(|t|t["id"].as_str().map(str::to_owned)).collect();
    let roads:Vec<Value>=state["roads"]["reservations"].as_array().ok_or("RCS 道路預約格式不符")?.iter()
        .filter(|l|(observes_own_car&&job.vehicle_id.is_some()&&l["vehicle_id"].as_str()==job.vehicle_id.as_deref())||own_ids.iter().chain(related_ids.iter()).any(|id|l["task_id"]==*id)).cloned().collect();
    job.cancel_evidence=json!({"checked_at":now(),"rcs_fenced":true,"ecs_fenced":true,"tasks":own,
        "related_yield_tasks":related,"interactions":interactions,"vehicle":car,"road_reservations":roads,"vehicle_lock":lock,"released_proof":released_proof});
    phase(job,"VERIFYING","已送出停止要求，確認任務結果、位置、載貨及道路預約");
    if own.iter().any(|t|matches!(t["status"].as_str(),Some("QUEUED"|"DISPATCHING"|"RUNNING"))) {
        job.cancel_verify_since=None;
        return Err("等待 RCS 子任務確認取消；交付中的任務須先確認車端接收結果".into());
    }
    if related.iter().any(|t|matches!(t["status"].as_str(),Some("QUEUED"|"DISPATCHING"|"RUNNING"))) {
        job.cancel_verify_since=None;
        return Err("等待已接受的關聯讓位任務抵達等待點，不在半路強制釋放".into());
    }
    if let Some(car)=&car {
        if car["online"]!=true{return Err("等待作業用車重新連線以確認取消結果".into());}
        if !car["active"].is_null()||car["pending_callbacks"]!=0{job.cancel_verify_since=None;return Err("等待車端停止執行並完成回報，不以取消回應當作已停止".into());}
        if now().saturating_sub(car["sample_at"].as_u64().unwrap_or(0))>5000{return Err("等待最新車況確認".into());}
    } else if job.vehicle_id.is_some()&&observes_own_car{return Err("等待作業用車資訊".into());}
    let verifying_since=*job.cancel_verify_since.get_or_insert(now());
    let pick_id=format!("{}-PICK",job.id);let drop_id=format!("{}-DROP",job.id);
    let confirmed=ecs_stop.as_ref().unwrap()["confirmed_results"].as_array().cloned().unwrap_or_default();
    let picked=confirmed.iter().any(|r|r["command_id"]==pick_id)||interactions.iter().any(|op|op["command_id"]==pick_id&&op["status"]=="COMPLETED");
    let dropped=confirmed.iter().any(|r|r["command_id"]==drop_id)||interactions.iter().any(|op|op["command_id"]==drop_id&&op["status"]=="COMPLETED");
    if dropped{job.loaded=false;}else if picked{job.loaded=true;}
    let uncertain=job.cancel_requires_review
        ||own.iter().chain(related.iter()).any(|t|matches!(t["status"].as_str(),Some("INTERRUPTED"|"FAILED")))
        ||interactions.iter().any(|op|(matches!(op["status"].as_str(),Some("INTERRUPTED"|"TIMED_OUT"|"FAILED"))&&!confirmed.iter().any(|r|r["command_id"]==op["command_id"]))||(op["command_id"]!=pick_id&&op["command_id"]!=drop_id));
    if uncertain{manual(job,"取消停止已協調，但先前任務／設備結果不明；保留作業用車，需人工對帳");return Ok(());}
    if job.loaded{manual(job,"取消停止已確認，但作業已取料且尚未確認放料；保留用車與載貨紀錄，需人工處理貨物");return Ok(());}
    if !roads.is_empty(){
        if now().saturating_sub(verifying_since)<2000{return Err("車端已停止，等待 RCS 以最新車況確認道路預約是否可釋放".into());}
        manual(job,"車已停止，但仍有未確認清空的道路預約；保留用車與道路，需人工確認最終位置");return Ok(());
    }
    if car.as_ref().is_some_and(|v|!v["road_edge"].is_null()||v["current_node"].as_str().and_then(node).is_none()) {
        manual(job,"車輛未確認停在有效站點，不能解除作業用車");return Ok(());
    }
    if job.vehicle_id.is_some()&&lock.is_none()&&!released_proof&&!dropped {
        manual(job,"作業用車保留紀錄缺失，無法確認先前是否已解除；不操作可能已屬其他作業的車輛");return Ok(());
    }
    if lock.is_some(){http(RCS,"POST","/api/job-unlock",json!({"job_id":job.id}))?;}
    job.updated_at=now();job.wait_reason=None;job.paused=false;
    if dropped {
        job.step="DONE".into();job.status="COMPLETED".into();job.cancel_phase=Some("TOO_LATE".into());
        job.wait_reason=Some("取消確認時，放料交接已完成；作業保留為已完成，不撤銷交接結果".into());
        job.events.push(json!({"at":now(),"step":job.step,"message":"取消時已完成放料，完成結果與用車解除均已確認"}));
    } else {
        job.status="CANCELED".into();job.cancel_phase=Some("DONE".into());job.canceled_at=Some(now());
        job.events.push(json!({"at":now(),"step":job.step,"message":"整張作業已取消：車輛／設備已確認，無載貨與未確認道路，作業用車已解除"}));
    }
    Ok(())
}
