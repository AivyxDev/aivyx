mod app;
mod channel;
mod cmd;
mod output;
mod unlock;

use clap::Parser;

#[tokio::main]
async fn main() {
    let env_filter = tracing_subscriber::EnvFilter::from_default_env();

    if std::env::var("AIVYX_LOG_FORMAT").is_ok_and(|v| v == "json") {
        tracing_subscriber::fmt()
            .json()
            .with_writer(std::io::stderr)
            .with_env_filter(env_filter)
            .with_span_events(tracing_subscriber::fmt::format::FmtSpan::CLOSE)
            .init();
    } else {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_env_filter(env_filter)
            .init();
    }

    let cli = app::Cli::parse();

    let result = match cli.command {
        app::Command::Init => cmd::genesis::run(true).await,
        app::Command::Genesis { yes } => cmd::genesis::run(yes).await,
        app::Command::Status => cmd::status::run(),
        app::Command::Config { action } => match action {
            app::ConfigAction::Get { key } => cmd::config::get(&key),
            app::ConfigAction::Set { key, value } => cmd::config::set(&key, &value),
        },
        app::Command::Audit { action } => match action {
            app::AuditAction::Show { last } => cmd::audit::show(last),
            app::AuditAction::Verify => cmd::audit::verify(),
            app::AuditAction::Export { format, output } => {
                cmd::audit::export(&format, output.as_deref())
            }
            app::AuditAction::Search {
                r#type,
                from,
                to,
                limit,
            } => cmd::audit::search(r#type.as_deref(), from.as_deref(), to.as_deref(), limit),
            app::AuditAction::Prune { before } => cmd::audit::prune(&before),
        },
        app::Command::Secret { action } => match action {
            app::SecretAction::Set { name } => cmd::secret::set(&name),
            app::SecretAction::Get { name } => cmd::secret::get(&name),
            app::SecretAction::List => cmd::secret::list(),
            app::SecretAction::Delete { name } => cmd::secret::delete(&name),
        },
        app::Command::Agent { action } => match action {
            app::AgentAction::Create { name, role } => cmd::agent::create(&name, role.as_deref()),
            app::AgentAction::List => cmd::agent::list(),
            app::AgentAction::Show { name } => cmd::agent::show(&name),
            app::AgentAction::Persona { action } => match action {
                app::PersonaAction::Show { name } => cmd::agent::persona_show(&name),
                app::PersonaAction::Set {
                    name,
                    preset,
                    formality,
                    verbosity,
                    warmth,
                    humor,
                    confidence,
                    curiosity,
                    tone,
                    uses_emoji,
                } => cmd::agent::persona_set(
                    &name,
                    preset.as_deref(),
                    formality,
                    verbosity,
                    warmth,
                    humor,
                    confidence,
                    curiosity,
                    tone.as_deref(),
                    uses_emoji,
                ),
            },
        },
        app::Command::Run { agent, prompt } => cmd::run::run(&agent, &prompt).await,

        app::Command::Session { action } => match action {
            app::SessionAction::List => cmd::session::list(),
            app::SessionAction::Delete { id } => cmd::session::delete(&id),
        },
        app::Command::Memory { action } => match action {
            app::MemoryAction::List { kind } => cmd::memory::list(kind.as_deref()),
            app::MemoryAction::Delete { id } => cmd::memory::delete(&id),
            app::MemoryAction::Stats => cmd::memory::stats(),
            app::MemoryAction::Triples { subject, predicate } => {
                cmd::memory::triples(subject.as_deref(), predicate.as_deref())
            }
            app::MemoryAction::Profile { action } => match action {
                app::ProfileAction::Show => cmd::memory::profile_show(),
                app::ProfileAction::Extract { agent } => cmd::memory::profile_extract(&agent).await,
                app::ProfileAction::Set { field, value } => {
                    cmd::memory::profile_set(&field, &value)
                }
                app::ProfileAction::Clear => cmd::memory::profile_clear(),
            },
        },

        app::Command::Server { action } => match action {
            app::ServerAction::Start {
                bind,
                port,
                json_startup,
                stdin_passphrase,
            } => cmd::server::start(bind.as_deref(), port, json_startup, stdin_passphrase).await,
            app::ServerAction::Token { action } => match action {
                app::TokenAction::Generate => cmd::server::token_generate(),
                app::TokenAction::Show => cmd::server::token_show(),
            },
        },
        app::Command::Team { action } => match action {
            app::TeamAction::Create { name, nonagon } => cmd::team::create(&name, nonagon),
            app::TeamAction::List => cmd::team::list(),
            app::TeamAction::Show { name } => cmd::team::show(&name),
            app::TeamAction::Run {
                name,
                prompt,
                session,
            } => cmd::team::run(&name, &prompt, session.as_deref()).await,
            app::TeamAction::Session { action } => match action {
                app::TeamSessionAction::List { name } => cmd::team::session_list(&name),
                app::TeamSessionAction::Delete { name, session } => {
                    cmd::team::session_delete(&name, &session)
                }
            },
        },
        app::Command::Mcp { action } => match action {
            app::McpAction::List { command, url } => {
                cmd::mcp::list(command.as_deref(), url.as_deref()).await
            }
            app::McpAction::Test { command, url } => {
                cmd::mcp::test(command.as_deref(), url.as_deref()).await
            }
            app::McpAction::Templates => cmd::mcp::templates(),
        },
        app::Command::Project { action } => match action {
            app::ProjectAction::Add {
                path,
                name,
                language,
            } => cmd::project::add(&path, name.as_deref(), language.as_deref()),
            app::ProjectAction::List => cmd::project::list(),
            app::ProjectAction::Show { name } => cmd::project::show(&name),
            app::ProjectAction::Remove { name } => cmd::project::remove(&name),
        },
        app::Command::Task { action } => match action {
            app::TaskAction::Run { agent, goal } => cmd::task::run(&agent, &goal).await,
            app::TaskAction::List => cmd::task::list(),
            app::TaskAction::Show { id } => cmd::task::show(&id),
            app::TaskAction::Resume { id } => cmd::task::resume(&id).await,
            app::TaskAction::Cancel { id } => cmd::task::cancel(&id),
            app::TaskAction::Delete { id } => cmd::task::delete(&id),
        },
        app::Command::Channel { action } => match action {
            app::ChannelAction::List => cmd::channel::list(),
            app::ChannelAction::Add => cmd::channel::add(),
            app::ChannelAction::Remove { name } => cmd::channel::remove(&name),
            app::ChannelAction::Test { name } => cmd::channel::test(&name).await,
        },
        app::Command::Schedule { action } => match action {
            app::ScheduleAction::List => cmd::schedule::list(),
            app::ScheduleAction::Add {
                name,
                cron,
                agent,
                prompt,
                no_notify,
            } => cmd::schedule::add(&name, &cron, &agent, &prompt, no_notify),
            app::ScheduleAction::Remove { name } => cmd::schedule::remove(&name),
            app::ScheduleAction::Enable { name } => cmd::schedule::enable(&name),
            app::ScheduleAction::Disable { name } => cmd::schedule::disable(&name),
            app::ScheduleAction::RunNow { name } => cmd::schedule::run_now(&name).await,
            app::ScheduleAction::Notifications => cmd::schedule::list_notifications(),
        },
        app::Command::Skill { action } => match action {
            app::SkillAction::List => cmd::skill::list(),
            app::SkillAction::Show { name } => cmd::skill::show(&name),
            app::SkillAction::Install { path } => cmd::skill::install(&path),
            app::SkillAction::Remove { name } => cmd::skill::remove(&name),
            app::SkillAction::Validate { path } => cmd::skill::validate(&path),
            app::SkillAction::Create { output } => cmd::skill::create(output.as_deref()),
        },
        app::Command::Backup { output } => cmd::backup::create(&output),
        app::Command::Restore { archive } => cmd::backup::restore(&archive),
        app::Command::Digest { agent } => cmd::digest::run(agent.as_deref()).await,

    };

    if let Err(e) = result {
        output::error(&e.to_string());
        std::process::exit(1);
    }
}
