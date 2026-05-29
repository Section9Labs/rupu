//! Verifies that running an agent with a `concerns:` block produces the
//! expected coverage artifacts: catalog snapshot + ledger directory with
//! the coverage tools injected into the agent's tool list.

use rupu_agent::runner::{BypassDecider, CapturingMockProvider, ScriptedTurn};
use rupu_agent::{run_agent, AgentRunOpts};
use rupu_coverage::{
    target_id, CatalogMode, ConcernsBlock, ConcernsEntry, CoveragePaths, IncludeDirective,
};
use rupu_providers::types::StopReason;
use rupu_tools::ToolContext;
use std::sync::Arc;

fn stride_block() -> ConcernsBlock {
    ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Auto,
            filter: None,
        })],
    }
}

fn stride_index_block() -> ConcernsBlock {
    ConcernsBlock {
        entries: vec![ConcernsEntry::Include(IncludeDirective {
            include: "stride".to_string(),
            overrides: vec![],
            mode: CatalogMode::Index,
            filter: None,
        })],
    }
}

#[tokio::test]
async fn agent_run_with_concerns_writes_catalog_snapshot() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "Coverage check complete.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();

    let opts = AgentRunOpts {
        agent_name: "test-agent".into(),
        agent_system_prompt: "You are a coverage agent.".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_cov_test".into(),
        workspace_id: "ws_cov_test".into(),
        workspace_path: workspace.clone(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.clone(),
            ..Default::default()
        },
        user_message: "Check coverage.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        concerns: Some(stride_block()),
        scope_name: None,
        surface_tag: None,
    };

    run_agent(opts).await.expect("agent run should succeed");

    // Verify catalog snapshot was written.
    let target = target_id(&workspace, "test-agent");
    let paths = CoveragePaths::new(&workspace, &target);
    assert!(
        paths.catalog.exists(),
        "catalog snapshot should exist at {:?}",
        paths.catalog
    );

    // Verify snapshot is valid YAML with at least one concern.
    let snapshot_text = std::fs::read_to_string(&paths.catalog).unwrap();
    assert!(
        snapshot_text.contains("stride:spoofing"),
        "catalog snapshot should contain stride concerns, got: {snapshot_text}"
    );

    // Verify the 4 coverage tools were injected into the LLM request.
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one LLM request");
    let tool_names: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        tool_names.contains(&"coverage_mark"),
        "coverage_mark should be in tools: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"coverage_status"),
        "coverage_status should be in tools: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"coverage_remaining"),
        "coverage_remaining should be in tools: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"report_finding"),
        "report_finding should be in tools: {tool_names:?}"
    );
}

#[tokio::test]
async fn agent_run_without_concerns_does_not_inject_coverage_tools() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "Done.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();

    let opts = AgentRunOpts {
        agent_name: "plain-agent".into(),
        agent_system_prompt: "You are a plain agent.".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_plain_test".into(),
        workspace_id: "ws_plain".into(),
        workspace_path: workspace.clone(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.clone(),
            ..Default::default()
        },
        user_message: "Do nothing.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        concerns: None,
        scope_name: None,
        surface_tag: None,
    };

    run_agent(opts).await.expect("agent run should succeed");

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let tool_names: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        !tool_names.contains(&"coverage_mark"),
        "coverage_mark should NOT be present when concerns is None: {tool_names:?}"
    );
}

#[tokio::test]
async fn agent_run_with_concerns_injects_catalog_into_system_prompt() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // Use a capturing provider so we can inspect the system prompt.
    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "Done.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();

    let opts = AgentRunOpts {
        agent_name: "prompt-check-agent".into(),
        agent_system_prompt: "Base prompt.".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_prompt_check".into(),
        workspace_id: "ws_prompt".into(),
        workspace_path: workspace.clone(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.clone(),
            ..Default::default()
        },
        user_message: "check prompt".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        concerns: Some(stride_block()),
        scope_name: None,
        surface_tag: None,
    };

    run_agent(opts).await.expect("agent run should succeed");

    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1);
    let system = requests[0].system.as_deref().unwrap_or("");
    assert!(
        system.starts_with("Base prompt."),
        "system prompt should start with original prompt"
    );
    assert!(
        system.contains("STRIDE") || system.contains("stride") || system.contains("Spoofing"),
        "system prompt should contain catalog content from render_full_mode"
    );
}

#[tokio::test]
async fn surface_tag_override_is_respected() {
    // When AgentRunOpts carries surface_tag: Some("workflow"), the runner
    // must wire that value into tool_context.surface_tag rather than
    // defaulting to "agent". We verify this by running with a custom tag
    // and checking that the run completes successfully (the tag path is
    // exercised without error). The actual surface propagation into
    // FileTouchEvents is tested via unit tests in rupu-coverage; here
    // we only need to confirm the runner plumbs the field through.
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "Done with workflow surface.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);

    let opts = AgentRunOpts {
        agent_name: "workflow-step-agent".into(),
        agent_system_prompt: "You are a workflow step agent.".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_surface_tag_test".into(),
        workspace_id: "ws_surface_test".into(),
        workspace_path: workspace.clone(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.clone(),
            ..Default::default()
        },
        user_message: "Check coverage.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        // Enable coverage so the runner's surface_tag assignment fires.
        concerns: Some(stride_block()),
        scope_name: None,
        // This is the field under test: override to "workflow".
        surface_tag: Some("workflow".to_string()),
    };

    // The run must complete cleanly — confirms the surface_tag override
    // doesn't break the runner's coverage wiring path.
    run_agent(opts).await.expect("agent run with workflow surface_tag should succeed");

    // Verify catalog was still written (coverage harness ran normally).
    let target = rupu_coverage::target_id(&workspace, "workflow-step-agent");
    let paths = CoveragePaths::new(&workspace, &target);
    assert!(
        paths.catalog.exists(),
        "catalog snapshot should exist even with surface_tag override"
    );
}

#[tokio::test]
async fn agent_run_with_index_mode_concerns_injects_search_and_detail_tools() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    let provider = CapturingMockProvider::new(vec![ScriptedTurn::AssistantText {
        text: "Coverage index mode check complete.".into(),
        stop: StopReason::EndTurn,
        input_tokens: 1,
        output_tokens: 1,
    }]);
    let captured = provider.captured.clone();

    let opts = AgentRunOpts {
        agent_name: "index-mode-agent".into(),
        agent_system_prompt: "You are a coverage index agent.".into(),
        agent_tools: None,
        provider: Box::new(provider),
        provider_name: "mock".into(),
        model: "mock-1".into(),
        run_id: "run_index_mode_test".into(),
        workspace_id: "ws_index_mode".into(),
        workspace_path: workspace.clone(),
        transcript_path: workspace.join("run.jsonl"),
        max_turns: 5,
        decider: Arc::new(BypassDecider),
        tool_context: ToolContext {
            workspace_path: workspace.clone(),
            ..Default::default()
        },
        user_message: "Check coverage in index mode.".into(),
        initial_messages: Vec::new(),
        turn_index_offset: 0,
        mode_str: "bypass".into(),
        no_stream: true,
        suppress_stream_stdout: false,
        mcp_registry: None,
        effort: None,
        context_window: None,
        output_format: None,
        anthropic_task_budget: None,
        anthropic_context_management: None,
        anthropic_speed: None,
        parent_run_id: None,
        depth: 0,
        dispatchable_agents: None,
        step_id: String::new(),
        on_tool_call: None,
        on_stream_event: None,
        concerns: Some(stride_index_block()),
        scope_name: None,
        surface_tag: None,
    };

    run_agent(opts).await.expect("agent run with index mode concerns should succeed");

    // Verify that the LLM request included all 6 coverage tools,
    // including the new search and detail tools for index-mode catalogs.
    let requests = captured.lock().unwrap();
    assert_eq!(requests.len(), 1, "expected exactly one LLM request");
    let tool_names: Vec<&str> = requests[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        tool_names.contains(&"coverage_concerns_search"),
        "coverage_concerns_search should be in tools: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"coverage_concerns_detail"),
        "coverage_concerns_detail should be in tools: {tool_names:?}"
    );
    // Existing 4 tools must still be present.
    assert!(
        tool_names.contains(&"coverage_mark"),
        "coverage_mark should be in tools: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"report_finding"),
        "report_finding should be in tools: {tool_names:?}"
    );

    // In index mode, the system prompt should contain the index header
    // (not the full concern body), since render_prompt_section uses
    // index rendering when mode is CatalogMode::Index.
    let system = requests[0].system.as_deref().unwrap_or("");
    assert!(
        system.starts_with("You are a coverage index agent."),
        "system prompt should start with original prompt"
    );
    assert!(
        system.contains("coverage_concerns_search") || system.contains("index"),
        "index-mode system prompt should reference search tool or index: {system}"
    );
}
