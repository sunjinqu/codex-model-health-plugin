use crate::{
    candy::{Answer, History, ResponseParser},
    invalid,
};
use gateway_plugin_sdk::{
    PluginFault,
    call::{host::*, model::WirePayload},
    client::HostClient,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::json;
use std::{collections::BTreeSet, time::Duration};
use tokio::time::{Instant, timeout_at};

const NAMESPACE: &str = "candy";

pub async fn metadata<I: Serialize, O: DeserializeOwned>(
    host: &HostClient,
    method: &str,
    input: I,
) -> Result<O, PluginFault> {
    let reply = host
        .call(
            method,
            serde_json::to_value(input).map_err(|_| invalid("回调编码失败"))?,
            vec![],
        )
        .await
        .map_err(|e| e.into_plugin_fault())?;
    if !reply.payload.is_empty() {
        return Err(invalid("宿主元数据回调夹带了正文"));
    }
    serde_json::from_value(reply.result).map_err(|_| invalid("宿主回调响应格式错误"))
}
async fn payload<I: Serialize, O: DeserializeOwned>(
    host: &HostClient,
    method: &str,
    input: I,
) -> Result<O, PluginFault> {
    let reply = host
        .call(
            method,
            json!({}),
            serde_json::to_vec(&input).map_err(|_| invalid("回调编码失败"))?,
        )
        .await
        .map_err(|e| e.into_plugin_fault())?;
    serde_json::from_slice(&reply.payload).map_err(|_| invalid("宿主账号回调响应格式错误"))
}

/// 列出全部 OpenAI 账号（含已停用），页面据此显示状态；只有启用的账号可以测试
pub async fn accounts(host: &HostClient) -> Result<Vec<AuthRuntimeAccount>, PluginFault> {
    let mut cursor = None;
    let mut seen = BTreeSet::new();
    let mut accounts = vec![];
    loop {
        let page: AuthListResult = payload(
            host,
            "host.auth.list",
            AuthListRequest {
                provider_id: Some("openai".into()),
                cursor,
                limit: 100,
            },
        )
        .await?;
        accounts.extend(
            page.accounts
                .into_iter()
                .filter(|a| a.provider_id == "openai"),
        );
        cursor = page.next_cursor;
        let Some(next) = &cursor else {
            return Ok(accounts);
        };
        if !seen.insert(next.clone()) || seen.len() > 100 {
            return Err(invalid("账号分页重复或超过 100 页"));
        }
    }
}
pub async fn keys(host: &HostClient) -> Result<Vec<ClientKey>, PluginFault> {
    let mut cursor = None;
    let mut seen = BTreeSet::new();
    let mut keys = vec![];
    loop {
        let page = host
            .list_keys(KeyListRequest { cursor, limit: 100 })
            .await?;
        keys.extend(page.keys.into_iter().filter(|k| k.enabled));
        cursor = page.next_cursor;
        let Some(next) = &cursor else {
            return Ok(keys);
        };
        if !seen.insert(next.clone()) || seen.len() > 100 {
            return Err(invalid("Key 分页重复或超过 100 页"));
        }
    }
}
pub async fn models(host: &HostClient, key: &str) -> Result<ModelListResult, PluginFault> {
    metadata(
        host,
        "host.models.list",
        ModelListRequest {
            client_key_id: key.into(),
            protocol: "openai".into(),
            client_version: concat!("model-health/", env!("CARGO_PKG_VERSION")).into(),
        },
    )
    .await
}

/// 每次作答前重新确认账号仍启用且属于 OpenAI 平台
pub async fn testable_account(
    host: &HostClient,
    account_id: &str,
) -> Result<AuthRuntimeAccount, PluginFault> {
    let account: AuthRuntimeAccount = payload(
        host,
        "host.auth.get_runtime",
        AuthGetRequest {
            account_id: account_id.into(),
        },
    )
    .await?;
    if !account.enabled || account.provider_id != "openai" || account.account_id != account_id {
        return Err(invalid("账号已停用或不再是 OpenAI 账号，请刷新列表"));
    }
    Ok(account)
}

pub async fn read(
    host: &HostClient,
    account_id: &str,
) -> Result<Option<(History, u64)>, PluginFault> {
    let reply: StateGetResult = metadata(
        host,
        "host.state.get",
        StateGetRequest {
            namespace: NAMESPACE.into(),
            key: account_id.into(),
        },
    )
    .await?;
    reply
        .record
        .map(|r| {
            serde_json::from_value(r.value)
                .map(|history| (history, r.version))
                .map_err(|_| invalid("测试记录格式错误"))
        })
        .transpose()
}
pub async fn write(
    host: &HostClient,
    history: &History,
    version: Option<u64>,
) -> Result<(), PluginFault> {
    let _: StatePutResult = metadata(
        host,
        "host.state.put",
        StatePutRequest {
            namespace: NAMESPACE.into(),
            key: history.account_id.clone(),
            value: serde_json::to_value(history).map_err(|_| invalid("状态编码失败"))?,
            expected_version: version,
        },
    )
    .await?;
    Ok(())
}
pub async fn delete(host: &HostClient, account_id: &str, version: u64) -> Result<(), PluginFault> {
    let _: StateDeleteResult = metadata(
        host,
        "host.state.delete",
        StateDeleteRequest {
            namespace: NAMESPACE.into(),
            key: account_id.into(),
            expected_version: version,
        },
    )
    .await?;
    Ok(())
}

pub struct Execution {
    pub request_id: Option<String>,
    pub result: Result<Answer, String>,
}

/// 显式绑定账号、Key、provider 与协议发送一次请求；整次请求受 `limit` 约束，超时即关闭执行流
pub async fn ask(
    host: &HostClient,
    account_id: &str,
    key_id: &str,
    model: &str,
    body: serde_json::Value,
    limit: Duration,
) -> Execution {
    let deadline = Instant::now() + limit;
    let timeout = || format!("单次请求达到 {} 秒上限", limit.as_secs());
    let request = ModelExecuteRequest {
        client_key_id: Some(key_id.into()),
        model: model.into(),
        protocol: "openai".into(),
        operation: ModelOperation::Generate,
        provider: Some("openai".into()),
        account_id: Some(account_id.into()),
        previous_response_id: None,
    };
    let failed = |message: String| Execution {
        request_id: None,
        result: Err(message),
    };
    let (Ok(metadata), Ok(body)) = (serde_json::to_value(request), serde_json::to_vec(&body))
    else {
        return failed("模型请求编码失败".into());
    };
    let reply = match timeout_at(
        deadline,
        host.call("host.model.execute_stream", metadata, body),
    )
    .await
    {
        Ok(Ok(reply)) => reply,
        Ok(Err(error)) => return failed(error.into_plugin_fault().message),
        Err(_) => return failed(timeout()),
    };
    let Ok(started) = serde_json::from_value::<ModelStreamResult>(reply.result) else {
        return failed("模型响应缺少请求 ID".into());
    };
    let result = match timeout_at(deadline, read_stream(host, &started.stream)).await {
        Ok(Ok(answer)) => Ok(answer),
        other => {
            let mut message = match other {
                Ok(Err(message)) => message,
                _ => timeout(),
            };
            let close = host
                .call(
                    "host.model.stream_close",
                    json!({"stream":started.stream}),
                    vec![],
                )
                .await;
            if let Err(error) = close {
                message.push_str(&format!(
                    "；关闭执行流：{}",
                    error.into_plugin_fault().message
                ));
            }
            Err(message)
        }
    };
    Execution {
        request_id: Some(started.request_id),
        result,
    }
}

async fn read_stream(host: &HostClient, stream: &str) -> Result<Answer, String> {
    let mut parser = ResponseParser::default();
    loop {
        let reply = host
            .call(
                "host.model.stream_read",
                json!({"stream":stream,"maximum_bytes":262144}),
                vec![],
            )
            .await
            .map_err(|e| e.into_plugin_fault().message)?;
        let state: ModelStreamReadResult =
            serde_json::from_value(reply.result).map_err(|_| "模型流状态格式错误".to_string())?;
        let batch = if reply.payload.is_empty() && state.events == 0 {
            ModelEventBatch::default()
        } else {
            ModelEventBatch::decode(&reply.payload)
                .map_err(|_| "模型事件批次格式错误".to_string())?
        };
        if batch.events.len() != state.events as usize {
            return Err("模型事件数量不一致".into());
        }
        for event in batch.events {
            let Some(wire) = event.wire else {
                continue;
            };
            if wire.protocol != "openai" {
                return Err("模型返回了不匹配的协议".into());
            }
            match wire.payload {
                // 协议 5 起，上游走 WebSocket 时事件以 WebSocket 原文交付，解析视图同样可判分
                WirePayload::Json { data, .. }
                | WirePayload::WebSocket {
                    data: Some(data), ..
                } => parser.feed(&data)?,
                WirePayload::RawSse { frame } => parser.feed_sse(
                    std::str::from_utf8(&frame).map_err(|_| "SSE 编码错误".to_string())?,
                )?,
                _ => return Err("模型未返回 Responses 流，无法判分".into()),
            }
        }
        if state.end {
            return parser.finish();
        }
    }
}
