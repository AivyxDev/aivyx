use aivyx_agent::AgentSession;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::Result;
use aivyx_crypto::MasterKey;

use crate::channel::CliChannel;
use crate::output;

pub async fn run(agent_name: &str, prompt: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let config = AivyxConfig::load(dirs.config_path())?;

    let session = AgentSession::new(dirs, config, master_key);
    let cwd = std::env::current_dir().ok();
    let mut agent = session
        .create_agent_with_context(agent_name, cwd.as_deref())
        .await?;

    output::header(&format!("Running agent: {agent_name}"));
    println!();

    let (token_tx, mut token_rx) = tokio::sync::mpsc::channel::<String>(64);

    let print_handle = tokio::spawn(async move {
        use std::io::Write;
        while let Some(token) = token_rx.recv().await {
            print!("{token}");
            std::io::stdout().flush().ok();
        }
        println!();
    });

    let cli_channel = CliChannel;
    let _result = agent
        .turn_stream(prompt, Some(&cli_channel), token_tx, None)
        .await?;
    let _ = print_handle.await;

    println!();
    output::kv(
        "Estimated cost",
        &format!("${:.4}", agent.current_cost_usd()),
    );
    println!();

    Ok(())
}

fn check_initialized(dirs: &AivyxDirs) -> Result<()> {
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Err(aivyx_core::AivyxError::NotInitialized(
            "run `aivyx genesis` first".into(),
        ));
    }
    Ok(())
}

fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}
