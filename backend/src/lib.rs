pub mod candy;
mod host;
mod management;

use gateway_plugin_sdk::{
    ErrorCode, PluginFault,
    client::{AuthorError, ComposedPlugin, PluginBuilder},
};
use std::{collections::BTreeSet, sync::Arc};
use tokio::sync::{Mutex, Semaphore};

/// 同时进行的作答请求上限；页面的并发设置也不超过这个值
pub(crate) const MAXIMUM_PARALLEL: usize = 8;

pub(crate) struct Service {
    state: Mutex<()>,
    execution: Semaphore,
    busy: std::sync::Mutex<BTreeSet<String>>,
}

/// 账号正在作答的进程内标记；调用结束或被宿主取消时随析构释放
pub(crate) struct Busy<'a> {
    service: &'a Service,
    account_id: String,
}
impl Service {
    pub(crate) fn claim(&self, account_id: &str) -> Result<Busy<'_>, PluginFault> {
        let mut busy = self.busy.lock().unwrap_or_else(|e| e.into_inner());
        if !busy.insert(account_id.into()) {
            return Err(PluginFault::new(
                ErrorCode::Conflict,
                "该账号已有测试在进行，请等待完成",
            ));
        }
        Ok(Busy {
            service: self,
            account_id: account_id.into(),
        })
    }
    pub(crate) fn is_busy(&self, account_id: &str) -> bool {
        self.busy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(account_id)
    }
}
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.service
            .busy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.account_id);
    }
}

pub fn plugin() -> Result<ComposedPlugin, AuthorError> {
    let service = Arc::new(Service {
        state: Mutex::new(()),
        execution: Semaphore::new(MAXIMUM_PARALLEL),
        busy: std::sync::Mutex::default(),
    });
    PluginBuilder::from_json(include_bytes!("../../plugin.json"))?
        .management(management::registration(), move |call| {
            let service = Arc::clone(&service);
            async move { management::handle(&service, call).await }
        })?
        .build()
}
pub(crate) fn invalid(message: impl Into<String>) -> PluginFault {
    PluginFault::new(ErrorCode::InvalidInput, message)
}
pub(crate) fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_nanos()
}
pub(crate) fn now_ms() -> u64 {
    (now_nanos() / 1_000_000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_answers_one_request_at_a_time() {
        let service = Service {
            state: Mutex::new(()),
            execution: Semaphore::new(MAXIMUM_PARALLEL),
            busy: std::sync::Mutex::default(),
        };
        let first = service.claim("a").unwrap();
        assert!(service.is_busy("a"));
        assert_eq!(service.claim("a").err().unwrap().code, ErrorCode::Conflict);
        let other = service.claim("b").unwrap();
        drop(first);
        assert!(!service.is_busy("a") && service.is_busy("b"));
        drop(other);
        assert!(service.claim("a").is_ok());
    }
}
