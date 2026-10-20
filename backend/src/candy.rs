//! 糖果题：题目、请求体、判分、Responses 流解析与每账号测试记录

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// 题目原文来自 haowang02/codex-candy-eval（MIT），正确答案为 21
pub const PROMPT: &str = "不使用任何外部工具回答以下问题：

在一个黑色的袋子里放有三种口味的糖果，每种糖果有两种不同的形状（圆形和五角星形，不同的形状靠手感可以分辨）。现已知不同口味的糖和不同形状的数量统计如下表。参赛者需要在活动前决定摸出的糖果数目，那么，最少取出多少个糖果才能保证手中同时拥有不同形状的苹果味和桃子味的糖？（同时手中有圆形苹果味匹配五角星桃子味糖果，或者有圆形桃子味匹配五角星苹果味糖果都满足要求）

        苹果味  桃子味  西瓜味
圆形       7      9      8
五角星形   7      6      4
";
pub const ANSWER: &str = "21";
/// `none` 不发送 reasoning，由主服务与上游决定推理强度
pub const EFFORTS: [&str; 6] = ["none", "low", "medium", "high", "xhigh", "max"];
pub const HISTORY_LIMIT: usize = 20;
const ANSWER_BYTES: usize = 3000;
const ERROR_BYTES: usize = 512;
const STREAM_TEXT_BYTES: usize = 1024 * 1024;
pub const INTERRUPTED: &str = "请求中断，结果未知；不计入正确率";

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Correct,
    Wrong,
    Failed,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, Debug)]
pub struct Usage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    fn from_response(usage: &Value) -> Self {
        Self {
            input_tokens: usage["input_tokens"].as_u64(),
            output_tokens: usage["output_tokens"].as_u64(),
            reasoning_tokens: usage["output_tokens_details"]["reasoning_tokens"].as_u64(),
        }
    }
}

/// 一次作答；只有 correct / wrong 计入正确率，出错与中断都不参与评分
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub struct Record {
    pub id: String,
    pub time: u64,
    pub model: String,
    pub effort: String,
    pub status: Status,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub answer_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(flatten)]
    pub usage: Usage,
}

impl Record {
    pub fn pending(id: String, time: u64, model: &str, effort: &str) -> Self {
        Self {
            id,
            time,
            model: model.into(),
            effort: effort.into(),
            status: Status::Pending,
            answer: None,
            answer_truncated: false,
            error: None,
            request_id: None,
            duration_ms: None,
            usage: Usage::default(),
        }
    }

    /// 按完整回答判分；保存的回答摘录有上限，不影响判分
    pub fn graded(mut self, request_id: Option<String>, result: Result<Answer, String>) -> Self {
        self.request_id = request_id.map(|id| excerpt(&id, 128));
        match result {
            Ok(answer) => {
                self.usage = answer.usage;
                self.status = if has_standalone_answer(&answer.text) {
                    Status::Correct
                } else {
                    Status::Wrong
                };
                self.answer_truncated = answer.text.len() > ANSWER_BYTES;
                self.answer = Some(excerpt(&answer.text, ANSWER_BYTES));
            }
            Err(error) => {
                self.status = Status::Failed;
                self.error = Some(excerpt(&error, ERROR_BYTES));
            }
        }
        self
    }

    /// 列表只带判分与用量；回答与错误详情按需读取，避免账号多时超过管理接口的 1 MiB 上限
    pub fn summary(&self) -> Value {
        let mut value = json!({"id":self.id,"time":self.time,"model":self.model,"effort":self.effort,"status":self.status});
        for (key, field) in [
            ("duration_ms", self.duration_ms),
            ("input_tokens", self.usage.input_tokens),
            ("output_tokens", self.usage.output_tokens),
            ("reasoning_tokens", self.usage.reasoning_tokens),
        ] {
            if let Some(field) = field {
                value[key] = json!(field);
            }
        }
        value
    }
}

/// 一个账号的测试记录，按时间从旧到新，最多保留 `HISTORY_LIMIT` 条
#[derive(Clone, PartialEq, Serialize, Deserialize, Debug)]
pub struct History {
    pub account_id: String,
    pub records: Vec<Record>,
}

impl History {
    pub fn new(account_id: &str) -> Self {
        Self {
            account_id: account_id.into(),
            records: vec![],
        }
    }

    /// 发送前已写入的 pending 记录若没有对应的进行中请求，只能按中断计入，不能变成免费重试
    pub fn recover(&mut self) -> bool {
        let mut changed = false;
        for record in &mut self.records {
            if record.status == Status::Pending {
                record.status = Status::Failed;
                record.error = Some(INTERRUPTED.into());
                changed = true;
            }
        }
        changed
    }

    pub fn push(&mut self, record: Record) {
        self.records.push(record);
        let overflow = self.records.len().saturating_sub(HISTORY_LIMIT);
        self.records.drain(..overflow);
    }

    pub fn finish(&mut self, record: Record) {
        match self.records.iter_mut().find(|r| r.id == record.id) {
            Some(slot) => *slot = record,
            None => self.push(record),
        }
    }

    pub fn summaries(&self, limit: usize) -> Vec<Value> {
        let skip = self.records.len().saturating_sub(limit);
        self.records[skip..].iter().map(Record::summary).collect()
    }
}

pub fn payload(model: &str, effort: &str) -> Value {
    let mut body = json!({"model":model,"stream":true,"store":false,"input":[{"role":"user","content":PROMPT}]});
    if effort != "none" {
        body["reasoning"] = json!({ "effort": effort });
    }
    body
}

/// 回答中出现前后都不紧邻数字的 21 即为答对，与 codex-candy-eval 的判定一致
pub fn has_standalone_answer(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.windows(2).enumerate().any(|(i, pair)| {
        pair == ANSWER.as_bytes()
            && (i == 0 || !bytes[i - 1].is_ascii_digit())
            && bytes.get(i + 2).is_none_or(|b| !b.is_ascii_digit())
    })
}

pub fn excerpt(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

#[derive(Debug, PartialEq)]
pub struct Answer {
    pub text: String,
    pub usage: Usage,
}

/// 只解析 Responses 流中判分需要的事件；以 response.completed 的最终输出为准
#[derive(Default)]
pub struct ResponseParser {
    parts: BTreeMap<(u64, u64), String>,
    streamed: usize,
    completed: Option<(String, Usage)>,
    failure: Option<String>,
    refused: bool,
}

impl ResponseParser {
    pub fn feed_sse(&mut self, frame: &str) -> Result<(), String> {
        let data = frame
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(|line| line.strip_prefix(' ').unwrap_or(line))
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() || data == "[DONE]" {
            return Ok(());
        }
        let value =
            serde_json::from_str(&data).map_err(|_| "模型流事件不是有效 JSON".to_string())?;
        self.feed(&value)
    }

    pub fn feed(&mut self, value: &Value) -> Result<(), String> {
        if value["type"] == "error" || value.get("error").is_some_and(|e| !e.is_null()) {
            let error = if value["error"].is_object() {
                &value["error"]
            } else {
                value
            };
            self.failure = Some(upstream_reason(error));
            return Ok(());
        }
        match value["type"].as_str().unwrap_or_default() {
            "response.output_text.delta" => {
                let delta = value["delta"].as_str().unwrap_or_default();
                self.streamed += delta.len();
                if self.streamed > STREAM_TEXT_BYTES {
                    return Err("回答超过 1 MiB".into());
                }
                let key = (
                    value["output_index"].as_u64().unwrap_or(0),
                    value["content_index"].as_u64().unwrap_or(0),
                );
                self.parts.entry(key).or_default().push_str(delta);
            }
            "response.refusal.delta" | "response.refusal.done" => self.refused = true,
            "response.failed" | "response.incomplete" => {
                let response = &value["response"];
                let reason = &response["incomplete_details"]["reason"];
                self.failure = Some(match reason.as_str() {
                    Some("max_output_tokens") => "回答达到输出长度上限".into(),
                    Some("content_filter") => "回答被内容过滤".into(),
                    Some(other) => format!("回答未完成：{other}"),
                    None => upstream_reason(&response["error"]),
                });
            }
            "response.completed" => {
                let response = &value["response"];
                let mut text = String::new();
                for item in response["output"].as_array().into_iter().flatten() {
                    for part in item["content"].as_array().into_iter().flatten() {
                        match part["type"].as_str() {
                            Some("output_text") => {
                                text.push_str(part["text"].as_str().unwrap_or_default())
                            }
                            Some("refusal") => self.refused = true,
                            _ => {}
                        }
                    }
                }
                // 部分上游在完成事件里省略 output，此时才回退到增量文本
                if response["output"].as_array().is_none_or(Vec::is_empty) {
                    text = self.parts.values().map(String::as_str).collect();
                }
                self.completed = Some((text, Usage::from_response(&response["usage"])));
            }
            _ => {}
        }
        Ok(())
    }

    pub fn finish(self) -> Result<Answer, String> {
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        if self.refused {
            return Err("模型拒绝回答".into());
        }
        let (text, usage) = self
            .completed
            .ok_or_else(|| "响应流在完成前结束".to_string())?;
        if text.trim().is_empty() {
            return Err("模型没有返回文本".into());
        }
        Ok(Answer { text, usage })
    }
}

fn upstream_reason(error: &Value) -> String {
    let message = error["message"]
        .as_str()
        .or_else(|| error["code"].as_str())
        .unwrap_or("未知错误");
    format!("上游返回错误：{message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed(text: &str) -> Value {
        json!({"type":"response.completed","response":{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":text}]}],"usage":{"input_tokens":300,"output_tokens":40,"output_tokens_details":{"reasoning_tokens":12}}}})
    }

    #[test]
    fn standalone_answer_ignores_neighbouring_digits() {
        for text in ["21", "答案是 **21**。", "最少 21 个", "(21)", "21.", "x21y"] {
            assert!(has_standalone_answer(text), "{text}");
        }
        for text in ["", "2", "121", "210", "2021 年", "答案是 20", "２１"] {
            assert!(!has_standalone_answer(text), "{text}");
        }
        assert!(has_standalone_answer("先算 121，再得 21"));
    }

    #[test]
    fn completed_output_is_graded_with_usage() {
        let mut parser = ResponseParser::default();
        parser
            .feed(&json!({"type":"response.output_text.delta","delta":"draft"}))
            .unwrap();
        parser.feed(&completed("最少需要 21 个")).unwrap();
        let answer = parser.finish().unwrap();
        assert_eq!(answer.text, "最少需要 21 个");
        assert_eq!(
            answer.usage,
            Usage {
                input_tokens: Some(300),
                output_tokens: Some(40),
                reasoning_tokens: Some(12)
            }
        );
        let record =
            Record::pending("1".into(), 0, "m", "low").graded(Some("r".into()), Ok(answer));
        assert_eq!(record.status, Status::Correct);
        assert_eq!(record.summary()["reasoning_tokens"], 12);
        assert!(record.summary().get("answer").is_none());
    }

    #[test]
    fn missing_output_falls_back_to_deltas_from_raw_sse() {
        let mut parser = ResponseParser::default();
        for frame in [
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"答案：\"}\n\n",
            "data: {\"type\":\"response.output_text.delta\",\"delta\":\"20\"}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n",
            "data: [DONE]\n\n",
        ] {
            parser.feed_sse(frame).unwrap();
        }
        let record = Record::pending("1".into(), 0, "m", "none").graded(None, parser.finish());
        assert_eq!(record.status, Status::Wrong);
        assert_eq!(record.answer.as_deref(), Some("答案：20"));
    }

    #[test]
    fn failures_are_not_graded() {
        let cases = [
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),
            json!({"type":"error","code":"rate_limit_exceeded","message":"slow down"}),
            json!({"type":"response.refusal.delta","delta":"no"}),
            completed("   "),
        ];
        for event in cases {
            let mut parser = ResponseParser::default();
            parser.feed(&event).unwrap();
            let record = Record::pending("1".into(), 0, "m", "low").graded(None, parser.finish());
            assert_eq!(record.status, Status::Failed, "{event}");
            assert!(record.error.is_some());
        }
        let record = Record::pending("1".into(), 0, "m", "low")
            .graded(None, ResponseParser::default().finish());
        assert_eq!(record.error.as_deref(), Some("响应流在完成前结束"));
    }

    #[test]
    fn long_answers_are_graded_in_full_but_stored_as_excerpt() {
        let text = format!("{}21", "长".repeat(2000));
        let record = Record::pending("1".into(), 0, "m", "low").graded(
            None,
            Ok(Answer {
                text,
                usage: Usage::default(),
            }),
        );
        assert_eq!(record.status, Status::Correct);
        assert!(record.answer_truncated);
        assert!(record.answer.unwrap().len() <= ANSWER_BYTES);
    }

    #[test]
    fn history_keeps_latest_records_and_charges_interruptions() {
        let mut history = History::new("a");
        for i in 0..25 {
            history.push(Record::pending(i.to_string(), i, "m", "low"));
            history.recover();
        }
        assert_eq!(history.records.len(), HISTORY_LIMIT);
        assert_eq!(history.records[0].id, "5");
        assert!(
            history
                .records
                .iter()
                .all(|r| r.status == Status::Failed && r.error.as_deref() == Some(INTERRUPTED))
        );
        let mut done = Record::pending("24".into(), 24, "m", "low");
        done.status = Status::Correct;
        history.finish(done.clone());
        assert_eq!(history.records.last(), Some(&done));
        assert_eq!(history.summaries(3).len(), 3);
    }

    #[test]
    fn effort_none_leaves_reasoning_to_the_host() {
        assert!(payload("m", "none").get("reasoning").is_none());
        let body = payload("m", "xhigh");
        assert_eq!(body["reasoning"]["effort"], "xhigh");
        assert_eq!(body["input"][0]["content"], PROMPT);
        assert_eq!(body["store"], false);
    }
}
