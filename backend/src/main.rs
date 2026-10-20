#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    use gateway_plugin_sdk::client::{PluginSession, SessionConfig};
    let session = PluginSession::accept(
        tokio::io::stdin(),
        tokio::io::stdout(),
        SessionConfig::default(),
    )
    .await?;
    if session.handshake().plugin_id != "local.model-health" {
        return Err("插件身份不匹配".into());
    }
    session.run(codex_model_health_plugin::plugin()?).await?;
    Ok(())
}
