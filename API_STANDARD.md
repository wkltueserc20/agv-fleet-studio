# API 通訊規格與邊界

版本：draft-2026-11-03。此文件記錄本機原型的實際介面；尚未宣告為可交付外部廠商的正式 v1。目前介接服務、WCS、RCS、ECS 及三台 AGV 共七個獨立程序，共用一個 Rust 專案。設備位址目前固定為本機連接埠，跨電腦部署與正式認證尚待實作。

## 角色與資料責任

| 角色 | 責任 | 可以呼叫 | 不應呼叫 |
|---|---|---|---|
| 介接服務 :8210 | 外部作業／任務入口 | WCS（整張作業）、RCS（單項任務） | AGV、ECS；模擬移車或復歸 |
| WCS :8215 | 作業步驟、等待與整張作業結果 | RCS、ECS | 直接控制 AGV 或 PLC |
| ECS :8216 | 設備握手（目前為模擬） | 設備信號模擬器 | 派車或管理車隊 |
| RCS :8211 | 保存任務、派車、路徑與交管、判斷回報 | AGV | 自己的 HTTP；為畫面反查介接服務 |
| AGV :8212–8214 | 執行本車任務、提供車況、回報與申請通行 | RCS | 介接服務、其他 AGV |

回覆沿原 HTTP 連線返回。AGV → RCS 的進度回報、通行申請是合法反向通訊。不是所有封包都只能單向往右走。介接服務下單成功只表示 RCS 已保存任務，不表示車端已接單或任務已完成。

## 第一階段 WCS／ECS（已新增）

- 新搬運作業走介接服務 `POST /api/jobs`，內容為 request_id、from、to、可選 requested_vehicle；201 新建、200 同內容重送、409 衝突。`GET /api/jobs` 查整張作業。
- `POST /api/jobs/{job_id}/pause {paused:true/false}` 只停止 WCS 發出下一步。已派車及設備握手仍可能完成，不是急停。
- 介接服務仍提供 `POST /api/tasks` 的 MOVE；舊 TRANSPORT 接口保留相容，仍是車端時間模擬，不能當成有 WCS／ECS 的新作業。
- WCS 固定流程為：WAIT_SOURCE → RESERVE → TO_SOURCE → PICK → WAIT_DEST → TO_DEST → DROP → RELEASE → DONE。每一步有時間、用車、等待原因及子任務編號。
- RCS `/api/job-lock`、`/api/job-unlock` 為 WCS 內部接口。整張作業用車鎖定寫入 rcs.json；普通任務、其他作業、自動讓位與模擬移車不可挪用該車。解除前確認任務、回報與道路／點位預約均已結束。
- WCS 以 `{job_id}-source`、`{job_id}-dest` 作為 MOVE request_id，並把 job_id 傳給 RCS。子任務核對作業鎖定及車號。已在站點時不建立原地任務。
- ECS `GET /api/equipment` 提供設備及交接；`POST /api/interactions` 使用 command_id、job_id、vehicle_id、station、action(PICK/DROP)。相同 command_id 不重做，異內容回 409。
- 模擬握手 REQUESTED → ALLOWED → COMPLETED；完成含模擬信號復位。交接受有料／可收料、故障及自動完成開關控制；尚無實體 PLC 或車端取放料感測。
- `POST :8216/api/equipment/sim/signals` 僅在 ECS 內部頁操作模擬信號。介接服務不轉送此操作。
- WCS `GET /api/dashboard` 彙整自身作業、ECS 狀態與診斷紀錄；介接服務／RCS 的 `/api/workflow` 只代理這個監控接口。RCS 派車本身不依賴這個畫面查詢。
- WCS 作業存在 data/wcs.json，設備與操作存在 data/ecs.json，RCS 舊任務仍存在 data/rcs.json。重啟時，非取消中的已指派未完成作業與未完成設備交接標記 INTERRUPTED；取消中的作業繼續查證，不恢復正常搬運。保留資源，不自動重做。
- 已提供整張作業取消，取消確認逾時轉人工處理；設備等待逾時與中斷作業的人工對帳／模擬復歸已實作；不可用舊單項取消當成整張作業取消。子任務被取消時 WCS 轉待確認。HTTP 與設備等待已有逾時處置；車輛移動／交通逾時仍屬後續 RCS 範圍。
- 作業用車鎖定與暫停狀態都持久化。來源尚未就緒且未鎖車的作業重啟後可繼續等待；已取得用車的作業需要人工確認，不能直接按繼續。

## 現有接口分類

`GET /api/catalog` 每項有 `contract` 及 `contract_version`，畫面可按用途篩選。

| 分類 | 接收服務與接口 | 用途 |
|---|---|---|
| external 外部入口 | 介接服務 POST /api/tasks | 建立搬運需求 |
| external 外部入口 | 介接服務 GET /api/state | 查詢任務及車隊摘要，目前仍回完整 RCS 狀態 |
| external 外部入口 | 介接服務 GET /api/vehicles | 經 RCS 查詢車況 |
| external 外部入口 | 介接服務 POST /api/cancel/{task_id} | 提出取消需求 |
| management 車隊管理 | RCS POST /api/tasks、GET /api/state、GET /api/vehicles、POST /api/dispatch、POST /api/cancel/{task_id} | 介接服務對接及內部管理；派車開關不提供給介接服務 |
| device 設備通訊 | AGV POST /api/missions、GET /api/state、POST /api/cancel/{task_id} | RCS 交付、查詢、取消本車任務 |
| device 設備通訊 | RCS POST /api/events、POST /api/entry | AGV 任務回報與路徑通行申請 |
| device 本車操作 | AGV POST /api/control | 本機暫停／繼續，並非硬體急停 |
| diagnostic 畫面／診斷 | 各服務 /health、/api/snapshot、/api/traces、/api/catalog、/api/map | 開發觀察工具，不交給外部作任務接口 |
| simulation 模擬測試 | RCS /api/sim/park、/api/sim/blocking-demo、/api/roads/recover；AGV /api/sim/position | 改模擬位置及復歸，正式設備接口不採用 |
| legacy 舊版相容 | 介接服務 GET /api/vehicle | 固定取 CT-SIM-01，外部改用 /api/vehicles |

介接服務的模擬移車、建立阻擋情境、路段復歸三個接口已移除，呼叫回 404。RCS 模擬接口仍保留供內部測試。

## 已實作的資料與處理規則

- 傳輸 HTTP + JSON，UTF-8，欄位 snake_case。時間採 Unix 毫秒，介面顯示台北時間；座標目前是示意單位。
- 建單內容：`request_id` 必填、非空、UTF-8 最多 100 位元組；`from`、`to` 為有效且不同的站點。`requested_vehicle` 可選。`kind` 接受 TRANSPORT（預設）或 MOVE（點到點移動）；外部不可建立內部 YIELD 任務。RCS 產生 `task.id` 並保存，介接服務不另存一份任務佇列。
- 相同 `request_id` 與相同內容：200 + `duplicate:true`，不重複建單；相同編號但內容不同：409。新任務：201。介接服務原樣保留 RCS 的 HTTP 狀態碼及 JSON；連線／解析失敗才回 502。
- 錯誤目前為 `{ "error": "說明" }`。400 表示輸入錯誤；404 不存在；409 狀態衝突；儲存錯誤等以實際服務碼為準。尚無統一機器可讀 error_code。
- MOVE 語意：`from` 是必經起點，`to` 是終點；車不在 from 時，先沿核准路徑前往 from，再前往 to。兩段分別使用 pickup／destination 通行階段，階段名稱沿用協定但不進行任何取放料動作。若已在 from，第一段是原地到站確認。保留交管、自動讓位、進度回報及取消；取消途中仍保留未確認路段。
- 車端任務：`task_id`、`vehicle_id`、`from`、`to`、`kind`，讓位另有 `parent_task`。核對車號、忙碌及重複任務，不能因 HTTP 重送再次開始同一單。
- AGV 任務回報：`task_id`、`vehicle_id`、`seq`、`status`、`progress`、`phase`。核對指派車號；重複／舊順序可略過。當前接受 RUNNING、COMPLETED、CANCELED；設備 FAILED 與拒收原因碼仍待補。
- AGV 通行申請：`task_id`、`vehicle_id`、`stage`；RCS 回 `granted`、`path`、`route_token` 或等待原因。HTTP 200 不等於 granted:true。
- RCS 背景約每 0.5 秒 GET 各車 /api/state；任務推進時 AGV POST /api/events。無任務時獨立主動心跳尚未實作。API GET 紀錄每 2 秒取樣，不是所有查詢逐筆保留。
- RCS 的畫面快照讀內部任務與背景車況，不因畫面更新額外查車。介接服務快照從 RCS 取得資料並加入本機紀錄。AGV 快照只讀本車，RCS 離線仍可看本車；這不表示 RCS 離線後仍可自行開始新路徑。
- 診斷快照會向 AGV 收集 /api/traces，屬開發觀察用途，不是派車流程。正式版應改用統一日誌平台或可關閉的診斷服務。
- API「暫停畫面更新」只凍結前端資料，不停止調度／查詢／回報。暫停期間仍只保留服務自身有限歷史，並非永久錄製。

## 正式 v1 前仍需完成

1. 凍結版本化接口（建議 /api/v1）、OpenAPI 與 JSON Schema，加入單筆任務查詢，定義取消中／取消完成。現有未版本化接口尚未冒稱已是 v1。
2. 將外部回應收斂為任務狀態與必要車況；目前 /api/state 仍含內部事件與交管資訊，不應直接提供外部完整管理資料。
3. 加入 request_id 全鏈路關聯、command_id、error_code、協定版本、boot_id、車況序號、過期判斷，定義 timeout 後查證再重試，避免不確定結果造成重派。
4. 外部客戶認證、任務所有權、管理者與設備權限、TLS、限流。X-Lab-Source 只供紀錄來源，可偽造，不是認證或授權。當前僅綁定 loopback；用途分類也不構成權限隔離。
5. 將介接服務／RCS／車端拆成獨立套件與可設定位址，共享協定型別；目前同一 binary 的不同 role，不同程序沒有共享記憶體，但仍共享編譯期模擬地圖與固定車號。
6. 補足真正設備狀態、PLC 安全／故障／載貨確認、充電、離線重連與重啟對帳。與真車對接前需逐項驗收。

整張搬運作業目前只接受 A、B、C、WIP、CUT、CNC、OVEN、PACK 八個設備交接站。充電站、待命區、空閒區及避讓點仍可用 MOVE 抵達，但不冒充有取放握手的機台。

### 介接服務名稱對照

對外畫面名稱為「外部介接入口」，服務名稱 integration-service（原稱中央）。網址仍為 :8210；既有 API、role: central 與 --role central 保留相容，不改歷史資料。程式入口處理函式改稱 integration_handler，啟動紀錄與新日誌使用 integration-service 名稱。此服務目前接收、轉交、查詢，完整 API Gateway 的認證／限流仍未實作。


### 整張作業取消（第二階段第一項）

- 外部介接入口／RCS 管理頁代理 `POST /api/jobs/{job_id}/cancel` 到 WCS；可帶 `reason`。首次202表示已持久保存取消要求，不表示取消完成。相同作業重送只查回同一取消結果，完成作業不能用取消撤銷。
- WCS 停止正常流程推進，即使原作業已暫停也會處理取消。取消時若已有 HTTP 指令在途，仍須查證；RCS `/api/job-cancel` 與 ECS `/api/interactions/cancel-job` 保存屏障，阻止後續同作業動作。
- RCS 逐一取消該作業子任務；交付中等接單結果確認。已接受的讓位動作先完成到等待點，不在半路直接解除。ECS 只停止未完成的模擬交接；完成與結果不明的紀錄不被改寫。
- 取消最終可能為 CANCELED（已確認取消）、INTERRUPTED（待人工確認、保留資源）或 COMPLETED／TOO_LATE（放料已完成，未撤銷結果）。已取料、車在半路、道路仍有預約、回報不明等情況不釋放用車。
- 取消期間保存要求／期限／階段、證據與時間點。預設60秒確認期限，可用 WCS_CANCEL_TIMEOUT_MS 設1000～300000毫秒；此為取消確認期限，不是一般業務步驟逾時。逾時轉待人工確認，不當作停止成功。
- WCS 重啟的取消中作業會繼續確認，不恢復正常作業步驟。RCS 保存取消屏障及成功解除的紀錄；ECS 保存取消屏障與模擬操作結果，避免重啟後重做。未知或已載貨情況仍需後續人工對帳功能，不提供強制清空保留。
- GUI 的「取消整張作業」與原有「取消 RCS 子任務」不同；後者不代表整張作業已取消。API 紀錄增加 job_id／command_id 供關聯檢查。設備取消仍是本機模擬，不是實際 PLC 安全停止。


## 人工確認與模擬復歸

WCS `/api/jobs/{job_id}/recover` 僅處理「已提出取消且 INTERRUPTED」的作業。重啟中斷須先取消；正常執行、已完成或已取消的作業不能藉此復歸。內部 WCS 操作頁提供處理人、說明、等待點及五項確認；外部介接入口8210回403。來源標頭仍不是正式身份認證，正式部署需加入權限與操作員登入。

首次202表示確認已保存，狀態RECOVERING。WCS先確認ECS交接停止並保存獨立人工對帳，再請RCS核對在線、無任務、回報已送完及目標無占用／預約，直接設定模擬位置至P1/P2/P3/I1/I2；核對成功後一次保存RCS復歸收據及解除作業用車、原作業道路預約。這不是實際行駛或真車復歸協定；貨物清空是人工聲明，非感測器驗證。

成功後作業CANCELED、cancel_phase=MANUALLY_RESOLVED、recovery.state=DONE。保留原步驟、任務狀態、已完成／不確定交接及所有事件，另記人工處理貨物、處理人、時間、位置和各服務收據；不重跑原搬運。ECS故障／有料信號不自動改寫。車輛原暫停狀態不自動解除。

60秒仍未確認時回INTERRUPTED、recovery.state=BLOCKED，保留尚未解除資源。相同確認內容與request_id可重試；不同內容回409，避免部分成功後更改位置。WCS重啟續查收據；RCS收據必須先於車況檢查去重，已解除車輛即使正在執行新單也不再次設定位置。設備等待逾時已有本機版本；真車感測及人工身份權限仍待實作。


## 設備等待期限與異常處理

內部WCS/ECS頁提供「設備等待期限與異常處理」。兩服務各有GET/POST `/api/policy`；外部介接入口不轉送此管理API。單位毫秒，設定範圍1000～86400000（畫面以秒輸入），持久保存，WCS套用新作業、ECS套用新交接。各作業／交接保留建立時的政策快照，變更不縮短既有期限。舊資料無期限時從首次監控起初始化，不以過去建立時間誤判。

預設WCS來源／目的設備各300秒、ECS結果確認120秒；ECS等待允許30秒、允許後等待完成60秒。採實際時間，包含流程暫停、離線與重啟時間。暫停仍不停止已發起的設備交接；WCS持續唯讀監控已發起交接與等待期限，但不發起下一步。移動與派車交通逾時屬RCS，未在此新增。

ECS交接逾時為TIMED_OUT，設備故障為FAILED，包含alarm{code,owner,station,command_id,occurred_at,message}。異常碼ECS_ALLOW_TIMEOUT／ECS_COMPLETION_TIMEOUT／EQUIPMENT_FAULT；ECS重啟未完成交接為INTERRUPTED／ECS_RESTART。截止後信號才恢復不改回成功；相同command_id回原結果，不重置計時。未對帳站點不接受新交接。

WCS來源或目的等待逾時為SOURCE_WAIT_TIMEOUT／DESTINATION_WAIT_TIMEOUT；未能取得設備完成結果（含通訊中斷）為ECS_RESPONSE_TIMEOUT。WCS收到ECS異常保留其owner和code，作業轉INTERRUPTED，保存一次ALARM事件、等待期限與原因。已收到完成結果不倒寫成逾時；未知載貨／交接結果仍需人工處理。故障信號清除不自動恢復作業。

處理方式：符合條件時由WCS人工確認後接續；否則取消整張作業，協調停止後人工確認與模擬復歸。保留已完成／逾時／不確定的原交接，另存對帳收據；已對帳的異常不再阻擋設備接新作業。貨物、用車、道路仍按原取消／復歸規則確認，不因超時直接釋放。實車／PLC故障碼、硬體安全停止、操作權限、設備斷線心跳及自動重試策略尚待實作。


## 確認異常後繼續原作業

內部WCS POST `/api/jobs/{job_id}/continue` 接受request_id、operator、note、equipment_ready、vehicle_stopped、cargo_confirmed與interaction_result。外部8210不開放；RCS內部管理頁可轉送。首次202為RESUMING，表示已保存確認，未代表已恢復。重送同一request_id及內容回原結果，不重做；不同內容409。保存人工接續陣列continuations（原步驟、原異常、處理人、時間、證據），成功清除目前alarm但保留原ALARM事件。

僅支援未取消、INTERRUPTED的WAIT_SOURCE／WAIT_DEST／PICK／DROP。WAIT_SOURCE確認尚未取料、來源有料且無故障後重新計時；WAIT_DEST確認貨物仍在原車、目的可收料後重新計時。PICK／DROP必須填interaction_result=COMPLETED，明確人工確認實際已完成，才分別接WAIT_DEST／RELEASE，不重新執行本次交接。未完成或不明仍用取消與結案，不提供重做按鈕。

WCS以內部 `/api/job-continue-check` 請RCS核對：原作業無取消屏障、原車保留一致、子任務均已完成、車在線、原站點、無路段中斷、未暫停、空閒、回報送完、無道路／站點預約。接續不改位置、不換車、不卸貨、不解除保留。TO_SOURCE／TO_DEST等移動中斷不在本階段範圍；已取消／正在取消及結案復歸中的作業不可接續。

ECS內部 `/api/interactions/confirm-result` 保存獨立confirmed_results：核對job／command／vehicle／station／action一致、交接已完成或已停止（TIMED_OUT／FAILED／INTERRUPTED）、無故障及無其他交接。原操作status與時間不改寫；模擬有料信號依人工完成的PICK=false／DROP=true更新。已確認操作不再阻擋設備，取消判斷也納入人工完成證據，避免重複取放料或誤判載貨。這是本機人工聲明，不是實車感測驗證。

RESUMING預設60秒核對；未通過保持保留，逾時INTERRUPTED且attempt.state=BLOCKED，可原內容重試或取消。WCS重啟繼續核對、ECS確認收據持久去重。成功沿用原作業的政策快照重算下一等待期限。取消優先於接續：並行取消要求合併後不恢復正常流程。操作員登入／授權與實車載貨、定位、交接結果驗證仍待對接。


## 待人工確認處理清單與提醒

WCS頁新增「待人工確認」分頁、跨頁提醒橫幅；介接入口／RCS操作頁的WCS作業區有清單連結。可按提醒程度篩選、搜尋作業／車號／設備／原因，依等待最久排序。顯示原異常、首次待確認時間、等待多久、原車、載貨紀錄及RCS實際保留的用車、道路路徑與站點。關聯設備不等於已被占用；尚未精算被阻擋的其他任務數。

GET WCS `/api/attention`回傳checked_at、policy、summary、items、storage_error、rcs_error。WCS儲存attention_since，進入INTERRUPTED時設定；重啟／重試不歸零，取消／接續／復歸核對期間仍列出。成功接續或完成／取消結案後移出，原異常／確認歷史保留。再次發生新異常開始新的等待時間。舊資料首次載入依alarm.occurred_at或updated_at補入時間。

POST WCS `/api/attention/policy`接受warning_ms、critical_ms；1000≤黃色<紅色≤86400000。預設300000／900000毫秒（畫面5／15分鐘），立即影響目前與之後清單，持久保存，與業務步驟逾時政策分開。未達門檻WAITING、達黃色WARNING、達紅色CRITICAL；包含核對與離線期間的實際時間。

RCS資源只核對本作業及其關聯讓位任務；已經屬於其他作業的車輛預約不歸到舊作業。RCS離線、資料格式不完整或儲存異常時resources.known=false，畫面標示無法確認，不顯示假零值；WCS連線中斷保留上次畫面並顯示未更新。狀態為本次查詢快照，不是真車安全驗證。

清單按鈕沿用確認後接續、取消及人工結案API，不另行繞過核對。提醒和政策設定不操作AGV、不自動釋放保留、不發送外部通知。外部介接8210不開放提醒管理API。指定人員通知、已讀／接手／值班升級、通知歷史及真車感測／正式權限仍待實作。
