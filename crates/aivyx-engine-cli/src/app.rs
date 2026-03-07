use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aivyx", about = "Secure, privacy-first AI agent framework")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Initialize ~/.aivyx/ with master key, config, and audit log
    Init,

    /// First-run setup wizard — identity, provider, passphrase, persona, projects
    Genesis {
        /// Non-interactive mode with sensible defaults
        #[arg(long, alias = "non-interactive")]
        yes: bool,
    },

    /// Show current configuration and audit summary
    Status,

    /// Get or set configuration values
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// View or verify the audit log
    Audit {
        #[command(subcommand)]
        action: AuditAction,
    },

    /// Manage secrets in the encrypted store
    Secret {
        #[command(subcommand)]
        action: SecretAction,
    },

    /// Manage agent profiles
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },

    /// Run a single-turn agent task
    Run {
        /// Agent profile name
        agent: String,
        /// Prompt to send to the agent
        prompt: String,
    },

    /// Manage saved chat sessions
    Session {
        #[command(subcommand)]
        action: SessionAction,
    },

    /// Manage multi-agent teams
    Team {
        #[command(subcommand)]
        action: TeamAction,
    },

    /// Manage agent memories and knowledge triples
    Memory {
        #[command(subcommand)]
        action: MemoryAction,
    },

    /// HTTP API server management
    Server {
        #[command(subcommand)]
        action: ServerAction,
    },

    /// Manage MCP server connections and discover tools
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },

    /// Manage registered projects
    Project {
        #[command(subcommand)]
        action: ProjectAction,
    },

    /// Manage multi-step tasks (missions)
    Task {
        #[command(subcommand)]
        action: TaskAction,
    },

    /// Manage inbound communication channels
    Channel {
        #[command(subcommand)]
        action: ChannelAction,
    },

    /// Manage scheduled background tasks
    Schedule {
        #[command(subcommand)]
        action: ScheduleAction,
    },

    /// Manage installed SKILL.md skills
    Skill {
        #[command(subcommand)]
        action: SkillAction,
    },

    /// Create a backup of the aivyx data directory
    Backup {
        /// Output file path for the backup archive
        output: std::path::PathBuf,
    },

    /// Restore from a backup archive
    Restore {
        /// Path to the backup archive to restore from
        archive: std::path::PathBuf,
    },

    /// Generate or show a daily digest
    Digest {
        /// Agent profile name to use for LLM access
        #[arg(long)]
        agent: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Get a configuration value
    Get {
        /// Dotted key path (e.g., "autonomy.default_tier")
        key: String,
    },
    /// Set a configuration value
    Set {
        /// Dotted key path
        key: String,
        /// New value
        value: String,
    },
}

#[derive(Subcommand)]
pub enum AuditAction {
    /// Show recent audit entries
    Show {
        /// Number of entries to display
        #[arg(long, default_value = "10")]
        last: usize,
    },
    /// Verify audit log chain integrity
    Verify,
    /// Export audit log to JSON or CSV
    Export {
        /// Output format
        #[arg(long, default_value = "json")]
        format: String,
        /// Output file path (stdout if not specified)
        #[arg(long)]
        output: Option<std::path::PathBuf>,
    },
    /// Search audit log entries
    Search {
        /// Filter by event type (e.g., "SystemInit", "ToolExecuted")
        #[arg(long, short = 't')]
        r#type: Option<String>,
        /// Only include entries after this date (RFC 3339)
        #[arg(long)]
        from: Option<String>,
        /// Only include entries before this date (RFC 3339)
        #[arg(long)]
        to: Option<String>,
        /// Maximum number of results
        #[arg(long, default_value = "100")]
        limit: usize,
    },
    /// Prune old audit entries
    Prune {
        /// Remove entries before this date (RFC 3339)
        #[arg(long)]
        before: String,
    },
}

#[derive(Subcommand)]
pub enum SecretAction {
    /// Store a secret value (prompts for input)
    Set {
        /// Secret name (e.g., "claude_api_key")
        name: String,
    },
    /// Retrieve and display a masked secret
    Get {
        /// Secret name
        name: String,
    },
    /// List all secret names in the store
    List,
    /// Delete a secret from the store
    Delete {
        /// Secret name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum AgentAction {
    /// Create a new agent profile from template
    Create {
        /// Agent name
        name: String,
        /// Specialized role (assistant, coder, researcher, writer, ops)
        #[arg(long)]
        role: Option<String>,
    },
    /// List configured agents
    List,
    /// Show agent profile details
    Show {
        /// Agent name
        name: String,
    },
    /// Inspect or modify an agent's persona
    Persona {
        #[command(subcommand)]
        action: PersonaAction,
    },
}

#[derive(Subcommand)]
pub enum PersonaAction {
    /// Show the persona for an agent
    Show {
        /// Agent name
        name: String,
    },
    /// Set persona fields on an agent
    Set {
        /// Agent name
        name: String,
        /// Apply a preset (assistant, coder, researcher, writer, ops)
        #[arg(long)]
        preset: Option<String>,
        /// Formality dimension (0.0 to 1.0)
        #[arg(long)]
        formality: Option<f32>,
        /// Verbosity dimension (0.0 to 1.0)
        #[arg(long)]
        verbosity: Option<f32>,
        /// Warmth dimension (0.0 to 1.0)
        #[arg(long)]
        warmth: Option<f32>,
        /// Humor dimension (0.0 to 1.0)
        #[arg(long)]
        humor: Option<f32>,
        /// Confidence dimension (0.0 to 1.0)
        #[arg(long)]
        confidence: Option<f32>,
        /// Curiosity dimension (0.0 to 1.0)
        #[arg(long)]
        curiosity: Option<f32>,
        /// Tone descriptor (e.g., "friendly", "technical")
        #[arg(long)]
        tone: Option<String>,
        /// Whether to use emoji
        #[arg(long)]
        uses_emoji: Option<bool>,
    },
}

#[derive(Subcommand)]
pub enum SessionAction {
    /// List saved sessions
    List,
    /// Delete a saved session
    Delete {
        /// Session ID to delete
        id: String,
    },
}

#[derive(Subcommand)]
pub enum MemoryAction {
    /// List stored memories
    List {
        /// Filter by memory kind (e.g., "fact", "preference")
        #[arg(long)]
        kind: Option<String>,
    },
    /// Delete a memory by ID
    Delete {
        /// Memory ID to delete
        id: String,
    },
    /// Show memory subsystem statistics
    Stats,
    /// List knowledge triples
    Triples {
        /// Filter by subject
        #[arg(long)]
        subject: Option<String>,
        /// Filter by predicate
        #[arg(long)]
        predicate: Option<String>,
    },
    /// Manage the user profile
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
}

#[derive(Subcommand)]
pub enum ProfileAction {
    /// Show the current user profile
    Show,
    /// Extract a profile from accumulated facts via LLM
    Extract {
        /// Agent name to use for LLM access
        #[arg(long, default_value = "assistant")]
        agent: String,
    },
    /// Set a profile field directly (name, timezone)
    Set {
        /// Field to set (name, timezone)
        field: String,
        /// New value
        value: String,
    },
    /// Clear the user profile (reset to empty)
    Clear,
}

#[derive(Subcommand)]
pub enum ServerAction {
    /// Start the HTTP API server
    Start {
        /// Bind address (overrides config)
        #[arg(long)]
        bind: Option<String>,
        /// Port (overrides config, use 0 for random)
        #[arg(long)]
        port: Option<u16>,
        /// Emit JSON startup line to stdout for sidecar integration
        #[arg(long)]
        json_startup: bool,
        /// Read passphrase from stdin instead of interactive prompt
        #[arg(long)]
        stdin_passphrase: bool,
    },
    /// Manage bearer authentication tokens
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
}

#[derive(Subcommand)]
pub enum TokenAction {
    /// Generate a new bearer token
    Generate,
    /// Show the existing bearer token
    Show,
}

#[derive(Subcommand)]
pub enum TeamAction {
    /// Create a new team configuration
    Create {
        /// Team name
        name: String,
        /// Generate all 9 Nonagon roles
        #[arg(long)]
        nonagon: bool,
    },
    /// List configured teams
    List,
    /// Show team configuration
    Show {
        /// Team name
        name: String,
    },
    /// Run a team task
    Run {
        /// Team name
        name: String,
        /// Prompt to send to the team
        prompt: String,
        /// Resume from a saved session ID
        #[arg(long)]
        session: Option<String>,
    },
    /// Manage team sessions
    Session {
        #[command(subcommand)]
        action: TeamSessionAction,
    },
}

#[derive(Subcommand)]
pub enum TeamSessionAction {
    /// List saved sessions for a team
    List {
        /// Team name
        name: String,
    },
    /// Delete a saved team session
    Delete {
        /// Team name
        name: String,
        /// Session ID
        session: String,
    },
}

#[derive(Subcommand)]
pub enum TaskAction {
    /// Run a new multi-step mission
    Run {
        /// Agent profile name
        agent: String,
        /// High-level goal to accomplish
        goal: String,
    },
    /// List all missions
    List,
    /// Show details of a mission
    Show {
        /// Task ID
        id: String,
    },
    /// Resume an interrupted mission
    Resume {
        /// Task ID
        id: String,
    },
    /// Cancel a running mission
    Cancel {
        /// Task ID
        id: String,
    },
    /// Delete a completed/failed/cancelled mission
    Delete {
        /// Task ID
        id: String,
    },
}

#[derive(Subcommand)]
pub enum ProjectAction {
    /// Register a project directory
    Add {
        /// Path to the project root
        path: String,
        /// Custom project name (defaults to directory name)
        #[arg(long)]
        name: Option<String>,
        /// Primary language (auto-detected if not provided)
        #[arg(long)]
        language: Option<String>,
    },
    /// List registered projects
    List,
    /// Show project details
    Show {
        /// Project name
        name: String,
    },
    /// Remove a registered project
    Remove {
        /// Project name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum McpAction {
    /// List tools from an MCP server
    List {
        /// Stdio command to spawn (e.g., "npx -y @modelcontextprotocol/server-everything")
        #[arg(long)]
        command: Option<String>,
        /// SSE endpoint URL (alternative to --command)
        #[arg(long)]
        url: Option<String>,
    },
    /// Test an MCP server connection
    Test {
        /// Stdio command to spawn
        #[arg(long)]
        command: Option<String>,
        /// SSE endpoint URL
        #[arg(long)]
        url: Option<String>,
    },
    /// List available MCP server templates
    Templates,
}

#[derive(Subcommand)]
pub enum ChannelAction {
    /// List all configured channels
    List,
    /// Interactive wizard to add a new channel
    Add,
    /// Remove a channel by name
    Remove {
        /// Channel name
        name: String,
    },
    /// Test channel connection
    Test {
        /// Channel name
        name: String,
    },
}

#[derive(Subcommand)]
pub enum SkillAction {
    /// List installed skills
    List,
    /// Show full details of a skill
    Show {
        /// Skill name
        name: String,
    },
    /// Install a skill from a local directory
    Install {
        /// Path to the skill directory (must contain SKILL.md)
        path: String,
    },
    /// Remove an installed skill
    Remove {
        /// Skill name
        name: String,
    },
    /// Validate a SKILL.md file for correctness and best practices
    Validate {
        /// Path to the SKILL.md file or skill directory
        path: String,
    },
    /// Scaffold a new skill from an interactive wizard
    Create {
        /// Output directory (default: current directory)
        #[arg(long, short)]
        output: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum ScheduleAction {
    /// List all configured schedules
    List,
    /// Add a new scheduled task
    Add {
        /// Schedule name (slug-style, e.g., "morning-digest")
        name: String,
        /// Cron expression (5-field, e.g., "0 7 * * *")
        #[arg(long)]
        cron: String,
        /// Agent profile to run
        #[arg(long)]
        agent: String,
        /// Prompt to send to the agent
        #[arg(long)]
        prompt: String,
        /// Do not store results as notifications
        #[arg(long)]
        no_notify: bool,
    },
    /// Remove a scheduled task
    Remove {
        /// Schedule name
        name: String,
    },
    /// Enable a disabled schedule
    Enable {
        /// Schedule name
        name: String,
    },
    /// Disable a schedule without removing it
    Disable {
        /// Schedule name
        name: String,
    },
    /// Run a schedule entry immediately (bypass cron timing)
    RunNow {
        /// Schedule name
        name: String,
    },
    /// List pending notifications from background activity
    Notifications,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Cli {
        Cli::parse_from(std::iter::once("aivyx").chain(args.iter().copied()))
    }

    #[test]
    fn parse_init() {
        let cli = parse(&["init"]);
        assert!(matches!(cli.command, Command::Init));
    }

    #[test]
    fn parse_genesis() {
        let cli = parse(&["genesis"]);
        if let Command::Genesis { yes } = cli.command {
            assert!(!yes);
        } else {
            panic!("expected Genesis");
        }
    }

    #[test]
    fn parse_genesis_yes() {
        let cli = parse(&["genesis", "--yes"]);
        if let Command::Genesis { yes } = cli.command {
            assert!(yes);
        } else {
            panic!("expected Genesis --yes");
        }
    }

    #[test]
    fn parse_status() {
        let cli = parse(&["status"]);
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn parse_config_get_and_set() {
        let cli = parse(&["config", "get", "provider.model_id"]);
        assert!(matches!(
            cli.command,
            Command::Config {
                action: ConfigAction::Get { .. }
            }
        ));

        let cli = parse(&["config", "set", "provider.model_id", "gpt-4"]);
        if let Command::Config {
            action: ConfigAction::Set { key, value },
        } = cli.command
        {
            assert_eq!(key, "provider.model_id");
            assert_eq!(value, "gpt-4");
        } else {
            panic!("expected Config::Set");
        }
    }

    #[test]
    fn parse_run_with_agent_and_prompt() {
        let cli = parse(&["run", "assistant", "hello world"]);
        if let Command::Run { agent, prompt } = cli.command {
            assert_eq!(agent, "assistant");
            assert_eq!(prompt, "hello world");
        } else {
            panic!("expected Run");
        }
    }

    #[test]
    fn parse_agent_create_with_role() {
        let cli = parse(&["agent", "create", "my-bot", "--role", "coder"]);
        if let Command::Agent {
            action: AgentAction::Create { name, role },
        } = cli.command
        {
            assert_eq!(name, "my-bot");
            assert_eq!(role.as_deref(), Some("coder"));
        } else {
            panic!("expected Agent::Create");
        }
    }

    #[test]
    fn parse_agent_persona_show() {
        let cli = parse(&["agent", "persona", "show", "assistant"]);
        assert!(matches!(
            cli.command,
            Command::Agent {
                action: AgentAction::Persona {
                    action: PersonaAction::Show { .. }
                }
            }
        ));
    }

    #[test]
    fn parse_agent_persona_set_with_preset() {
        let cli = parse(&[
            "agent", "persona", "set", "mybot", "--preset", "coder", "--warmth", "0.8",
        ]);
        if let Command::Agent {
            action:
                AgentAction::Persona {
                    action:
                        PersonaAction::Set {
                            name,
                            preset,
                            warmth,
                            ..
                        },
                },
        } = cli.command
        {
            assert_eq!(name, "mybot");
            assert_eq!(preset.as_deref(), Some("coder"));
            assert_eq!(warmth, Some(0.8));
        } else {
            panic!("expected Agent::Persona::Set");
        }
    }

    #[test]
    fn parse_task_run() {
        let cli = parse(&["task", "run", "assistant", "research quantum computing"]);
        if let Command::Task {
            action: TaskAction::Run { agent, goal },
        } = cli.command
        {
            assert_eq!(agent, "assistant");
            assert_eq!(goal, "research quantum computing");
        } else {
            panic!("expected Task::Run");
        }
    }

    #[test]
    fn parse_task_subcommands() {
        assert!(matches!(
            parse(&["task", "list"]).command,
            Command::Task {
                action: TaskAction::List
            }
        ));
        assert!(matches!(
            parse(&["task", "show", "abc"]).command,
            Command::Task {
                action: TaskAction::Show { .. }
            }
        ));
        assert!(matches!(
            parse(&["task", "resume", "abc"]).command,
            Command::Task {
                action: TaskAction::Resume { .. }
            }
        ));
        assert!(matches!(
            parse(&["task", "cancel", "abc"]).command,
            Command::Task {
                action: TaskAction::Cancel { .. }
            }
        ));
        assert!(matches!(
            parse(&["task", "delete", "abc"]).command,
            Command::Task {
                action: TaskAction::Delete { .. }
            }
        ));
    }

    #[test]
    fn parse_project_add() {
        let cli = parse(&["project", "add", "/home/user/myapp"]);
        if let Command::Project {
            action:
                ProjectAction::Add {
                    path,
                    name,
                    language,
                },
        } = cli.command
        {
            assert_eq!(path, "/home/user/myapp");
            assert!(name.is_none());
            assert!(language.is_none());
        } else {
            panic!("expected Project::Add");
        }
    }

    #[test]
    fn parse_project_add_with_options() {
        let cli = parse(&[
            "project",
            "add",
            "/tmp/proj",
            "--name",
            "my-proj",
            "--language",
            "Rust",
        ]);
        if let Command::Project {
            action:
                ProjectAction::Add {
                    path,
                    name,
                    language,
                },
        } = cli.command
        {
            assert_eq!(path, "/tmp/proj");
            assert_eq!(name.as_deref(), Some("my-proj"));
            assert_eq!(language.as_deref(), Some("Rust"));
        } else {
            panic!("expected Project::Add");
        }
    }

    #[test]
    fn parse_project_subcommands() {
        assert!(matches!(
            parse(&["project", "list"]).command,
            Command::Project {
                action: ProjectAction::List
            }
        ));
        assert!(matches!(
            parse(&["project", "show", "foo"]).command,
            Command::Project {
                action: ProjectAction::Show { .. }
            }
        ));
        assert!(matches!(
            parse(&["project", "remove", "bar"]).command,
            Command::Project {
                action: ProjectAction::Remove { .. }
            }
        ));
    }

    #[test]
    fn parse_mcp_list_with_command() {
        let cli = parse(&["mcp", "list", "--command", "npx -y @mcp/server"]);
        if let Command::Mcp {
            action: McpAction::List { command, url },
        } = cli.command
        {
            assert_eq!(command.as_deref(), Some("npx -y @mcp/server"));
            assert!(url.is_none());
        } else {
            panic!("expected Mcp::List");
        }
    }

    #[test]
    fn parse_memory_profile_subcommands() {
        assert!(matches!(
            parse(&["memory", "profile", "show"]).command,
            Command::Memory {
                action: MemoryAction::Profile {
                    action: ProfileAction::Show
                }
            }
        ));
        assert!(matches!(
            parse(&["memory", "profile", "clear"]).command,
            Command::Memory {
                action: MemoryAction::Profile {
                    action: ProfileAction::Clear
                }
            }
        ));
    }

    #[test]
    fn parse_server_start_with_flags() {
        let cli = parse(&[
            "server",
            "start",
            "--port",
            "8080",
            "--json-startup",
            "--stdin-passphrase",
        ]);
        if let Command::Server {
            action:
                ServerAction::Start {
                    bind,
                    port,
                    json_startup,
                    stdin_passphrase,
                },
        } = cli.command
        {
            assert!(bind.is_none());
            assert_eq!(port, Some(8080));
            assert!(json_startup);
            assert!(stdin_passphrase);
        } else {
            panic!("expected Server::Start");
        }
    }

    #[test]
    fn parse_audit_show_default_last() {
        let cli = parse(&["audit", "show"]);
        if let Command::Audit {
            action: AuditAction::Show { last },
        } = cli.command
        {
            assert_eq!(last, 10); // default
        } else {
            panic!("expected Audit::Show");
        }
    }

    #[test]
    fn parse_schedule_list() {
        assert!(matches!(
            parse(&["schedule", "list"]).command,
            Command::Schedule {
                action: ScheduleAction::List
            }
        ));
    }

    #[test]
    fn parse_schedule_add() {
        let cli = parse(&[
            "schedule",
            "add",
            "morning-digest",
            "--cron",
            "0 7 * * *",
            "--agent",
            "assistant",
            "--prompt",
            "Generate morning digest",
        ]);
        if let Command::Schedule {
            action:
                ScheduleAction::Add {
                    name,
                    cron,
                    agent,
                    prompt,
                    no_notify,
                },
        } = cli.command
        {
            assert_eq!(name, "morning-digest");
            assert_eq!(cron, "0 7 * * *");
            assert_eq!(agent, "assistant");
            assert_eq!(prompt, "Generate morning digest");
            assert!(!no_notify);
        } else {
            panic!("expected Schedule::Add");
        }
    }

    #[test]
    fn parse_schedule_subcommands() {
        assert!(matches!(
            parse(&["schedule", "remove", "foo"]).command,
            Command::Schedule {
                action: ScheduleAction::Remove { .. }
            }
        ));
        assert!(matches!(
            parse(&["schedule", "enable", "foo"]).command,
            Command::Schedule {
                action: ScheduleAction::Enable { .. }
            }
        ));
        assert!(matches!(
            parse(&["schedule", "disable", "foo"]).command,
            Command::Schedule {
                action: ScheduleAction::Disable { .. }
            }
        ));
        assert!(matches!(
            parse(&["schedule", "run-now", "foo"]).command,
            Command::Schedule {
                action: ScheduleAction::RunNow { .. }
            }
        ));
        assert!(matches!(
            parse(&["schedule", "notifications"]).command,
            Command::Schedule {
                action: ScheduleAction::Notifications
            }
        ));
    }

    #[test]
    fn parse_skill_list() {
        assert!(matches!(
            parse(&["skill", "list"]).command,
            Command::Skill {
                action: SkillAction::List
            }
        ));
    }

    #[test]
    fn parse_skill_show() {
        let cli = parse(&["skill", "show", "webapp-testing"]);
        if let Command::Skill {
            action: SkillAction::Show { name },
        } = cli.command
        {
            assert_eq!(name, "webapp-testing");
        } else {
            panic!("expected Skill::Show");
        }
    }

    #[test]
    fn parse_skill_install() {
        let cli = parse(&["skill", "install", "/tmp/my-skill"]);
        if let Command::Skill {
            action: SkillAction::Install { path },
        } = cli.command
        {
            assert_eq!(path, "/tmp/my-skill");
        } else {
            panic!("expected Skill::Install");
        }
    }

    #[test]
    fn parse_skill_remove() {
        let cli = parse(&["skill", "remove", "old-skill"]);
        if let Command::Skill {
            action: SkillAction::Remove { name },
        } = cli.command
        {
            assert_eq!(name, "old-skill");
        } else {
            panic!("expected Skill::Remove");
        }
    }

    #[test]
    fn parse_digest() {
        let cli = parse(&["digest"]);
        if let Command::Digest { agent } = cli.command {
            assert!(agent.is_none());
        } else {
            panic!("expected Digest");
        }
    }

    #[test]
    fn parse_digest_with_agent() {
        let cli = parse(&["digest", "--agent", "researcher"]);
        if let Command::Digest { agent } = cli.command {
            assert_eq!(agent.as_deref(), Some("researcher"));
        } else {
            panic!("expected Digest");
        }
    }

    #[test]
    fn parse_team_run_with_session() {
        let cli = parse(&[
            "team",
            "run",
            "dev-team",
            "build it",
            "--session",
            "abc-123",
        ]);
        if let Command::Team {
            action:
                TeamAction::Run {
                    name,
                    prompt,
                    session,
                },
        } = cli.command
        {
            assert_eq!(name, "dev-team");
            assert_eq!(prompt, "build it");
            assert_eq!(session.as_deref(), Some("abc-123"));
        } else {
            panic!("expected Team::Run");
        }
    }

    #[test]
    fn parse_team_run_without_session() {
        let cli = parse(&["team", "run", "dev-team", "build it"]);
        if let Command::Team {
            action:
                TeamAction::Run {
                    name,
                    prompt,
                    session,
                },
        } = cli.command
        {
            assert_eq!(name, "dev-team");
            assert_eq!(prompt, "build it");
            assert!(session.is_none());
        } else {
            panic!("expected Team::Run");
        }
    }

    #[test]
    fn parse_team_session_list() {
        let cli = parse(&["team", "session", "list", "dev-team"]);
        assert!(matches!(
            cli.command,
            Command::Team {
                action: TeamAction::Session {
                    action: TeamSessionAction::List { .. }
                }
            }
        ));
    }

    #[test]
    fn parse_team_session_delete() {
        let cli = parse(&["team", "session", "delete", "dev-team", "sess-1"]);
        if let Command::Team {
            action:
                TeamAction::Session {
                    action: TeamSessionAction::Delete { name, session },
                },
        } = cli.command
        {
            assert_eq!(name, "dev-team");
            assert_eq!(session, "sess-1");
        } else {
            panic!("expected Team::Session::Delete");
        }
    }
}
