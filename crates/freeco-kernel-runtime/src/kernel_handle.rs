//! Trait abstraction for kernel operations needed by the agent runtime.
//!
//! This trait allows `freeco-kernel-runtime` to call back into the kernel for
//! inter-agent operations (spawn, send, list, kill) without creating
//! a circular dependency. The kernel implements this trait and passes
//! it into the agent loop.

use async_trait::async_trait;

/// Agent info returned by list and discovery operations.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub id: String,
    pub name: String,
    pub state: String,
    pub model_provider: String,
    pub model_name: String,
    pub description: String,
    pub tags: Vec<String>,
    pub tools: Vec<String>,
}

/// What came back from one task handed to an agent with `agent_delegate`.
#[derive(Debug, Clone)]
pub struct DelegatedTask {
    pub agent_id: String,
    pub agent_name: String,
    /// The fresh conversation the task ran in. It stays, so the user can open
    /// it and the lead agent can follow up in it with `agent_send`.
    pub session_id: String,
    pub reply: String,
}

/// Handle to kernel operations, passed into the agent loop so agents
/// can interact with each other via tools.
#[allow(clippy::too_many_arguments)]
#[async_trait]
pub trait KernelHandle: Send + Sync {
    /// Spawn a new agent from a TOML manifest string.
    /// `parent_id` is the UUID string of the spawning agent (for lineage tracking).
    /// Returns (agent_id, agent_name) on success.
    async fn spawn_agent(
        &self,
        manifest_toml: &str,
        parent_id: Option<&str>,
    ) -> Result<(String, String), String>;

    /// Send a message to another agent on behalf of `from_agent_id`.
    ///
    /// The kernel uses the sender to tell the receiving agent who is asking,
    /// to name the sender in the audit trail and to show the exchange on the
    /// Comms page. Without a known sender this is plain
    /// [`send_to_agent`](Self::send_to_agent).
    async fn send_to_agent_from(
        &self,
        from_agent_id: Option<&str>,
        agent_id: &str,
        message: &str,
    ) -> Result<String, String> {
        let _ = from_agent_id;
        self.send_to_agent(agent_id, message).await
    }

    /// Send a message to another agent and get the response.
    async fn send_to_agent(&self, agent_id: &str, message: &str) -> Result<String, String>;

    /// Run one task in a fresh conversation of `agent` (the caller itself when
    /// `None`) and wait for its reply.
    ///
    /// A fresh conversation is what lets several of these run at once, even
    /// on one agent, without waiting for or disturbing any chat that agent is
    /// already in. It is filed under the same project as the conversation that
    /// asked, so the project shows the whole piece of work.
    async fn delegate_task(
        &self,
        caller_id: Option<&str>,
        agent: Option<&str>,
        title: &str,
        task: &str,
    ) -> Result<DelegatedTask, String> {
        let _ = (caller_id, agent, title, task);
        Err("Delegation is not available".to_string())
    }

    /// List all running agents.
    /// Store an API key for a provider, and report whether it now works.
    ///
    /// Setup is the one thing the Assistant is asked for constantly and could
    /// not do: it could describe where to paste a key and then watch the user
    /// fail. The key goes to the same store the dashboard writes to, so it
    /// never lands in config.toml or a log.
    ///
    /// The key is never echoed back. Callers get "configured" or an error,
    /// which is all that is needed to decide what to say next.
    fn set_provider_key(&self, provider: &str, key: &str) -> Result<String, String>;

    /// Point an agent at a different model.
    ///
    /// Returns the id as stored, which is not always the id passed in -- the
    /// catalog resolves aliases -- and the caller should report the stored one
    /// so the user is told what actually happened.
    fn set_agent_model_by_id(
        &self,
        agent_id: &str,
        model: &str,
        provider: Option<&str>,
    ) -> Result<(String, String), String>;

    /// Providers, with whether each already has a working key.
    fn list_providers_status(&self) -> Vec<(String, String, bool)>;

    fn list_agents(&self) -> Vec<AgentInfo>;

    /// Kill an agent by ID.
    fn kill_agent(&self, agent_id: &str) -> Result<(), String>;

    /// Activate (wake up) an inactive agent by ID, flipping its state to Running.
    /// Used by orchestrator agents to dispatch work to currently inactive agents
    /// (Suspended, Crashed, or never-started). Terminated agents cannot be revived.
    /// Returns the agent's name on success.
    fn activate_agent(&self, agent_id: &str) -> Result<String, String> {
        let _ = agent_id;
        Err("Agent activation not available".to_string())
    }

    /// Store a value in shared memory (cross-agent accessible).
    /// The data gateway, when one is configured.
    ///
    /// Returned through the handle rather than constructed in the agent loop
    /// because the gateway needs the database the kernel owns, and because a
    /// single instance keeps one token table: a value masked in one
    /// conversation must restore in another, or the same secret appears under
    /// two names and the model is told they are different things.
    ///
    /// Defaults to None so a handle without one still compiles; a None gateway
    /// means prompts pass through unscanned, which is the behaviour that
    /// existed before this was wired.
    fn data_gateway(&self) -> Option<std::sync::Arc<freeco_datagateway::DataGateway>> {
        None
    }

    fn memory_store(&self, key: &str, value: serde_json::Value) -> Result<(), String>;

    /// Recall a value from shared memory.
    fn memory_recall(&self, key: &str) -> Result<Option<serde_json::Value>, String>;

    /// Find agents by query (matches on name substring, tag, or tool name; case-insensitive).
    fn find_agents(&self, query: &str) -> Vec<AgentInfo>;

    /// Post a task to the shared task queue. Returns the task ID.
    async fn task_post(
        &self,
        title: &str,
        description: &str,
        assigned_to: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<String, String>;

    /// Claim the next available task (optionally filtered by assignee). Returns task JSON or None.
    async fn task_claim(&self, agent_id: &str) -> Result<Option<serde_json::Value>, String>;

    /// Post a task onto a team's board (named by id or name), optionally
    /// waiting on other tasks. The assignee, when given, must be on the team.
    async fn team_task_post(
        &self,
        title: &str,
        description: &str,
        assigned_to: Option<&str>,
        created_by: Option<&str>,
        team: Option<&str>,
        blocked_by: &[String],
    ) -> Result<String, String> {
        let _ = (
            title,
            description,
            assigned_to,
            created_by,
            team,
            blocked_by,
        );
        Err("Team task boards are not available".to_string())
    }

    /// Mark a task as completed with a result string.
    async fn task_complete(&self, task_id: &str, result: &str) -> Result<(), String>;

    /// Complete a task on behalf of `by` (the holder or the poster only) and
    /// return the ids of the waiting tasks this made ready.
    async fn task_complete_by(
        &self,
        task_id: &str,
        result: &str,
        by: Option<&str>,
    ) -> Result<Vec<String>, String> {
        let _ = by;
        self.task_complete(task_id, result)
            .await
            .map(|_| Vec::new())
    }

    /// List tasks, optionally filtered by status.
    async fn task_list(&self, status: Option<&str>) -> Result<Vec<serde_json::Value>, String>;

    /// Publish a custom event that can trigger proactive agents.
    async fn publish_event(
        &self,
        event_type: &str,
        payload: serde_json::Value,
    ) -> Result<(), String>;

    /// Add an entity to the knowledge graph.
    async fn knowledge_add_entity(
        &self,
        entity: freeco_types::memory::Entity,
    ) -> Result<String, String>;

    /// Add a relation to the knowledge graph.
    async fn knowledge_add_relation(
        &self,
        relation: freeco_types::memory::Relation,
    ) -> Result<String, String>;

    /// Query the knowledge graph with a pattern.
    async fn knowledge_query(
        &self,
        pattern: freeco_types::memory::GraphPattern,
    ) -> Result<Vec<freeco_types::memory::GraphMatch>, String>;

    /// Create a cron job for the calling agent.
    async fn cron_create(
        &self,
        agent_id: &str,
        job_json: serde_json::Value,
    ) -> Result<String, String> {
        let _ = (agent_id, job_json);
        Err("Cron scheduler not available".to_string())
    }

    /// List cron jobs for the calling agent.
    async fn cron_list(&self, agent_id: &str) -> Result<Vec<serde_json::Value>, String> {
        let _ = agent_id;
        Err("Cron scheduler not available".to_string())
    }

    /// Cancel a cron job by ID.
    async fn cron_cancel(&self, job_id: &str) -> Result<(), String> {
        let _ = job_id;
        Err("Cron scheduler not available".to_string())
    }

    /// The agent's `capabilities.shell` patterns (its granted
    /// `Capability::ShellExec` globs). Empty when it has none, which leaves
    /// shell commands to the exec policy alone.
    fn shell_patterns(&self, agent_id: &str) -> Vec<String> {
        let _ = agent_id;
        Vec::new()
    }

    /// Check if a tool requires approval based on current policy.
    fn requires_approval(&self, tool_name: &str) -> bool {
        let _ = tool_name;
        false
    }

    /// Request approval for a tool execution. Blocks until approved/denied/timed out.
    /// Returns `Ok(true)` if approved, `Ok(false)` if denied or timed out.
    /// Ask the user to allow one tool call.
    ///
    /// `purpose` is the agent's own account of what it is trying to achieve and
    /// why this tool is needed for it. Without it the user sees a tool name and
    /// a string of arguments, which is not a question anyone can answer: faced
    /// with it people approve everything or deny everything. Implementations
    /// fall back to a generic per-tool description when it is absent, but that
    /// describes the mechanism rather than the aim, so it is a poor substitute.
    async fn request_approval(
        &self,
        agent_id: &str,
        tool_name: &str,
        action_summary: &str,
        purpose: Option<&str>,
    ) -> Result<bool, String> {
        let _ = (agent_id, tool_name, action_summary, purpose);
        Ok(true) // Default: auto-approve
    }

    /// Scan the skills in a GitHub repository without installing anything.
    /// Returns the pinned commit plus a security report per skill.
    async fn skill_github_preview(&self, source: &str) -> Result<serde_json::Value, String> {
        let _ = source;
        Err("Skill installs not available".to_string())
    }

    /// Install skills from a GitHub repository at exactly `commit` (a full
    /// SHA from a preview). `skills` limits the install to those names.
    async fn skill_github_install(
        &self,
        source: &str,
        commit: &str,
        skills: &[String],
        overwrite: bool,
    ) -> Result<serde_json::Value, String> {
        let _ = (source, commit, skills, overwrite);
        Err("Skill installs not available".to_string())
    }

    /// List available Hands and their activation status.
    async fn hand_list(&self) -> Result<Vec<serde_json::Value>, String> {
        Err("Hands system not available".to_string())
    }

    /// Install a Hand from TOML content.
    async fn hand_install(
        &self,
        toml_content: &str,
        skill_content: &str,
    ) -> Result<serde_json::Value, String> {
        let _ = (toml_content, skill_content);
        Err("Hands system not available".to_string())
    }

    /// Activate a Hand — spawns a specialized autonomous agent.
    async fn hand_activate(
        &self,
        hand_id: &str,
        config: std::collections::HashMap<String, serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let _ = (hand_id, config);
        Err("Hands system not available".to_string())
    }

    /// Check the status and dashboard metrics of an active Hand.
    async fn hand_status(&self, hand_id: &str) -> Result<serde_json::Value, String> {
        let _ = hand_id;
        Err("Hands system not available".to_string())
    }

    /// Deactivate a running Hand and stop its agent.
    async fn hand_deactivate(&self, instance_id: &str) -> Result<(), String> {
        let _ = instance_id;
        Err("Hands system not available".to_string())
    }

    /// The organisation: companies, projects, teams and their members.
    async fn org_overview(&self) -> Result<serde_json::Value, String> {
        Err("Organisation store not available".to_string())
    }

    /// Create a team, optionally within a project.
    async fn team_create(&self, name: &str, project_id: Option<&str>) -> Result<String, String> {
        let _ = (name, project_id);
        Err("Organisation store not available".to_string())
    }

    /// Put an agent in a team with a stated accountability.
    async fn team_add_member(
        &self,
        team_id: &str,
        agent_id: &str,
        role: Option<&str>,
    ) -> Result<String, String> {
        let _ = (team_id, agent_id, role);
        Err("Organisation store not available".to_string())
    }

    /// Every team, with its project and its members.
    ///
    /// Teams could be created and never read back, so an agent could be put in
    /// one and had no way to find out, and no way to learn the id its team
    /// memory is keyed by.
    async fn team_list(&self) -> Result<serde_json::Value, String> {
        Err("Organisation store not available".to_string())
    }

    /// Create a company — the top of the organisation, which projects belong to.
    ///
    /// The store and the HTTP route for this both existed; the tool did not, so
    /// an agent could be made CEO of an organisation and still had no way to
    /// create the company it was CEO of. Worse, `project_create` takes a
    /// `company_id` and the companies table was empty, so any id an agent
    /// supplied failed on the foreign key with nothing it could do about it.
    async fn company_create(
        &self,
        name: &str,
        description: Option<&str>,
    ) -> Result<String, String> {
        let _ = (name, description);
        Err("Organisation store not available".to_string())
    }

    /// Create a project that teams and conversations attach to.
    async fn project_create(
        &self,
        name: &str,
        description: Option<&str>,
        company_id: Option<&str>,
    ) -> Result<String, String> {
        let _ = (name, description, company_id);
        Err("Organisation store not available".to_string())
    }

    /// Health of the installation, including what containers are running and
    /// whether any are unaccounted for.
    async fn system_audit(&self) -> Result<serde_json::Value, String> {
        Err("System audit not available".to_string())
    }

    /// Rename the calling agent's current conversation.
    async fn session_rename(&self, agent_id: &str, label: &str) -> Result<String, String> {
        let _ = (agent_id, label);
        Err("Session management not available".to_string())
    }

    /// Start a new named conversation for the calling agent.
    ///
    /// `carry_history` copies the current conversation into the new one and
    /// deliberately does not remove it from the original: there is no
    /// recoverable message delete yet, and losing a conversation to a tidying
    /// operation is far worse than leaving a duplicate behind.
    async fn session_fork(
        &self,
        agent_id: &str,
        label: &str,
        carry_history: bool,
    ) -> Result<String, String> {
        let _ = (agent_id, label, carry_history);
        Err("Session management not available".to_string())
    }

    /// List discovered external A2A agents as (name, url) pairs.
    fn list_a2a_agents(&self) -> Vec<(String, String)> {
        vec![]
    }

    /// Get the URL of a discovered external A2A agent by name.
    fn get_a2a_agent_url(&self, name: &str) -> Option<String> {
        let _ = name;
        None
    }

    /// Send a message to a user on a named channel adapter (e.g., "email", "telegram").
    /// When `thread_id` is provided, the message is sent as a thread reply.
    /// Returns a confirmation string on success.
    /// Get the default recipient for a channel (e.g. default_chat_id for Telegram).
    async fn get_channel_default_recipient(&self, channel: &str) -> Option<String> {
        let _ = channel;
        None
    }

    async fn send_channel_message(
        &self,
        channel: &str,
        recipient: &str,
        message: &str,
        thread_id: Option<&str>,
    ) -> Result<String, String> {
        let _ = (channel, recipient, message, thread_id);
        Err("Channel send not available".to_string())
    }

    /// Send media content (image/file) to a user on a named channel adapter.
    /// `media_type` is "image" or "file", `media_url` is the URL, `caption` is optional text.
    /// When `thread_id` is provided, the media is sent as a thread reply.
    async fn send_channel_media(
        &self,
        channel: &str,
        recipient: &str,
        media_type: &str,
        media_url: &str,
        caption: Option<&str>,
        filename: Option<&str>,
        thread_id: Option<&str>,
    ) -> Result<String, String> {
        let _ = (
            channel, recipient, media_type, media_url, caption, filename, thread_id,
        );
        Err("Channel media send not available".to_string())
    }

    /// Send a local file (raw bytes) to a user on a named channel adapter.
    /// Used by the `channel_send` tool when `file_path` is provided.
    /// When `thread_id` is provided, the file is sent as a thread reply.
    async fn send_channel_file_data(
        &self,
        channel: &str,
        recipient: &str,
        data: Vec<u8>,
        filename: &str,
        mime_type: &str,
        thread_id: Option<&str>,
    ) -> Result<String, String> {
        let _ = (channel, recipient, data, filename, mime_type, thread_id);
        Err("Channel file data send not available".to_string())
    }

    /// Refresh an agent's last_active timestamp without changing any other state.
    /// Called by the agent loop before long LLM calls to prevent heartbeat false-positives.
    fn touch_agent(&self, agent_id: &str) {
        let _ = agent_id;
    }

    /// Spawn an agent with capability inheritance enforcement.
    /// `parent_caps` are the parent's granted capabilities. The kernel MUST verify
    /// that every capability in the child manifest is covered by `parent_caps`.
    async fn spawn_agent_checked(
        &self,
        manifest_toml: &str,
        parent_id: Option<&str>,
        parent_caps: &[freeco_types::capability::Capability],
    ) -> Result<(String, String), String> {
        // Default: delegate to spawn_agent (no enforcement)
        // The kernel MUST override this with real enforcement
        let _ = parent_caps;
        self.spawn_agent(manifest_toml, parent_id).await
    }

    /// One agent's full setup (model, tools it really has, skills, MCP
    /// servers, tags), for agents that manage other agents. Accepts a UUID or
    /// a name.
    fn describe_agent(&self, agent: &str) -> Result<serde_json::Value, String> {
        let _ = agent;
        Err("Agent details not available".to_string())
    }

    /// An agent's skill allowlist and every installed skill name, as
    /// `(assigned, installed)`. An empty `assigned` means every skill.
    fn agent_skills(&self, agent: &str) -> Result<(Vec<String>, Vec<String>), String> {
        let _ = agent;
        Err("Skill assignment not available".to_string())
    }

    /// Replace an agent's skill allowlist. Empty means every skill.
    fn set_agent_skills_by_ref(&self, agent: &str, skills: Vec<String>) -> Result<(), String> {
        let _ = (agent, skills);
        Err("Skill assignment not available".to_string())
    }

    /// Reload skills from disk; returns how many are installed afterwards.
    fn reload_skills_now(&self) -> Result<usize, String> {
        Err("Skill reload not available".to_string())
    }

    /// Every model in the catalog, with whether it can be used right now.
    fn list_model_choices(&self) -> Vec<ModelChoice> {
        Vec::new()
    }
}

/// A model an agent can be pointed at, as `model_list` reports it.
#[derive(Debug, Clone, Default)]
pub struct ModelChoice {
    pub id: String,
    pub display_name: String,
    pub provider: String,
    pub tier: String,
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub supports_tools: bool,
    pub input_cost_per_m: f64,
    pub output_cost_per_m: f64,
    /// The provider has a key (or needs none) and has not reported the model
    /// as unavailable for that key.
    pub usable: bool,
    /// The provider has a key or needs none. Kept apart from `usable` so a
    /// model the provider does not serve is not reported as a missing key,
    /// which would send the agent to replace a working key.
    pub provider_has_key: bool,
}
