use std::collections::HashMap;

use aivyx_config::{McpServerConfig, McpTransport};
use aivyx_core::Result;
use aivyx_mcp::McpClient;

use crate::output;

/// List tools available from an MCP server.
pub async fn list(command: Option<&str>, url: Option<&str>) -> Result<()> {
    let config = build_config(command, url)?;

    output::header(&format!("Connecting to MCP server: {}", config.name));

    let client = McpClient::connect(&config).await?;
    let init = client.initialize().await?;

    output::success(&format!(
        "Connected to {} v{} (protocol {})",
        init.server_info.name,
        init.server_info.version.as_deref().unwrap_or("?"),
        init.protocol_version,
    ));

    let tools = client.list_tools().await?;
    println!();
    output::header(&format!("Available tools ({})", tools.len()));

    for tool in &tools {
        println!(
            "  {} — {}",
            tool.name,
            tool.description.as_deref().unwrap_or("(no description)")
        );
    }

    println!();
    client.shutdown().await?;
    Ok(())
}

/// Test an MCP server connection by connecting, initializing, and listing tools.
pub async fn test(command: Option<&str>, url: Option<&str>) -> Result<()> {
    let config = build_config(command, url)?;

    output::header(&format!("Testing MCP server: {}", config.name));

    // Step 1: Connect
    output::kv("Connect", "...");
    let client = McpClient::connect(&config).await?;
    output::success("Connected");

    // Step 2: Initialize
    output::kv("Initialize", "...");
    let init = client.initialize().await?;
    output::success(&format!(
        "Server: {} v{} (protocol: {})",
        init.server_info.name,
        init.server_info.version.as_deref().unwrap_or("?"),
        init.protocol_version,
    ));

    // Step 3: List tools
    output::kv("Tools/list", "...");
    let tools = client.list_tools().await?;
    output::success(&format!("{} tools discovered", tools.len()));

    for tool in &tools {
        println!(
            "    {} — {}",
            tool.name,
            tool.description.as_deref().unwrap_or("(no description)")
        );
    }

    // Step 4: Shutdown
    client.shutdown().await?;
    output::success("Shutdown complete");

    println!();
    output::header("Result");
    output::success("All checks passed");
    println!();

    Ok(())
}

/// Build an McpServerConfig from the CLI arguments.
fn build_config(command: Option<&str>, url: Option<&str>) -> Result<McpServerConfig> {
    match (command, url) {
        (Some(cmd), _) => {
            // Parse the command string into command + args.
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if parts.is_empty() {
                return Err(aivyx_core::AivyxError::Config(
                    "MCP command cannot be empty".into(),
                ));
            }
            Ok(McpServerConfig {
                name: parts[0].to_string(),
                transport: McpTransport::Stdio {
                    command: parts[0].to_string(),
                    args: parts[1..].iter().map(|s| s.to_string()).collect(),
                },
                env: HashMap::new(),
                timeout_secs: 30,
            })
        }
        (None, Some(url)) => Ok(McpServerConfig {
            name: url.to_string(),
            transport: McpTransport::Sse {
                url: url.to_string(),
            },
            env: HashMap::new(),
            timeout_secs: 30,
        }),
        (None, None) => Err(aivyx_core::AivyxError::Config(
            "specify either --command or --url for the MCP server".into(),
        )),
    }
}

/// List available MCP server templates grouped by category.
pub fn templates() -> Result<()> {
    use aivyx_server::routes::templates::TEMPLATES;

    output::header("Available MCP Server Templates");
    println!();

    let mut current_category = "";
    for t in TEMPLATES.iter() {
        if t.category != current_category {
            if !current_category.is_empty() {
                println!();
            }
            current_category = t.category;
            println!("  {} {}", t.icon, t.category);
            println!("  {}", "─".repeat(40));
        }
        let key_marker = if t.requires_api_key { " 🔑" } else { "" };
        println!("    {} {}{}", t.id, t.description, key_marker);
    }

    println!();
    output::kv("Total", &format!("{} templates", TEMPLATES.len()));
    println!("  🔑 = requires API key");
    println!();
    output::success("Install via: aivyx plugin install <template-id>");
    println!();

    Ok(())
}
