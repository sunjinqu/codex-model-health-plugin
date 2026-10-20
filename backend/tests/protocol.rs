use gateway_plugin_sdk::{
    CallContext, ErrorCode, Frame, Handshake, Manifest, Message, PROTOCOL_VERSION, PluginFault,
    Stage,
    call::{
        host::ModelEventBatch,
        model::{ExecutionEvent, WireEvent, WirePayload},
    },
    client::{PluginSession, SessionConfig, read_frame, write_frame},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Duration};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

struct Peer {
    reader: ReadHalf<DuplexStream>,
    writer: WriteHalf<DuplexStream>,
    task: tokio::task::JoinHandle<()>,
    sequence: u64,
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Peer {
    async fn new() -> Self {
        let (host, transport) = tokio::io::duplex(2 * 1024 * 1024);
        let (reader, writer) = tokio::io::split(transport);
        let task = tokio::spawn(async move {
            let session = PluginSession::accept(reader, writer, SessionConfig::default())
                .await
                .unwrap();
            session
                .run(codex_model_health_plugin::plugin().unwrap())
                .await
                .unwrap();
        });
        let (reader, writer) = tokio::io::split(host);
        let mut peer = Self {
            reader,
            writer,
            task,
            sequence: 1,
        };
        let manifest = Manifest::from_author_slice(include_bytes!("../../plugin.json")).unwrap();
        peer.send(Frame::control(Message::Hello {
            handshake: Handshake {
                protocol_version: PROTOCOL_VERSION,
                artifact_sha256: "a".repeat(64),
                plugin_id: "local.model-health".into(),
                instance_id: "test".into(),
                generation: 1,
                incarnation: "test".into(),
                configuration: json!({}),
                contributes: manifest.contributes,
            },
        }))
        .await;
        assert!(matches!(
            peer.receive().await.message,
            Message::Ready { .. }
        ));
        peer
    }
    async fn send(&mut self, frame: Frame) {
        write_frame(&mut self.writer, &frame).await.unwrap();
    }
    async fn receive(&mut self) -> Frame {
        tokio::time::timeout(Duration::from_secs(30), read_frame(&mut self.reader))
            .await
            .expect("plugin timeout")
            .unwrap()
    }
    async fn api(
        &mut self,
        path: &str,
        input: Option<Value>,
        host: &mut Host,
    ) -> Result<Value, PluginFault> {
        let id = self.sequence;
        self.sequence += 2;
        self.send(Frame{message:Message::Call{id,method:"management.handle".into(),context:CallContext{call_id:id,instance_id:"test".into(),generation:1,incarnation:"test".into(),stage:Stage::Management,timeout_ms:120000,resource_stream:false,resource_scope_id:"scope".into(),request_id:None,attempt_id:None,account_id:None,credential_revision:None,request:None},params:json!({"method":if input.is_some(){"POST"}else{"GET"},"path":path,"query":"","content_type":input.as_ref().map(|_|"application/json"),"headers":[]})},payload:input.map(|v|serde_json::to_vec(&v).unwrap()).unwrap_or_default()}).await;
        loop {
            let frame = self.receive().await;
            match frame.message {
                Message::Callback {
                    id: cid,
                    parent_id,
                    method,
                    params,
                } => {
                    assert_eq!(parent_id, id);
                    let response = match host.callback(&method, params, frame.payload) {
                        Ok((result, payload)) => Frame {
                            message: Message::Result { id: cid, result },
                            payload,
                        },
                        Err(error) => Frame::control(Message::Error { id: cid, error }),
                    };
                    self.send(response).await;
                }
                Message::Result { id: rid, result } => {
                    assert_eq!(id, rid);
                    assert_eq!(result["content_type"], "application/json");
                    let body: Value = serde_json::from_slice(&frame.payload).unwrap();
                    let status = result["status"].as_u64().unwrap();
                    if status >= 400 {
                        let mut error: PluginFault =
                            serde_json::from_value(body["error"].clone()).unwrap();
                        error.http_status = Some(status as u16);
                        return Err(error);
                    }
                    assert_eq!(status, 200);
                    return Ok(body);
                }
                Message::Error { id: rid, error } => {
                    panic!("application error leaked into protocol: {rid}, {error:?}");
                }
                _ => panic!("unexpected frame"),
            }
        }
    }
    async fn run(
        &mut self,
        account: &str,
        effort: &str,
        host: &mut Host,
    ) -> Result<Value, PluginFault> {
        self.api(
            "api/candy/run",
            Some(json!({"account_id":account,"key_id":"key-b","model":"gpt-6-astra","effort":effort})),
            host,
        )
        .await
    }
}

struct Host {
    state: BTreeMap<String, (Value, u64)>,
    executions: Vec<Value>,
    bodies: Vec<Value>,
    answer: String,
    fail: bool,
    raw_sse: bool,
    repeat_cursor: bool,
    callbacks: Vec<String>,
    closed: usize,
}
impl Default for Host {
    fn default() -> Self {
        Self {
            state: BTreeMap::new(),
            executions: vec![],
            bodies: vec![],
            answer: "最少需要 21 个糖果".into(),
            fail: false,
            raw_sse: false,
            repeat_cursor: false,
            callbacks: vec![],
            closed: 0,
        }
    }
}
fn account(id: &str) -> Value {
    json!({"account_id":id,"provider_id":"openai","credential_revision":1,"name":format!("name-{id}"),"email":format!("{id}@example.com"),"upstream_user_id":format!("user-{id}"),"upstream_account_id":format!("upstream-{id}"),"plan_type":"pro","authentication_kind":"oauth","enabled":id != "account-c","credential_state":"ready","has_refresh_token":true,"access_token_expires_at_ms":null,"next_refresh_at_ms":null})
}
impl Host {
    fn records(&self, account: &str) -> Vec<Value> {
        self.state
            .get(account)
            .map(|(v, _)| v["records"].as_array().unwrap().clone())
            .unwrap_or_default()
    }
    fn writes(&self) -> usize {
        self.callbacks
            .iter()
            .filter(|m| *m == "host.state.put")
            .count()
    }
    fn callback(
        &mut self,
        method: &str,
        params: Value,
        payload: Vec<u8>,
    ) -> Result<(Value, Vec<u8>), PluginFault> {
        self.callbacks.push(method.into());
        let metadata = |value| Ok((value, vec![]));
        if method.starts_with("host.state.") {
            // 旧版 Meow/ModelTrace 的 runs 命名空间只保留声明，新版本不读不写
            assert_eq!(params["namespace"], "candy");
        }
        match method {
            "host.auth.list" => {
                assert_eq!(params, json!({}));
                let query: Value = serde_json::from_slice(&payload).unwrap();
                assert_eq!(query["provider_id"], "openai");
                let first = query["cursor"].is_null();
                let accounts = if first {
                    vec![account("account-a")]
                } else {
                    vec![account("account-b"), account("account-c")]
                };
                let next = if first || self.repeat_cursor {
                    json!("page-2")
                } else {
                    Value::Null
                };
                let body = json!({"accounts":accounts,"next_cursor":next});
                Ok((json!({}), serde_json::to_vec(&body).unwrap()))
            }
            "host.auth.get_runtime" => {
                let query: Value = serde_json::from_slice(&payload).unwrap();
                Ok((
                    json!({}),
                    serde_json::to_vec(&account(query["account_id"].as_str().unwrap())).unwrap(),
                ))
            }
            "host.keys.list" => {
                let next = params["cursor"].is_null();
                metadata(
                    json!({"keys":[{"id":if next {"key-a"}else{"key-b"},"name":"test key","enabled":true}],"next_cursor":if next {json!("key-a")}else{Value::Null}}),
                )
            }
            "host.models.list" => {
                assert_eq!(params["client_key_id"], "key-b");
                assert_eq!(params["protocol"], "openai");
                metadata(json!({"models":["gpt-6-astra"]}))
            }
            "host.state.get" => {
                let record = self.state.get(params["key"].as_str().unwrap()).map(
                    |(value, version)| json!({"value":value,"version":version,"schema_version":1}),
                );
                metadata(json!({ "record": record }))
            }
            "host.state.put" => {
                let key = params["key"].as_str().unwrap().to_string();
                assert_eq!(params["value"]["account_id"], key);
                assert!(params["value"]["records"].as_array().unwrap().len() <= 20);
                let current = self.state.get(&key).map(|(_, v)| *v);
                if current != params["expected_version"].as_u64() {
                    return Err(PluginFault::new(ErrorCode::Conflict, "CAS conflict"));
                }
                let version = current.unwrap_or(0) + 1;
                self.state.insert(key, (params["value"].clone(), version));
                metadata(json!({ "version": version }))
            }
            "host.state.delete" => {
                let key = params["key"].as_str().unwrap();
                let current = self.state.get(key).map(|(_, v)| *v);
                if current != params["expected_version"].as_u64() {
                    return Err(PluginFault::new(ErrorCode::Conflict, "CAS conflict"));
                }
                self.state.remove(key);
                metadata(json!({"deleted":true}))
            }
            "host.model.execute_stream" => {
                assert_eq!(params["client_key_id"], "key-b");
                assert_eq!(params["provider"], "openai");
                assert_eq!(params["protocol"], "openai");
                assert_eq!(params["model"], "gpt-6-astra");
                let account = params["account_id"].as_str().unwrap();
                assert_eq!(
                    self.records(account).last().unwrap()["status"],
                    "pending",
                    "pending record must be persisted before sending"
                );
                self.executions.push(params);
                let body: Value = serde_json::from_slice(&payload).unwrap();
                assert_eq!(body["stream"], true);
                assert_eq!(body["store"], false);
                assert_eq!(body["model"], "gpt-6-astra");
                assert!(body["tools"].is_null());
                assert!(
                    body["input"][0]["content"]
                        .as_str()
                        .unwrap()
                        .contains("苹果味")
                );
                self.bodies.push(body);
                metadata(
                    json!({"request_id":format!("request-{}",self.executions.len()),"stream":"stream-one"}),
                )
            }
            "host.model.stream_read" => {
                if self.fail {
                    return Err(PluginFault::new(
                        ErrorCode::Upstream,
                        "fixture network error",
                    ));
                }
                let data = json!({"type":"response.completed","response":{"id":"response-one","status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":self.answer}]}],"usage":{"input_tokens":310,"output_tokens":45,"output_tokens_details":{"reasoning_tokens":20}}}});
                let payload = if self.raw_sse {
                    WirePayload::RawSse {
                        frame: format!("event: response.completed\ndata: {data}\n\n").into_bytes(),
                    }
                } else {
                    WirePayload::Json {
                        event: None,
                        data,
                        id: None,
                        retry: None,
                        raw_sse: None,
                    }
                };
                let payload = ModelEventBatch {
                    events: vec![ExecutionEvent::wire(WireEvent {
                        protocol: "openai".into(),
                        payload,
                    })],
                }
                .encode()
                .unwrap();
                Ok((json!({"events":1,"end":true}), payload))
            }
            "host.model.stream_close" => {
                assert_eq!(params["stream"], "stream-one");
                self.closed += 1;
                metadata(json!({}))
            }
            _ => panic!("unexpected callback: {method}"),
        }
    }
}

#[tokio::test]
async fn bootstrap_lists_every_account_and_key_without_inference_or_writes() {
    let mut peer = Peer::new().await;
    let mut host = Host::default();
    let boot = peer.api("api/bootstrap", None, &mut host).await.unwrap();
    let accounts = boot["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 3);
    for (view, id) in accounts.iter().zip(["account-a", "account-b", "account-c"]) {
        assert_eq!(view["id"], id);
        assert_eq!(view["email"], format!("{id}@example.com"));
        assert_eq!(view["plan"], "pro");
        assert_eq!(view["enabled"], id != "account-c");
        assert_eq!(view["running"], false);
        assert_eq!(view["records"], json!([]));
    }
    assert_eq!(boot["keys"].as_array().unwrap().len(), 2);
    assert_eq!(boot["answer"], "21");
    assert_eq!(boot["efforts"][0], "none");
    assert!(boot["timeout_seconds"].as_u64().unwrap() < 120);
    assert!(boot["prompt"].as_str().unwrap().contains("五角星形"));
    assert!(!boot.to_string().contains("access_token\":\""));
    assert!(host.executions.is_empty());
    assert_eq!(host.writes(), 0);
    let models = peer
        .api("api/models", Some(json!({"key_id":"key-b"})), &mut host)
        .await
        .unwrap();
    assert_eq!(models["models"], json!(["gpt-6-astra"]));
}

#[tokio::test]
async fn answers_are_graded_persisted_and_bound_to_the_selected_account_and_key() {
    let mut peer = Peer::new().await;
    let mut host = Host::default();
    let correct = peer.run("account-a", "low", &mut host).await.unwrap();
    assert_eq!(correct["record"]["status"], "correct");
    assert_eq!(correct["record"]["model"], "gpt-6-astra");
    assert_eq!(correct["record"]["effort"], "low");
    assert_eq!(correct["record"]["input_tokens"], 310);
    assert_eq!(correct["record"]["reasoning_tokens"], 20);
    assert!(correct["record"]["duration_ms"].is_u64());
    assert!(correct["record"].get("answer").is_none());
    assert_eq!(host.bodies[0]["reasoning"]["effort"], "low");
    assert_eq!(host.executions[0]["account_id"], "account-a");

    host.answer = "答案是 20 个".into();
    host.raw_sse = true;
    let wrong = peer.run("account-a", "low", &mut host).await.unwrap();
    assert_eq!(wrong["record"]["status"], "wrong");
    let records = wrong["records"].as_array().unwrap();
    assert_eq!(
        records.iter().map(|r| &r["status"]).collect::<Vec<_>>(),
        ["correct", "wrong"]
    );
    assert_eq!(host.records("account-a").len(), 2);
    assert_eq!(host.executions.len(), 2);

    let detail = peer
        .api(
            "api/candy/record",
            Some(json!({"account_id":"account-a","record_id":wrong["record"]["id"]})),
            &mut host,
        )
        .await
        .unwrap();
    assert_eq!(detail["record"]["answer"], "答案是 20 个");
    assert_eq!(detail["record"]["request_id"], "request-2");

    let boot = peer.api("api/bootstrap", None, &mut host).await.unwrap();
    assert_eq!(boot["accounts"][0]["records"].as_array().unwrap().len(), 2);
    assert_eq!(boot["accounts"][1]["records"], json!([]));
}

#[tokio::test]
async fn effort_none_leaves_reasoning_to_the_host() {
    let mut peer = Peer::new().await;
    let mut host = Host::default();
    let result = peer.run("account-b", "none", &mut host).await.unwrap();
    assert_eq!(result["record"]["effort"], "none");
    assert!(host.bodies[0].get("reasoning").is_none());
}

#[tokio::test]
async fn request_failures_are_recorded_but_never_graded() {
    let mut peer = Peer::new().await;
    let mut host = Host {
        fail: true,
        ..Host::default()
    };
    let result = peer.run("account-a", "low", &mut host).await.unwrap();
    assert_eq!(result["record"]["status"], "failed");
    assert_eq!(host.closed, 1, "a failed stream must be closed");
    let detail = peer
        .api(
            "api/candy/record",
            Some(json!({"account_id":"account-a","record_id":result["record"]["id"]})),
            &mut host,
        )
        .await
        .unwrap();
    assert!(
        detail["record"]["error"]
            .as_str()
            .unwrap()
            .contains("fixture network error")
    );
    assert_eq!(detail["record"]["request_id"], "request-1");

    host.fail = false;
    host.answer = "  ".into();
    let empty = peer.run("account-a", "low", &mut host).await.unwrap();
    assert_eq!(empty["record"]["status"], "failed");
}

#[tokio::test]
async fn disabled_accounts_and_malformed_requests_are_rejected_before_any_write() {
    let mut peer = Peer::new().await;
    let mut host = Host::default();
    let error = peer.run("account-c", "low", &mut host).await.unwrap_err();
    assert_eq!(error.http_status, Some(400));
    assert!(error.message.contains("已停用"));
    for body in [
        json!({"account_id":"account-a","key_id":"key-b","model":"gpt-6-astra","effort":"turbo"}),
        json!({"account_id":"account-a","key_id":"key-b","model":"  ","effort":"low"}),
        json!({"account_id":"account-a","key_id":"","model":"gpt-6-astra","effort":"low"}),
        json!({"account_id":"","key_id":"key-b","model":"gpt-6-astra","effort":"low"}),
        json!({"account_id":"account-a","model":"gpt-6-astra","effort":"low"}),
    ] {
        let error = peer
            .api("api/candy/run", Some(body.clone()), &mut host)
            .await
            .unwrap_err();
        assert_eq!(error.http_status, Some(400), "{body}");
    }
    assert_eq!(host.writes(), 0);
    assert!(host.executions.is_empty());
    assert_eq!(host.callbacks, ["host.auth.get_runtime"]);
}

#[tokio::test]
async fn interrupted_pending_records_are_charged_and_never_retried_for_free() {
    let mut peer = Peer::new().await;
    let mut host = Host::default();
    host.state.insert(
        "account-a".into(),
        (
            json!({"account_id":"account-a","records":[{"id":"1","time":1,"model":"gpt-6-astra","effort":"low","status":"pending"}]}),
            1,
        ),
    );
    let boot = peer.api("api/bootstrap", None, &mut host).await.unwrap();
    assert_eq!(boot["accounts"][0]["records"][0]["status"], "failed");
    assert_eq!(host.writes(), 0, "viewing does not rewrite history");
    let detail = peer
        .api(
            "api/candy/record",
            Some(json!({"account_id":"account-a","record_id":"1"})),
            &mut host,
        )
        .await
        .unwrap();
    assert!(detail["record"]["error"].as_str().unwrap().contains("中断"));

    let result = peer.run("account-a", "low", &mut host).await.unwrap();
    assert_eq!(
        result["records"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| &r["status"])
            .collect::<Vec<_>>(),
        ["failed", "correct"]
    );
    assert_eq!(host.records("account-a")[0]["status"], "failed");
    assert_eq!(host.executions.len(), 1);
}

#[tokio::test]
async fn history_keeps_the_latest_twenty_answers() {
    let mut peer = Peer::new().await;
    let mut host = Host::default();
    let mut ids = vec![];
    for _ in 0..22 {
        ids.push(
            peer.run("account-a", "medium", &mut host).await.unwrap()["record"]["id"]
                .as_str()
                .unwrap()
                .to_owned(),
        );
    }
    let records = host.records("account-a");
    assert_eq!(records.len(), 20);
    assert_eq!(records[0]["id"], ids[2]);
    assert_eq!(records[19]["id"], ids[21]);
    let error = peer
        .api(
            "api/candy/record",
            Some(json!({"account_id":"account-a","record_id":ids[0]})),
            &mut host,
        )
        .await
        .unwrap_err();
    assert_eq!(error.http_status, Some(400));
}

#[tokio::test]
async fn clear_removes_only_the_requested_accounts() {
    let mut peer = Peer::new().await;
    let mut host = Host::default();
    peer.run("account-a", "low", &mut host).await.unwrap();
    peer.run("account-b", "low", &mut host).await.unwrap();
    let cleared = peer
        .api(
            "api/candy/clear",
            Some(json!({"account_ids":["account-a","account-c"]})),
            &mut host,
        )
        .await
        .unwrap();
    assert_eq!(cleared, json!({"cleared":1,"skipped":[]}));
    assert!(host.records("account-a").is_empty());
    assert_eq!(host.records("account-b").len(), 1);
    let bad = peer
        .api(
            "api/candy/clear",
            Some(json!({"account_ids":["a\nb"]})),
            &mut host,
        )
        .await
        .unwrap_err();
    assert_eq!(bad.http_status, Some(400));
    assert_eq!(host.records("account-b").len(), 1);
}

#[tokio::test]
async fn history_survives_a_new_plugin_session() {
    let mut host = Host::default();
    let first = {
        let mut peer = Peer::new().await;
        peer.run("account-b", "high", &mut host).await.unwrap()
    };
    let mut peer = Peer::new().await;
    let boot = peer.api("api/bootstrap", None, &mut host).await.unwrap();
    assert_eq!(boot["accounts"][1]["records"][0], first["record"]);
}

#[tokio::test]
async fn repeated_account_cursor_fails_before_any_inference() {
    let mut peer = Peer::new().await;
    let mut host = Host {
        repeat_cursor: true,
        ..Host::default()
    };
    let error = peer
        .api("api/bootstrap", None, &mut host)
        .await
        .unwrap_err();
    assert!(error.message.contains("分页"));
    assert!(host.executions.is_empty());
}
