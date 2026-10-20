use crate::{
    MAXIMUM_PARALLEL, Service,
    candy::{self, EFFORTS, HISTORY_LIMIT, History, Record},
    host, invalid, now_ms, now_nanos,
};
use gateway_plugin_sdk::{
    ErrorCode, PluginFault,
    call::management::*,
    client::{HostClient, TypedCall, TypedReply},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Duration};

/// 宿主单次管理调用上限为 120 秒，模型请求须在其中完成并留出写回记录的时间
const REQUEST_LIMIT: Duration = Duration::from_secs(100);
/// 管理接口响应上限 1 MiB；账号很多时减少列表中每个账号的历史条数
const RESPONSE_BUDGET: usize = 960 * 1024;

pub fn registration() -> ManagementRegistration {
    ManagementRegistration {
        routes: [
            ("GET", "api/bootstrap"),
            ("POST", "api/models"),
            ("POST", "api/candy/run"),
            ("POST", "api/candy/record"),
            ("POST", "api/candy/clear"),
        ]
        .into_iter()
        .map(|(method, path)| ManagementRoute {
            method: method.into(),
            path: path.into(),
            request_content_types: if method == "POST" {
                vec!["application/json".into()]
            } else {
                vec![]
            },
            response_content_types: vec!["application/json".into()],
        })
        .collect(),
        resources: ["web/index.html", "web/app.js", "web/app.css"]
            .into_iter()
            .map(|path| ManagementResource {
                path: path.into(),
                public: false,
            })
            .collect(),
        pages: vec![ManagementPage {
            id: "model-health".into(),
            title: "账号降智测试".into(),
            description: Some("用糖果题批量测试账号是否降智，按模型与推理强度统计正确率".into()),
            entry: "web/index.html".into(),
            icon: None,
        }],
        callbacks: vec![],
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RunInput {
    account_id: String,
    key_id: String,
    model: String,
    effort: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordInput {
    account_id: String,
    record_id: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClearInput {
    account_ids: Vec<String>,
}

fn input<T: serde::de::DeserializeOwned>(
    call: &TypedCall<ManagementRequest>,
) -> Result<T, PluginFault> {
    serde_json::from_slice(&call.payload).map_err(|_| invalid("请求参数格式错误"))
}
fn reply(value: Value) -> Result<TypedReply<ManagementResponse>, PluginFault> {
    reply_status(200, value)
}
fn reply_status(status: u16, value: Value) -> Result<TypedReply<ManagementResponse>, PluginFault> {
    Ok(TypedReply::new(ManagementResponse {
        status,
        content_type: "application/json".into(),
        headers: vec![],
    })
    .with_payload(serde_json::to_vec(&value).map_err(|_| invalid("响应编码失败"))?))
}
/// 账号 ID 直接作为状态键，需符合宿主的键规则
fn account_key(id: &str) -> Result<&str, PluginFault> {
    if id.is_empty()
        || id.chars().count() > 256
        || id.len() > 512
        || id.chars().any(char::is_control)
    {
        return Err(invalid("账号 ID 格式错误"));
    }
    Ok(id)
}

pub async fn handle(
    service: &Service,
    call: TypedCall<ManagementRequest>,
) -> Result<TypedReply<ManagementResponse>, PluginFault> {
    match dispatch(service, call).await {
        Ok(response) => Ok(response),
        Err(error) => {
            let status = match error.code {
                ErrorCode::InvalidInput => 400,
                ErrorCode::Rejected | ErrorCode::PermissionDenied => 403,
                ErrorCode::Unsupported => 404,
                ErrorCode::Conflict => 409,
                ErrorCode::Capacity => 503,
                ErrorCode::Timeout => 504,
                ErrorCode::Upstream => 502,
                ErrorCode::Fault | ErrorCode::Cancelled | ErrorCode::Uncertain => 500,
            };
            // 应用错误走 HTTP 响应；协议 Error 会被宿主替换成泛化的“副作用”提示。
            // 不转发原始 details；保留有界原因和发送事实，页面不会自动重试。
            reply_status(
                status,
                json!({"error": {
                    "code": error.code,
                    "message": candy::excerpt(&error.message, 512),
                    "send_state": error.send_state,
                }}),
            )
        }
    }
}

async fn dispatch(
    service: &Service,
    call: TypedCall<ManagementRequest>,
) -> Result<TypedReply<ManagementResponse>, PluginFault> {
    if !call.request.query.is_empty() || call.payload.len() > 256 * 1024 {
        return Err(invalid("管理请求大小或查询参数不符合接口"));
    }
    match (call.request.method.as_str(), call.request.path.as_str()) {
        ("GET", "api/bootstrap") => reply(bootstrap(service, &call.host).await?),
        ("POST", "api/models") => {
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct ModelInput {
                key_id: String,
            }
            let query: ModelInput = input(&call)?;
            reply(
                serde_json::to_value(host::models(&call.host, &query.key_id).await?)
                    .map_err(|_| invalid("模型列表编码失败"))?,
            )
        }
        ("POST", "api/candy/run") => run(service, &call.host, input(&call)?).await,
        ("POST", "api/candy/record") => {
            let query: RecordInput = input(&call)?;
            let (mut history, _) = host::read(&call.host, account_key(&query.account_id)?)
                .await?
                .ok_or_else(|| invalid("记录不存在或已被清空"))?;
            if !service.is_busy(&query.account_id) {
                history.recover();
            }
            let record = history
                .records
                .into_iter()
                .find(|r| r.id == query.record_id)
                .ok_or_else(|| invalid("记录不存在或已被清空"))?;
            reply(json!({ "record": record }))
        }
        ("POST", "api/candy/clear") => clear(service, &call.host, input(&call)?).await,
        _ => Err(PluginFault::new(
            ErrorCode::Unsupported,
            "未注册的插件管理接口",
        )),
    }
}

async fn bootstrap(service: &Service, host: &HostClient) -> Result<Value, PluginFault> {
    let accounts = host::accounts(host).await?;
    let keys = host::keys(host).await?;
    let mut histories = Vec::with_capacity(accounts.len());
    for account in &accounts {
        let mut history = host::read(host, account_key(&account.account_id)?)
            .await?
            .map(|(history, _)| history)
            .unwrap_or_else(|| History::new(&account.account_id));
        // 只影响展示：没有进行中请求的 pending 记录显示为中断，下次测试时再持久化
        if !service.is_busy(&account.account_id) {
            history.recover();
        }
        histories.push(history);
    }
    let mut body = Value::Null;
    for limit in [HISTORY_LIMIT, 10, 5, 1] {
        let views = accounts
            .iter()
            .zip(&histories)
            .map(|(a, history)| {
                json!({"id":a.account_id,"name":a.name,"email":a.email,"plan":a.plan_type,
                    "enabled":a.enabled,"credential_state":a.credential_state,
                    "running":service.is_busy(&a.account_id),"records":history.summaries(limit)})
            })
            .collect::<Vec<_>>();
        body = json!({"prompt":candy::PROMPT,"answer":candy::ANSWER,"efforts":EFFORTS,
            "timeout_seconds":REQUEST_LIMIT.as_secs(),"history_limit":HISTORY_LIMIT,"history_shown":limit,
            "parallel_limit":MAXIMUM_PARALLEL,"accounts":views,"keys":keys});
        if serde_json::to_vec(&body).map_or(0, |b| b.len()) <= RESPONSE_BUDGET {
            break;
        }
    }
    Ok(body)
}

async fn run(
    service: &Service,
    host: &HostClient,
    query: RunInput,
) -> Result<TypedReply<ManagementResponse>, PluginFault> {
    if !EFFORTS.contains(&query.effort.as_str()) {
        return Err(invalid("未知的推理强度"));
    }
    let model = query.model.trim();
    if model.is_empty() || model.chars().count() > 256 {
        return Err(invalid("请选择模型"));
    }
    if query.key_id.trim().is_empty() {
        return Err(invalid("请选择计量 Key"));
    }
    let account_id = account_key(&query.account_id)?;
    let _permit = service.execution.try_acquire().map_err(|_| {
        PluginFault::new(
            ErrorCode::Capacity,
            format!("同时进行的测试超过 {MAXIMUM_PARALLEL} 个，请降低并发后重试"),
        )
    })?;
    let _busy = service.claim(account_id)?;
    host::testable_account(host, account_id).await?;
    let pending = Record::pending(now_nanos().to_string(), now_ms(), model, &query.effort);
    {
        let _lock = service.state.lock().await;
        let (mut history, version) = stored(host, account_id).await?;
        history.recover();
        history.push(pending.clone());
        // 发送前写入 pending；进程或页面中断后按未知结果计入，不会变成免费重试
        host::write(host, &history, version).await?;
    }
    // 网络等待期间不持有状态锁，其他账号可以并行作答
    let started = tokio::time::Instant::now();
    let execution = host::ask(
        host,
        account_id,
        &query.key_id,
        model,
        candy::payload(model, &query.effort),
        REQUEST_LIMIT,
    )
    .await;
    let mut record = pending.graded(execution.request_id, execution.result);
    record.duration_ms = Some(started.elapsed().as_millis() as u64);
    let _lock = service.state.lock().await;
    let (mut history, version) = stored(host, account_id).await?;
    history.finish(record.clone());
    host::write(host, &history, version).await?;
    reply(json!({"record":record.summary(),"records":history.summaries(HISTORY_LIMIT)}))
}

async fn stored(
    host: &HostClient,
    account_id: &str,
) -> Result<(History, Option<u64>), PluginFault> {
    Ok(match host::read(host, account_id).await? {
        Some((history, version)) => (history, Some(version)),
        None => (History::new(account_id), None),
    })
}

async fn clear(
    service: &Service,
    host: &HostClient,
    query: ClearInput,
) -> Result<TypedReply<ManagementResponse>, PluginFault> {
    if query.account_ids.len() > 10_000 {
        return Err(invalid("一次最多清空 10000 个账号的记录"));
    }
    let ids = query
        .account_ids
        .iter()
        .map(|id| account_key(id))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let _lock = service.state.lock().await;
    let (mut cleared, mut skipped) = (0, vec![]);
    for id in ids {
        // 正在作答的账号稍后要写回结果，不能清掉它的 pending 记录
        if service.is_busy(id) {
            skipped.push(id);
            continue;
        }
        if let Some((_, version)) = host::read(host, id).await? {
            host::delete(host, id, version).await?;
            cleared += 1;
        }
    }
    reply(json!({"cleared":cleared,"skipped":skipped}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_ids_follow_host_state_key_rules() {
        assert!(account_key("acct_1").is_ok());
        assert!(account_key(&"账".repeat(170)).is_ok());
        for id in [
            String::new(),
            "a\nb".into(),
            "x".repeat(257),
            "账".repeat(171),
        ] {
            assert!(account_key(&id).is_err());
        }
    }

    #[test]
    fn request_limit_leaves_room_inside_host_call_timeout() {
        assert!(REQUEST_LIMIT + Duration::from_secs(15) <= Duration::from_secs(120));
    }
}
