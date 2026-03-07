//! MCP server template catalog.
//!
//! `GET /plugins/templates` — returns curated, grouped MCP server templates
//! that users can browse and one-click install.

use axum::response::IntoResponse;
use serde::Serialize;

/// A single MCP server template.
#[derive(Debug, Serialize, Clone)]
pub struct PluginTemplate {
    /// Unique template identifier (e.g. "brave-search").
    pub id: &'static str,
    /// Human-readable display name.
    pub name: &'static str,
    /// Short description of what this plugin does.
    pub description: &'static str,
    /// Category group (e.g. "Search & Information").
    pub category: &'static str,
    /// Emoji icon for the category/template.
    pub icon: &'static str,
    /// NPX command to run the MCP server.
    pub command: &'static str,
    /// Command arguments.
    pub args: &'static [&'static str],
    /// Environment variable hints the user needs to set.
    pub env_hints: &'static [&'static str],
    /// Whether this plugin requires an API key to function.
    pub requires_api_key: bool,
}

/// `GET /plugins/templates` — return the full template catalog.
pub async fn list_templates() -> impl IntoResponse {
    axum::Json(TEMPLATES.to_vec())
}

/// Curated catalog of MCP server templates for a personal assistant.
pub static TEMPLATES: &[PluginTemplate] = &[
    // ── Search & Information ───────────────────────────────────────
    PluginTemplate {
        id: "brave-search",
        name: "Brave Search",
        description: "Search the web using Brave's privacy-focused search engine",
        category: "Search & Information",
        icon: "🔍",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-brave-search"],
        env_hints: &["BRAVE_API_KEY"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "web-search",
        name: "Web Search",
        description: "Search the web without API keys using DuckDuckGo",
        category: "Search & Information",
        icon: "🌐",
        command: "npx",
        args: &["-y", "@anthropic/mcp-server-web-search"],
        env_hints: &[],
        requires_api_key: false,
    },
    PluginTemplate {
        id: "fetch",
        name: "Fetch",
        description: "Read and extract content from any webpage or URL",
        category: "Search & Information",
        icon: "📰",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-fetch"],
        env_hints: &[],
        requires_api_key: false,
    },
    PluginTemplate {
        id: "tavily",
        name: "Tavily Search",
        description: "AI-optimized search with curated, structured results",
        category: "Search & Information",
        icon: "🎯",
        command: "npx",
        args: &["-y", "tavily-mcp@latest"],
        env_hints: &["TAVILY_API_KEY"],
        requires_api_key: true,
    },
    // ── Files & Documents ──────────────────────────────────────────
    PluginTemplate {
        id: "filesystem",
        name: "File System",
        description: "Browse, read, write, and search files on your computer",
        category: "Files & Documents",
        icon: "📁",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-filesystem", "/home"],
        env_hints: &[],
        requires_api_key: false,
    },
    PluginTemplate {
        id: "google-drive",
        name: "Google Drive",
        description: "Access, search, and manage your Google Drive files",
        category: "Files & Documents",
        icon: "☁️",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-gdrive"],
        env_hints: &["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "everart",
        name: "EverArt",
        description: "Generate and manage digital artwork and images",
        category: "Files & Documents",
        icon: "🖼️",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-everart"],
        env_hints: &["EVERART_API_KEY"],
        requires_api_key: true,
    },
    // ── Communication ──────────────────────────────────────────────
    PluginTemplate {
        id: "slack",
        name: "Slack",
        description: "Send messages, search conversations, and manage Slack channels",
        category: "Communication",
        icon: "💬",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-slack"],
        env_hints: &["SLACK_BOT_TOKEN", "SLACK_TEAM_ID"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "gmail",
        name: "Gmail",
        description: "Read, search, compose, and manage your email",
        category: "Communication",
        icon: "📧",
        command: "npx",
        args: &["-y", "@anthropic/mcp-server-gmail"],
        env_hints: &["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "bluesky",
        name: "Bluesky",
        description: "Post, search, and interact on the Bluesky social network",
        category: "Communication",
        icon: "🦋",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-bluesky"],
        env_hints: &["BLUESKY_HANDLE", "BLUESKY_APP_PASSWORD"],
        requires_api_key: true,
    },
    // ── Productivity ───────────────────────────────────────────────
    PluginTemplate {
        id: "google-calendar",
        name: "Google Calendar",
        description: "View, create, and manage calendar events and schedules",
        category: "Productivity",
        icon: "📅",
        command: "npx",
        args: &["-y", "@anthropic/mcp-server-google-calendar"],
        env_hints: &["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "notion",
        name: "Notion",
        description: "Access, search, and update your Notion pages and databases",
        category: "Productivity",
        icon: "📝",
        command: "npx",
        args: &["-y", "@notionhq/mcp-server-notion"],
        env_hints: &["NOTION_API_KEY"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "todoist",
        name: "Todoist",
        description: "Manage tasks, projects, and to-do lists",
        category: "Productivity",
        icon: "✅",
        command: "npx",
        args: &["-y", "@anthropic/mcp-server-todoist"],
        env_hints: &["TODOIST_API_TOKEN"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "google-maps",
        name: "Google Maps",
        description: "Get directions, find places, and explore locations",
        category: "Productivity",
        icon: "🗺️",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-google-maps"],
        env_hints: &["GOOGLE_MAPS_API_KEY"],
        requires_api_key: true,
    },
    // ── Smart Home & Life ──────────────────────────────────────────
    PluginTemplate {
        id: "home-assistant",
        name: "Home Assistant",
        description: "Control smart home devices, check sensors, and run automations",
        category: "Smart Home & Life",
        icon: "🏠",
        command: "npx",
        args: &["-y", "@keithah/mcp-server-home-assistant"],
        env_hints: &["HA_URL", "HA_TOKEN"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "spotify",
        name: "Spotify",
        description: "Control music playback, search songs, and manage playlists",
        category: "Smart Home & Life",
        icon: "🎵",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-spotify"],
        env_hints: &["SPOTIFY_CLIENT_ID", "SPOTIFY_CLIENT_SECRET"],
        requires_api_key: true,
    },
    PluginTemplate {
        id: "apple-notes",
        name: "Apple Notes",
        description: "Search and read your Apple Notes",
        category: "Smart Home & Life",
        icon: "🍎",
        command: "npx",
        args: &["-y", "@anthropic/mcp-server-apple-notes"],
        env_hints: &[],
        requires_api_key: false,
    },
    // ── Knowledge & Learning ───────────────────────────────────────
    PluginTemplate {
        id: "memory",
        name: "Memory",
        description: "Persistent key-value memory that survives across conversations",
        category: "Knowledge & Learning",
        icon: "🧠",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-memory"],
        env_hints: &[],
        requires_api_key: false,
    },
    PluginTemplate {
        id: "sqlite",
        name: "SQLite",
        description: "Query and manage local SQLite databases",
        category: "Knowledge & Learning",
        icon: "🗄️",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-sqlite"],
        env_hints: &[],
        requires_api_key: false,
    },
    PluginTemplate {
        id: "sequential-thinking",
        name: "Sequential Thinking",
        description: "Break down complex problems into structured reasoning steps",
        category: "Knowledge & Learning",
        icon: "💡",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-sequential-thinking"],
        env_hints: &[],
        requires_api_key: false,
    },
    // ── Creative & Media ───────────────────────────────────────────
    PluginTemplate {
        id: "puppeteer",
        name: "Browser Automation",
        description: "Automate web browsing, take screenshots, and fill forms",
        category: "Creative & Media",
        icon: "🌐",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-puppeteer"],
        env_hints: &[],
        requires_api_key: false,
    },
    PluginTemplate {
        id: "youtube",
        name: "YouTube",
        description: "Search videos, get transcripts, and extract video information",
        category: "Creative & Media",
        icon: "▶️",
        command: "npx",
        args: &["-y", "@anthropic/mcp-server-youtube"],
        env_hints: &[],
        requires_api_key: false,
    },
    PluginTemplate {
        id: "github",
        name: "GitHub",
        description: "Manage repositories, issues, and pull requests",
        category: "Developer",
        icon: "💻",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-github"],
        env_hints: &["GITHUB_PERSONAL_ACCESS_TOKEN"],
        requires_api_key: true,
    },
];
