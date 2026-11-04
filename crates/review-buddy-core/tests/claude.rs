//! Port of ClaudeTests.swift and the Prompts/AI-model parts of PromptsAndModelsTests.swift.

use review_buddy_core::claude::prompts;
use review_buddy_core::claude::session::*;
use review_buddy_core::claude::stream_events::*;
use review_buddy_core::claude::transcript::*;
use review_buddy_core::models::*;
use std::path::PathBuf;
use std::time::Duration;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(format!("{name}.jsonl"))
}

fn fixture_events(name: &str) -> Vec<ClaudeEvent> {
    let data = std::fs::read(fixture_path(name)).unwrap();
    let mut s = LineSplitter::new();
    s.feed(&data).iter().filter_map(|l| StreamJson::decode_line(l)).collect()
}

/// Writes an executable shell script and returns its path.
fn make_script(body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("rb-claude-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("claude");
    std::fs::write(&p, format!("#!/bin/sh\n{body}")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn has_pair(a: &[String], x: &str, y: &str) -> bool {
    a.windows(2).any(|w| w[0] == x && w[1] == y)
}

// MARK: - StreamJSON

#[test]
fn decodes_recorded_tool_session() {
    let ev = fixture_events("claude-stream");
    assert_eq!(ev.len(), 9);
    let ClaudeEvent::Initialized { session_id: sid, model } = &ev[0] else { panic!("no init") };
    assert_eq!(sid, "77dde8c7-4613-4ed2-bfe5-4fa278d39098");
    assert!(model.is_some());
    let ClaudeEvent::Assistant { blocks, .. } = &ev[1] else { panic!("no assistant") };
    let Some(ContentBlock::ToolUse { id, name, input }) = blocks.first() else { panic!("no tool use") };
    assert!(name == "Read" && id.starts_with("toolu_") && input.contains("file_path"));
    assert_eq!(ev[2], ClaudeEvent::other("rate_limit_event"));
    let ClaudeEvent::ToolResults(res) = &ev[3] else { panic!("no tool result") };
    assert!(res[0].tool_use_id == *id && res[0].content.contains("hello world") && !res[0].is_error);
    assert_eq!(ev[4], ClaudeEvent::other("system/thinking_tokens"));
    let ClaudeEvent::Result(r) = &ev[8] else { panic!("no result") };
    assert!(r.session_id.as_deref() == Some(sid.as_str()) && !r.is_error && r.subtype == "success");
    assert!(r.cost_usd.unwrap_or(0.0) > 0.0);
    assert!(r.structured_output.is_none());
}

#[test]
fn decodes_partial_messages() {
    let ev = fixture_events("claude-partial");
    let deltas: String = ev
        .iter()
        .filter_map(|e| if let ClaudeEvent::TextDelta { text, .. } = e { Some(text.as_str()) } else { None })
        .collect();
    assert_eq!(deltas, "one two three");
    assert!(ev.contains(&ClaudeEvent::other("stream_event/message_start")));
}

#[test]
fn tolerates_garbage() {
    assert_eq!(StreamJson::decode_line(b""), None);
    assert_eq!(StreamJson::decode_line(b"not json"), None);
    assert_eq!(StreamJson::decode_line(b"[1,2]"), None);
    assert_eq!(StreamJson::decode_line(br#"{"type":"brand_new"}"#), Some(ClaudeEvent::other("brand_new")));
    assert_eq!(StreamJson::decode_line(br#"{"type":"assistant"}"#), Some(ClaudeEvent::other("assistant")));
    assert_eq!(
        StreamJson::decode_line(br#"{"type":"user","message":{"content":[{"type":"text","text":"x"}]}}"#),
        Some(ClaudeEvent::other("user"))
    );
}

#[test]
fn structured_output_and_tool_result_arrays() {
    let r = StreamJson::decode_line(
        br#"{"type":"result","subtype":"success","is_error":false,"structured_output":{"a":1},"result":"x"}"#,
    );
    let Some(ClaudeEvent::Result(res)) = r else { panic!() };
    assert_eq!(res.structured_output, Some(serde_json::json!({"a": 1})));
    let Some(ClaudeEvent::Result(res2)) = StreamJson::decode_line(br#"{"type":"result","structured_output":null}"#) else {
        panic!()
    };
    assert_eq!(res2.structured_output, None);
    let tr = StreamJson::decode_line(
        br#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t","is_error":true,"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}]}}"#,
    );
    assert_eq!(tr, Some(ClaudeEvent::ToolResults(vec![ToolResult::new("t", "a\nb", true)])));
}

#[test]
fn decode_output_prefers_structured_then_text() {
    let mut r = ClaudeResult { structured_output: Some(serde_json::json!({"findings": []})), ..Default::default() };
    assert!(r.decode_output::<PreReviewResult>().unwrap().findings.is_empty());
    r.structured_output = None;
    r.text = Some(r#"{"summary":"S","readingOrder":[],"risks":[]}"#.into());
    assert_eq!(r.decode_output::<AISummary>().unwrap().summary, "S");
    r.text = Some("nope".into());
    assert_eq!(r.decode_output::<AISummary>().unwrap_err(), ClaudeError::BadOutput);
}

#[test]
fn encodes_user_and_interrupt() {
    let u = StreamJson::user_message("hi \"there\"\nline2");
    assert_eq!(u.last(), Some(&b'\n'));
    assert!(!u[..u.len() - 1].contains(&b'\n'), "message must be a single NDJSON line");
    let obj: serde_json::Value = serde_json::from_slice(&u[..u.len() - 1]).unwrap();
    assert_eq!(obj["type"], "user");
    assert_eq!(obj["message"]["role"], "user");
    assert_eq!(obj["message"]["content"], "hi \"there\"\nline2");
    let i = StreamJson::interrupt_request_with_id("abc");
    let i: serde_json::Value = serde_json::from_slice(&i[..i.len() - 1]).unwrap();
    assert!(i["type"] == "control_request" && i["request_id"] == "abc");
    assert_eq!(i["request"]["subtype"], "interrupt");
    let fresh = StreamJson::interrupt_request();
    assert_eq!(fresh.last(), Some(&b'\n'));
}

#[test]
fn line_splitter_handles_chunk_boundaries() {
    let mut s = LineSplitter::new();
    assert!(s.feed(b"ab").is_empty());
    assert_eq!(s.feed(b"c\nde"), vec![b"abc".to_vec()]);
    assert_eq!(s.feed(b"\n\nf\ng"), vec![b"de".to_vec(), b"f".to_vec()]);
    assert_eq!(s.flush(), Some(b"g".to_vec()));
    assert_eq!(s.flush(), None);
    // Byte-at-a-time feeding of a whole fixture yields the same lines.
    let data = std::fs::read(fixture_path("claude-stream")).unwrap();
    let expected = LineSplitter::new().feed(&data);
    let mut bytewise = LineSplitter::new();
    let mut got = Vec::new();
    for b in &data {
        got.extend(bytewise.feed(std::slice::from_ref(b)));
    }
    assert_eq!(got, expected);
    assert_eq!(got.len(), 9);
}

// MARK: - TranscriptReducer

#[test]
fn folds_tool_session() {
    let mut r = TranscriptReducer::default();
    r.user_sent("read it", Some("Read note"));
    assert!(r.is_running());
    for e in fixture_events("claude-stream") {
        r.apply(&e);
    }
    assert!(!r.is_running());
    assert_eq!(r.session_id(), Some("77dde8c7-4613-4ed2-bfe5-4fa278d39098"));
    let roles: Vec<Role> = r.messages().iter().map(|m| m.role).collect();
    assert_eq!(roles, vec![Role::User, Role::Tool, Role::Assistant]);
    assert_eq!(r.messages()[0].text, "Read note");
    let tool = &r.messages()[1];
    assert_eq!(tool.tool_name.as_deref(), Some("Read"));
    assert!(tool.tool_result.as_deref().unwrap().contains("hello world") && !tool.tool_failed);
    assert!(tool.tool_summary().starts_with("Read "));
    assert!(r.messages()[2].text.contains("hello world") && !r.messages()[2].is_streaming);
    assert_eq!(r.last_result().unwrap().subtype, "success");
}

#[test]
fn streamed_text_is_finalized() {
    let mut r = TranscriptReducer::default();
    r.user_sent("say", None);
    let mut saw_streaming = false;
    for e in fixture_events("claude-partial") {
        r.apply(&e);
        if r.messages().last().is_some_and(|m| m.is_streaming) {
            saw_streaming = true;
        }
    }
    assert!(saw_streaming);
    let assistants: Vec<_> = r.messages().iter().filter(|m| m.role == Role::Assistant).collect();
    assert_eq!(assistants.len(), 1);
    assert!(assistants[0].text == "one two three" && !assistants[0].is_streaming);
}

#[test]
fn errors_and_exit() {
    let mut r = TranscriptReducer::new(vec![], Some("old".into()));
    r.user_sent("x", None);
    r.apply(&ClaudeEvent::TextDelta { message_id: None, text: "partial".into() });
    r.apply(&ClaudeEvent::Result(ClaudeResult {
        session_id: Some("new".into()),
        is_error: true,
        subtype: "error_max_turns".into(),
        ..Default::default()
    }));
    assert!(r.session_id() == Some("new") && !r.is_running());
    let last = r.messages().last().unwrap();
    assert!(last.role == Role::Error && last.text.contains("error_max_turns"));
    assert!(r.messages().iter().all(|m| !m.is_streaming));

    r.user_sent("y", None);
    r.process_exited(1, "  boom \n");
    assert!(r.messages().last().unwrap().text == "boom" && !r.is_running());
    let count = r.messages().len();
    r.process_exited(0, "");
    assert_eq!(r.messages().len(), count, "exit while idle adds nothing");

    r.user_sent("z", None);
    r.process_exited(2, "");
    assert_eq!(r.messages().last().unwrap().text, "Claude exited (2).");

    r.user_sent("w", None);
    r.interrupted();
    assert!(r.messages().last().unwrap().role == Role::System && !r.is_running());
}

#[test]
fn tool_result_matched_after_reload() {
    let saved = vec![DiscussMessage::tool("tool1", "Bash", r#"{"command":"ls"}"#)];
    let mut r = TranscriptReducer::new(saved, None);
    r.apply(&ClaudeEvent::ToolResults(vec![ToolResult::new("tool1", "out", true)]));
    assert!(r.messages()[0].tool_result.as_deref() == Some("out") && r.messages()[0].tool_failed);
}

#[test]
fn tool_summaries() {
    let m = |name: &str, input: &str| DiscussMessage::tool(new_id(), name, input).tool_summary();
    assert_eq!(m("Read", r#"{"file_path":"/a/b/c/d/e.swift"}"#), "Read …/c/d/e.swift");
    assert_eq!(m("Edit", r#"{"file_path":"x.swift"}"#), "Edit x.swift");
    assert_eq!(m("Bash", r#"{"command":"npm test\necho done"}"#), "Bash: npm test");
    assert_eq!(m("Grep", r#"{"pattern":"foo"}"#), "Grep \"foo\"");
    assert_eq!(m("Glob", r#"{"pattern":"**/*.ts"}"#), "Glob **/*.ts");
    assert_eq!(m("Mystery", "{}"), "Mystery");
    assert_eq!(m("Read", "not json"), "Read");
    assert_eq!(m("Bash", &format!(r#"{{"command":"{}"}}"#, "é".repeat(100))), format!("Bash: {}", "é".repeat(80)));
    assert_eq!(DiscussMessage::new(Role::User, "plain").tool_summary(), "plain");
}

// MARK: - Process

#[test]
fn config_args() {
    let mut cfg = ClaudeConfig::new("/bin/claude", "/tmp");
    cfg.resume_session_id = Some("S".into());
    cfg.fork_session = true;
    cfg.append_system_prompt = Some("SYS".into());
    cfg.model = Some("opus".into());
    cfg.json_schema = Some("{}".into());
    cfg.extra_args = vec!["--x".into()];
    let s = cfg.streaming_args();
    assert_eq!(&s[..6], &["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"]);
    assert!(s.contains(&"--include-partial-messages".to_string()));
    assert!(s.contains(&"bypassPermissions".to_string()) && s.contains(&"--fork-session".to_string()));
    assert_eq!(s.last().unwrap(), "--x");
    assert!(has_pair(&s, "--resume", "S"));
    assert!(has_pair(&s, "--append-system-prompt", "SYS"));
    assert!(has_pair(&s, "--model", "opus"));
    assert!(!s.contains(&"--json-schema".to_string()));
    let o = cfg.one_shot_args("PROMPT");
    assert_eq!(o.last().unwrap(), "PROMPT");
    assert!(o.contains(&"json".to_string()) && !o.contains(&"stream-json".to_string()));
    assert!(has_pair(&o, "--json-schema", "{}"));
    let mut bare = ClaudeConfig::new("c", "/");
    bare.append_system_prompt = Some(String::new());
    bare.model = Some(String::new());
    let b = bare.streaming_args();
    assert!(!b.contains(&"--model".to_string()) && !b.contains(&"--append-system-prompt".to_string()));
    assert!(!b.contains(&"--resume".to_string()));
}

#[test]
fn session_end_to_end() {
    let fixture = fixture_path("claude-stream");
    let args_file = std::env::temp_dir().join(format!("rb-args-{}", new_id()));
    let script = make_script(&format!(
        "echo \"$@\" > '{}'\nread line\ncat '{}'\n",
        args_file.display(),
        fixture.display()
    ));
    let mut cfg = ClaudeConfig::new(script.to_string_lossy(), std::env::temp_dir());
    cfg.resume_session_id = Some("prev".into());
    let (session, rx) = ClaudeSession::with_channel(cfg, || {});
    session.send("hello").unwrap();
    let mut events = Vec::new();
    let status = loop {
        match rx.recv_timeout(Duration::from_secs(15)).expect("timed out") {
            SessionOutput::Event(e) => events.push(e),
            SessionOutput::Exited { status, .. } => break status,
        }
    };
    assert_eq!(status, 0);
    assert_eq!(events.len(), 9, "{events:?}");
    assert!(matches!(events.first(), Some(ClaudeEvent::Initialized { .. })));
    assert!(matches!(events.last(), Some(ClaudeEvent::Result(_))));
    assert_eq!(session.current_session_id().as_deref(), Some("77dde8c7-4613-4ed2-bfe5-4fa278d39098"));
    assert!(!session.is_process_running());
    let args = std::fs::read_to_string(&args_file).unwrap();
    assert!(args.contains("--resume prev"));
    session.finish();
}

#[test]
fn session_restarts_with_new_session_id_after_interrupt_request() {
    // First run emits init, then exits after reading the interrupt control_request
    // (the SIGINT fallback is not exercised: turn_active is false). The next send
    // restarts with --resume <reported id>.
    let args_file = std::env::temp_dir().join(format!("rb-args-{}", new_id()));
    let script = make_script(&format!(
        "echo \"$@\" >> '{}'\nread line\necho '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"SID1\"}}'\nread line\n",
        args_file.display()
    ));
    let (session, rx) = ClaudeSession::with_channel(ClaudeConfig::new(script.to_string_lossy(), "/"), || {});
    session.send("one").unwrap();
    assert!(matches!(
        rx.recv_timeout(Duration::from_secs(15)).unwrap(),
        SessionOutput::Event(ClaudeEvent::Initialized { .. })
    ));
    assert!(session.is_process_running());
    session.interrupt(); // writes the control_request line; the script's second `read` consumes it and exits
    assert!(matches!(rx.recv_timeout(Duration::from_secs(15)).unwrap(), SessionOutput::Exited { status: 0, .. }));
    assert!(!session.is_process_running());
    session.send("two").unwrap();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(15)).unwrap(), SessionOutput::Event(_)));
    session.terminate();
    assert!(matches!(rx.recv_timeout(Duration::from_secs(15)).unwrap(), SessionOutput::Exited { .. }));
    let args = std::fs::read_to_string(&args_file).unwrap();
    let lines: Vec<&str> = args.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(!lines[0].contains("--resume"));
    assert!(lines[1].contains("--resume SID1"));
}

#[test]
fn one_shot_parses_structured_output() {
    let script = make_script(
        r#"echo '{"type":"result","subtype":"success","is_error":false,"result":"ok","session_id":"S1","structured_output":{"findings":[]}}'"#,
    );
    let res = ClaudeOneShot::default().run(&ClaudeConfig::new(script.to_string_lossy(), std::env::temp_dir()), "p").unwrap();
    assert!(res.text.as_deref() == Some("ok") && res.session_id.as_deref() == Some("S1"));
    assert_eq!(res.structured_output, Some(serde_json::json!({"findings": []})));
}

#[test]
fn one_shot_errors() {
    let cwd = std::env::temp_dir();
    let run = |body: &str| ClaudeOneShot::default().run(&ClaudeConfig::new(make_script(body).to_string_lossy(), &cwd), "p");
    assert_eq!(
        run(r#"echo '{"type":"result","subtype":"error_during_execution","is_error":true,"result":"rate limited"}'; exit 1"#)
            .unwrap_err(),
        ClaudeError::Failed("rate limited".into())
    );
    assert_eq!(run("echo 'auth required' >&2; exit 3").unwrap_err(), ClaudeError::Failed("auth required\n".into()));
    assert_eq!(run("echo hello").unwrap_err(), ClaudeError::BadOutput);
}

// MARK: - Prompts

fn pr() -> PRDetail {
    let mut p = PRDetail::new(PRRef::new("o", "r", 42), "N", "Add cache", "main", "cache", "0123456789abcdef", "fedcba9876543210");
    p.body = "Speeds up lookups.".into();
    p.author = "alice".into();
    p
}

fn thread(path: &str, line: i64, body: &str, resolved: bool) -> ReviewThread {
    let mut t = ReviewThread::new(new_id(), path, Some(line), vec![ReviewComment::new("c", body, "bob", chrono::Utc::now())]);
    t.is_resolved = resolved;
    t
}

#[test]
fn discuss_context_contains_everything() {
    let pr = pr();
    let anchor = CodeAnchor::new("src/cache.swift", DiffSide::Right, 10, 12);
    let s = prompts::discuss_context(
        &pr,
        &anchor,
        &["let a = 1".into(), "let b = 2".into()],
        Some("// around"),
        &[thread("src/cache.swift", 11, "why\nthis?", true)],
        &[thread("other.swift", 3, "nit", false)],
        "Is this thread-safe?",
    );
    assert!(s.contains("o/r#42") && s.contains("Add cache"));
    assert!(s.contains("Speeds up lookups."));
    assert!(s.contains("src/cache.swift lines 10-12 of the new (head) version"));
    assert!(s.contains("let a = 1\nlet b = 2"));
    assert!(s.contains("// around"));
    assert!(s.contains("L11 [resolved]: @bob: why this?"));
    assert!(s.contains("other.swift:3 @bob: nit"));
    assert!(s.ends_with("Is this thread-safe?"));
    assert!(s.contains("0123456789") && !s.contains("0123456789a"));
    let left = prompts::discuss_context(&pr, &CodeAnchor::new("x", DiffSide::Left, 1, 1), &[], None, &[], &[], "q");
    assert!(left.contains("old (base) version") && !left.contains("Surrounding") && !left.contains("Existing review threads"));
}

#[test]
fn other_prompts() {
    let pr = pr();
    let sys = prompts::system_prompt(&pr);
    assert!(sys.contains("refs/review-buddy/base-42"));
    assert!(sys.contains("Do not push"));
    assert!(sys.contains("refs/review-buddy/head-42"));
    assert!(prompts::FIX_INSTRUCTION.contains("commit") && prompts::FIX_INSTRUCTION.contains("Do not push"));
    let d = prompts::draft_comment(&CodeAnchor::new("a.swift", DiffSide::Right, 3, 5));
    assert!(d.contains("a.swift lines 3-5") && d.contains("```suggestion"));
    assert!(prompts::summary(&pr).contains("Speeds up lookups."));
    // Summaries and pre-reviews read the PR as published, never local fix commits.
    assert!(prompts::pre_review(&pr).contains("base-42...refs/review-buddy/head-42"));
    assert!(prompts::summary(&pr).contains("base-42...refs/review-buddy/head-42"));
    assert!(!prompts::pre_review(&pr).contains("...HEAD"));
    assert!(prompts::summary(&pr).contains("PR description:\nSpeeds up lookups.\n\nReturn:"));
}

#[test]
fn schemas_are_valid_json() {
    for schema in [prompts::SUMMARY_SCHEMA, prompts::PRE_REVIEW_SCHEMA] {
        let v: serde_json::Value = serde_json::from_str(schema).unwrap();
        assert_eq!(v["type"], "object");
    }
}

#[test]
fn summary_schema_decodes_into_model() {
    let s: AISummary =
        serde_json::from_str(r#"{"summary":"S","readingOrder":[{"path":"a","reason":"r"}],"risks":[{"description":"d"}]}"#).unwrap();
    assert!(s.summary == "S" && s.reading_order[0].path == "a" && s.risks[0].path.is_none());
}

// MARK: - AI models

#[test]
fn finding_from_raw_model_output() {
    let json = r#"{"findings":[
      {"path":"a.swift","side":"LEFT","startLine":9,"endLine":4,"severity":"high","title":"Bug","body":"b","suggestion":""},
      {"path":"b.swift","side":"weird","startLine":2,"severity":"blocker","title":"T"}
    ]}"#;
    let r: PreReviewResult = serde_json::from_str(json).unwrap();
    let f = &r.findings[0];
    assert!(f.start_line == 4 && f.end_line == 9 && f.side == DiffSide::Left && f.severity == Severity::High);
    assert!(f.suggestion.is_none() && f.state == FindingState::Open && !f.id.is_empty());
    let g = &r.findings[1];
    assert!(g.side == DiffSide::Right && g.end_line == 2 && g.severity == Severity::Low && g.body.is_empty());
    assert_ne!(f.id, g.id);
}

#[test]
fn finding_round_trip_keeps_id_and_state() {
    let mut f = AIFinding::new("p", DiffSide::Right, 1, 2, Severity::Nit, "t", "b");
    f.suggestion = Some("x".into());
    f.state = FindingState::Dismissed;
    let back: AIFinding = serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
    assert_eq!(back, f);
}

#[test]
fn comment_body() {
    let mut f = AIFinding::new("p", DiffSide::Right, 3, 4, Severity::Medium, "Leak", "Closes nothing.");
    f.suggestion = Some("close()".into());
    assert_eq!(f.comment_body(), "**Leak**\n\nCloses nothing.\n\n```suggestion\nclose()\n```");
    assert_eq!(f.anchor(), CodeAnchor::new("p", DiffSide::Right, 3, 4));
    let g = AIFinding::new("p", DiffSide::Right, 3, 3, Severity::Low, "T", "B");
    assert_eq!(g.comment_body(), "**T**\n\nB");
}

#[test]
fn severity_ordering() {
    let mut all = Severity::ALL.to_vec();
    all.reverse();
    all.sort();
    assert_eq!(all, vec![Severity::High, Severity::Medium, Severity::Low, Severity::Nit]);
    assert!(Severity::High < Severity::Nit);
}

#[test]
fn discuss_thread_hash_and_title() {
    let h1 = DiscussThread::hash_lines(&["a", "b"]);
    assert_eq!(h1, DiscussThread::hash_lines(&["a", "b"]));
    assert_ne!(h1, DiscussThread::hash_lines(&["a", "c"]));
    assert_ne!(h1, DiscussThread::hash_lines(&["ab"]));
    let mut t = DiscussThread::new(PRRef::new("o", "r", 1), CodeAnchor::new("x/y.swift", DiffSide::Right, 1, 2), "s", h1);
    assert_eq!(t.title(), "y.swift L1-2");
    t.messages = vec![DiscussMessage::new(Role::Assistant, "hi"), DiscussMessage::new(Role::User, "Why?\nmore")];
    assert_eq!(t.title(), "Why?");
}
