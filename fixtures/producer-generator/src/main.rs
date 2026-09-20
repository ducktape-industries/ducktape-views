use agent_wire::{AgentMsg, AgentReply, ProvisionReceipt};
use runs_wire::{LoadMode, ModelMsg, RunsMsg, SkillRef};
use serde::Serialize;
use serde_json::{Value, json};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

const SDK_REVISION: &str = "8c764093d5974b4328c0b90de157d72597540020";

fn skill(name: &str, always: bool) -> SkillRef {
    SkillRef {
        name: name.into(),
        source_prefix: format!("/shared/skills/{name}"),
        source_snapshot: None,
        load: if always { LoadMode::Always } else { LoadMode::OnDemand },
    }
}

fn envelope<T: Serialize>(target: &str, message: T) -> Value {
    json!({"target": target, "payload": serde_json::to_value(message).unwrap()})
}

fn generated() -> Vec<(&'static str, Value, bool)> {
    let program = runs_wire::model_program("chiefduck");
    let register_skills = vec![skill("chiefduck", true)];
    let update_skills = vec![
        skill("review", true),
        skill("style", true),
        skill("tests", false),
    ];
    vec![
        (
            "agents/tests/fixtures/model-program.json",
            serde_json::to_value(&program).unwrap(),
            false,
        ),
        (
            "agents/tests/fixtures/agent-provision.json",
            serde_json::to_value(AgentReply::Provision(Some(ProvisionReceipt {
                account: 77,
                request_digest: [3; 32],
            })))
            .unwrap(),
            true,
        ),
        (
            "agents/tests/fixtures/provision-request.json",
            envelope(
                "agent",
                AgentMsg::Provision {
                    request_id: "chiefduck".into(),
                    name: "ChiefDuck".into(),
                    program,
                },
            ),
            true,
        ),
        (
            "agents/tests/fixtures/configure-model-register.json",
            envelope(
                "runs",
                RunsMsg::ConfigureModel {
                    operation: ModelMsg::RegisterModel {
                        account: 77,
                        agent_id: "chiefduck".into(),
                        display_name: "ChiefDuck".into(),
                        capability: "claude".into(),
                        recipe_hash: None,
                        skills: Some(register_skills),
                    },
                },
            ),
            true,
        ),
        (
            "agents/tests/fixtures/configure-model-update.json",
            envelope(
                "runs",
                RunsMsg::ConfigureModel {
                    operation: ModelMsg::UpdateModel {
                        agent_id: "reviewer-bot".into(),
                        display_name: Some("Reviewer Bot".into()),
                        capability: Some("claude".into()),
                        recipe_hash: None,
                        skills: Some(update_skills),
                    },
                },
            ),
            true,
        ),
        (
            "agents/tests/fixtures/configure-model-pause.json",
            envelope(
                "runs",
                RunsMsg::ConfigureModel {
                    operation: ModelMsg::PauseModel {
                        agent_id: "reviewer-bot".into(),
                    },
                },
            ),
            true,
        ),
        (
            "support/composer/tests/fixtures/message-rich.json",
            serde_json::to_value(chat_message::parse_message("Hello **world** <@7>\n> quote"))
                .unwrap(),
            false,
        ),
    ]
}

fn read_json(root: &Path, relative: &str) -> Value {
    let path = root.join(relative);
    serde_json::from_str(
        &fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display())),
    )
    .unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn write_json(root: &Path, relative: &str, value: &Value) {
    let path = root.join(relative);
    fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
    )
    .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}

fn main() {
    let mut args = env::args().skip(1);
    let mode = args.next().unwrap_or_else(|| "--check".into());
    let root = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    assert!(mode == "--check" || mode == "--write", "use --check or --write");

    let mut checked = 0;
    for (relative, expected, writable) in generated() {
        if mode == "--write" && writable {
            write_json(&root, relative, &expected);
        }
        let actual = read_json(&root, relative);
        assert_eq!(actual, expected, "producer fixture mismatch: {relative}");
        checked += 1;
    }
    println!("checked {checked} fixtures against SDK {SDK_REVISION}");
}
